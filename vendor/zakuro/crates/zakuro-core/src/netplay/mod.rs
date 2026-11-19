//! online: battles between two Haiku over the Internet or a local network.
//!
//! The game's own Local Battle (Wireless Battle > Local Battle) runs
//! unchanged: its Host/Join menu, the team and rule choices, the map and
//! the battle are the game's, and so is the way it keeps both consoles in
//! step (its Pia session over local wireless). What changes is under it:
//! local wireless (nwm::UDS, pcos/uds.rs) is carried between the two PCs by
//! this module's transport instead of radio, through a `room::Link` like a
//! development build's rooms.
//!
//! - When the host's game puts its network up, the host starts a session:
//!   it opens a UDP port, asks the router to forward it, learns its
//!   addresses and makes the code (code.rs). The frontend shows the code.
//! - When the joiner's game starts looking for networks, the frontend asks
//!   for the code; the joiner reaches the host with it, and from then on the
//!   host's network shows in the game's list like a console nearby.
//! - Either side leaving, or the connection being lost, closes the link,
//!   and the game sees the other console go as it would out of range.
//!
//! See dev/reports/online.md for the whole design.

pub mod code;
pub mod crypto;
pub mod nat;
pub mod transport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::room::{Link, Mac};

pub use code::{Code, CodeError};
pub use transport::{Compat, ModStamp, ModsDiff, RefuseReason, Rules};

/// the addresses the two consoles have on the link, made-up local ones.
pub const HOST_MAC: Mac = [0x02, 0x48, 0x4B, 0x00, 0x00, 0x01];
pub const JOINER_MAC: Mac = [0x02, 0x48, 0x4B, 0x00, 0x00, 0x02];

/// the port a host listens on unless the settings name another; the next
/// ones are tried when it is taken (two Haiku on one PC).
pub const DEFAULT_PORT: u16 = 24874;
pub const DEFAULT_STUN: &str = "stun.cloudflare.com:3478";

/// the version two players must share: the builds' gameplay must match for
/// the battle to stay the same on both. HAIKU_VERSION at build time (a
/// release sets it), else the crate's own.
pub fn haiku_version() -> String {
    // test only: FATES_ONLINE_VERSION pretends to be another version, for
    // checking that mismatched versions are refused
    if let Ok(version) = std::env::var("FATES_ONLINE_VERSION") {
        return version;
    }
    option_env!("HAIKU_VERSION").unwrap_or(concat!("0.1.0+", env!("CARGO_PKG_VERSION"))).to_owned()
}

/// the player's settings for online play ([pc.online]).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// the host's UDP port; 0 picks DEFAULT_PORT.
    pub port: u16,
    /// the STUN server asked for the public address, empty for none.
    pub stun: String,
    pub upnp: bool,
    pub natpmp: bool,
    pub ipv6: bool,
    /// the joiner looks for the host on its own network too.
    pub lan_discovery: bool,
    /// seconds without a packet before the other player counts as gone.
    pub timeout_secs: u32,
    /// test only: the address to listen on and put in the code (loopback
    /// for tests on one PC, which also keeps the firewall out of it).
    pub bind: Option<IpAddr>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            port: 0,
            stun: DEFAULT_STUN.to_owned(),
            upnp: true,
            natpmp: true,
            ipv6: true,
            lan_discovery: true,
            timeout_secs: 15,
            bind: None,
        }
    }
}

/// the settings, and what the environment says over them for tests:
/// FATES_ONLINE_BIND=127.0.0.1, FATES_ONLINE_PORT, FATES_ONLINE_STUN (empty:
/// none), FATES_ONLINE_UPNP=0, FATES_ONLINE_NATPMP=0, FATES_ONLINE_IPV6=0,
/// FATES_ONLINE_TIMEOUT=seconds.
fn config() -> Config {
    let mut config = CONFIG.lock().ok().and_then(|c| c.clone()).unwrap_or_default();
    let env = |name: &str| std::env::var(name).ok();
    if let Some(bind) = env("FATES_ONLINE_BIND").and_then(|v| v.parse().ok()) {
        config.bind = Some(bind);
    }
    if let Some(port) = env("FATES_ONLINE_PORT").and_then(|v| v.parse().ok()) {
        config.port = port;
    }
    if let Some(stun) = env("FATES_ONLINE_STUN") {
        config.stun = stun;
    }
    for (name, flag) in [("FATES_ONLINE_UPNP", &mut config.upnp), ("FATES_ONLINE_NATPMP", &mut config.natpmp), ("FATES_ONLINE_IPV6", &mut config.ipv6), ("FATES_ONLINE_LAN", &mut config.lan_discovery)] {
        if let Some(value) = env(name) {
            *flag = value != "0";
        }
    }
    if let Some(seconds) = env("FATES_ONLINE_TIMEOUT").and_then(|v| v.parse().ok()) {
        config.timeout_secs = seconds;
    }
    config
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

/// the frontend's settings, whenever they change.
pub fn configure(config: Config) {
    if let Ok(mut now) = CONFIG.lock() {
        *now = Some(config);
    }
}

/// why a session ended or never started, for the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// nothing answered at the code's addresses.
    Unreachable,
    /// a host answered, but holds no session for this code.
    Expired,
    /// the two run different versions of Haiku.
    Version { host: String, ours: String },
    /// the host already has its opponent.
    Busy,
    /// online2: the gameplay mods differ (from this side: its own, the
    /// host's, and those both have in other versions).
    Mods(ModsDiff),
    /// the other player left.
    PeerLeft,
    /// the connection was lost.
    Lost,
    /// no socket could be opened.
    Network(String),
}

/// where the session is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Progress {
    #[default]
    Idle,
    /// host: opening the port and learning the addresses.
    Preparing,
    /// host: the code to give, waiting for the player.
    Hosting { code: String, mapped: bool },
    /// joiner: reaching the host.
    Connecting,
    Connected { peer_version: String, rtt_ms: Option<u32> },
    /// joiner: the host could not be reached or turned it away.
    Failed(Failure),
    /// the connection ended other than by this side.
    Closed(Failure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Joiner,
}

struct Session {
    role: Role,
    link: Link,
    shared: transport::Shared,
    stop: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// changes with each new session, for pcos/uds.rs to take its link.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// the game is in its Local Battle (link::match::Local is up): only then
/// does local wireless work, so My Castle's Parley still finds no one.
static BATTLE: AtomicBool = AtomicBool::new(false);
/// the game's network list is open with no host: the frontend asks for
/// the code.
static WANTS_CODE: AtomicBool = AtomicBool::new(false);
/// a session's end the frontend has yet to tell the player.
static NOTICE: Mutex<Option<Notice>> = Mutex::new(None);
/// online2: the rules this session runs under: the host's as it sent them
/// in its WELCOME, on both sides.
static RULES: Mutex<Option<Rules>> = Mutex::new(None);
/// online2: the host's choice, from the settings (and the Host panel).
static HOST_RULES: Mutex<Option<Rules>> = Mutex::new(None);
/// online2: what this side brings (its gameplay mods), from the frontend.
static COMPAT: Mutex<Option<Compat>> = Mutex::new(None);

/// what the player is told once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Failed(Failure),
    /// host: a joiner was turned away.
    Refused(RefuseReason, String),
    /// online2: host: a joiner with other gameplay mods was turned away.
    RefusedMods(ModsDiff),
    Connected(Role),
    /// online2: the teams were refused: a character changed by cheats in
    /// this side's team (bit 0) or the other's (bit 1), in a battle whose
    /// host doesn't allow them.
    CheatedTeams(u32),
}

fn notice(notice: Notice) {
    if let Ok(mut slot) = NOTICE.lock() {
        *slot = Some(notice);
    }
}

pub fn take_notice() -> Option<Notice> {
    NOTICE.lock().ok().and_then(|mut slot| slot.take())
}

pub(crate) fn note_refused(reason: RefuseReason, version: &str, mods: ModsDiff) {
    if reason == RefuseReason::Mods {
        notice(Notice::RefusedMods(mods));
    } else {
        notice(Notice::Refused(reason, version.to_owned()));
    }
}

pub(crate) fn note_rules(rules: Rules) {
    if let Ok(mut slot) = RULES.lock() {
        *slot = Some(rules);
    }
}

/// the host's rules this session runs under (both sides), once connected.
pub fn rules() -> Option<Rules> {
    RULES.lock().ok().and_then(|slot| slot.clone())
}

/// online2: the host's choice for its next battles (the settings', or the
/// Host panel's). Tests: FATES_ONLINE_CHEATS=allow or forbid says over it.
pub fn set_host_rules(rules: Rules) {
    if let Ok(mut slot) = HOST_RULES.lock() {
        *slot = Some(rules);
    }
}

pub fn host_rules() -> Rules {
    let mut rules = HOST_RULES.lock().ok().and_then(|slot| slot.clone()).unwrap_or_default();
    match std::env::var("FATES_ONLINE_CHEATS").as_deref() {
        Ok("allow") => rules.allow_cheated_units = true,
        Ok("forbid") => rules.allow_cheated_units = false,
        _ => {}
    }
    rules
}

/// online2: what this side brings, set by the frontend once the game has
/// started with its mods.
pub fn set_compat(compat: Compat) {
    if let Ok(mut slot) = COMPAT.lock() {
        *slot = Some(compat);
    }
}

pub fn compat() -> Compat {
    COMPAT.lock().ok().and_then(|slot| slot.clone()).unwrap_or_default()
}

/// online2: whether characters changed by cheats are kept out of this
/// battle: the game's Local Battle is up, a player is connected, and the
/// host said "Don't allow".
pub fn cheats_forbidden() -> bool {
    battle() && rules().is_some_and(|rules| !rules.allow_cheated_units)
}

/// online2: the same, asked from Select Team (0) or the teams' check (1).
/// Test only: FATES_ONLINE_TEST_NO_GREY=1 leaves Select Team alone, as a
/// program that doesn't grey the teams would, so the check is what stops
/// such a team.
pub fn cheats_forbidden_at(place: u32) -> bool {
    if place == 0 && std::env::var_os("FATES_ONLINE_TEST_NO_GREY").is_some() {
        return false;
    }
    cheats_forbidden()
}

/// online2: overrides/online.c saw a team with a character changed by
/// cheats where they aren't allowed: 1 a greyed team was chosen in Select
/// Team (the game says why), 2 | sides << 4 the teams were refused.
pub fn cheated_team(what: u32) {
    if what == 1 {
        log::info!("online: a team with characters modified with cheats was chosen; they aren't allowed in this battle");
        return;
    }
    let sides = (what >> 4) & 3;
    log::info!(
        "online: the teams were refused: characters modified with cheats in {} (not allowed in this battle)",
        match sides {
            1 => "this player's team",
            2 => "the other player's team",
            _ => "both teams",
        }
    );
    notice(Notice::CheatedTeams(sides));
}

/// the game's Local Battle started (true) or ended (false).
pub fn set_battle(on: bool) {
    let was = BATTLE.swap(on, Ordering::Relaxed);
    if was != on {
        log::info!("online: the game's Local Battle {}", if on { "began: local wireless goes online" } else { "ended" });
    }
    if !on {
        WANTS_CODE.store(false, Ordering::Relaxed);
        stop("the Local Battle ended");
    }
}

pub fn battle() -> bool {
    BATTLE.load(Ordering::Relaxed)
}

/// the game opened its network list (to join): the code is wanted unless a
/// session is already up.
pub fn want_code() {
    let live = SESSION.lock().ok().is_some_and(|s| s.as_ref().is_some_and(|s| s.role == Role::Joiner && s.link.open()));
    if !live {
        stop("the joiner looks for a host again");
        WANTS_CODE.store(true, Ordering::Relaxed);
        log::info!("online: the game looks for a host: the code is asked for");
    }
}

pub fn wants_code() -> bool {
    WANTS_CODE.load(Ordering::Relaxed)
}

/// the player closed the code box without joining.
pub fn cancel_code() {
    WANTS_CODE.store(false, Ordering::Relaxed);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// the live session's link, for local wireless.
pub fn link() -> Option<Link> {
    SESSION.lock().ok()?.as_ref().map(|session| session.link.clone())
}

pub fn role() -> Option<Role> {
    SESSION.lock().ok()?.as_ref().map(|session| session.role)
}

pub fn progress() -> Progress {
    let Ok(session) = SESSION.lock() else { return Progress::Idle };
    match session.as_ref() {
        Some(session) => session.shared.lock().map(|p| p.clone()).unwrap_or_default(),
        None => Progress::Idle,
    }
}

/// ends the session, if any: the transport says goodbye after what is
/// still to be sent and closes the link.
pub fn stop(why: &str) {
    let ended = SESSION.lock().ok().and_then(|mut slot| slot.take());
    if let Some(session) = ended {
        log::info!("online: the session ends ({why})");
        drop(session);
    }
    // online2: the next session gets its own rules
    if let Ok(mut slot) = RULES.lock() {
        *slot = None;
    }
}

/// a session's transport ended by itself (lost, refused, the other side
/// left): the player is told, and it is gone.
pub fn poll() {
    let Ok(mut slot) = SESSION.lock() else { return };
    let Some(session) = slot.as_ref() else { return };
    let progress = session.shared.lock().map(|p| p.clone()).unwrap_or_default();
    match progress {
        Progress::Failed(failure) => {
            log::info!("online: joining failed: {failure:?}");
            notice(Notice::Failed(failure));
            *slot = None;
            // the box comes back, for the code again
            WANTS_CODE.store(battle(), Ordering::Relaxed);
        }
        Progress::Closed(failure) => {
            log::info!("online: the session closed: {failure:?}");
            notice(Notice::Failed(failure));
            *slot = None;
            if let Ok(mut rules) = RULES.lock() {
                *rules = None;
            }
        }
        _ => {}
    }
}

/// online2: what this side brings now: its gameplay mods, and a randomized
/// run the game is in (its characters' table is the run's), which the
/// other player must share.
fn compat_now() -> Compat {
    let mut compat = compat();
    // eur: the EUR build only plays the EUR build: it shows to the other
    // side as a gameplay mod the USA build doesn't have, so the two never
    // match (their code and data differ; nothing says a battle between them
    // would stay in step)
    if crate::region::is_eur() {
        compat.gameplay_mods.push(ModStamp { id: "haiku.region.eur".into(), name: "EUR Special Edition (CTR-P-BFZP)".into(), hash: crate::region::EUR });
    }
    // jpprep: the Japanese build only plays the same Japanese build
    if crate::region::is_jpn() {
        compat.gameplay_mods.push(ModStamp { id: "haiku.region.jpn".into(), name: format!("Japanese build, Fire Emblem if ({:016X})", crate::region::title()), hash: crate::region::title() });
    }
    if let Some(choice) = crate::randomizer::current() {
        compat.gameplay_mods.push(ModStamp {
            id: "haiku.randomizer".into(),
            name: format!("Randomizer (seed {})", crate::randomizer::seed_text(choice.seed)),
            hash: (choice.seed as u64) << 32 ^ (choice.options as u64) << 8 ^ choice.version as u64,
        });
        compat.gameplay_mods.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    }
    compat
}

fn options(config: &Config) -> transport::Options {
    transport::Options {
        version: haiku_version(),
        compat: compat_now(),
        rules: host_rules(),
        timeout: Duration::from_secs(config.timeout_secs.max(3) as u64),
        connect_timeout: Duration::from_secs(10),
        lan_discovery: config.lan_discovery,
    }
}

/// host: starts a session for the network the game puts up, and gives the
/// game's local wireless its link at once (the code follows in a moment).
pub fn host() -> Link {
    stop("a new network goes up");
    WANTS_CODE.store(false, Ordering::Relaxed);
    let config = config();
    let (link, end) = Link::external(HOST_MAC, "online battle");
    let shared: transport::Shared = Arc::new(Mutex::new(Progress::Preparing));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let session = Session { role: Role::Host, link: link.clone(), shared: shared.clone(), stop: stop_flag.clone() };
    let options = options(&config);
    let spawned = std::thread::Builder::new().name("online host".into()).spawn(move || {
        let Some(prepared) = prepare_host(&config) else {
            set(&shared, Progress::Failed(Failure::Network("no UDP port could be opened".into())));
            end.close();
            return;
        };
        set(&shared, Progress::Hosting { code: prepared.code.text(), mapped: prepared.mapping.is_some() });
        log::info!("online: hosting; the code is {}", prepared.code.text());
        if let Ok(path) = std::env::var("FATES_ONLINE_CODE_OUT") {
            // test only: the code for the other instance's script
            let _ = std::fs::write(&path, prepared.code.text());
        }
        let mapping = prepared.mapping.clone();
        transport::serve(prepared.sockets, prepared.code.secret, end, JOINER_MAC, stop_flag, shared, options);
        if let Some(mapping) = mapping {
            nat::release(&mapping.how);
        }
    });
    if spawned.is_ok() {
        if let Ok(mut slot) = SESSION.lock() {
            *slot = Some(session);
        }
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    link
}

fn set(shared: &transport::Shared, progress: Progress) {
    if let Ok(mut now) = shared.lock() {
        *now = progress;
    }
}

struct Prepared {
    sockets: Vec<UdpSocket>,
    code: Code,
    mapping: Option<nat::Mapped>,
}

/// opens the host's port and finds what goes in its code.
fn prepare_host(config: &Config) -> Option<Prepared> {
    let bind_ip = config.bind.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let first = if config.port == 0 { DEFAULT_PORT } else { config.port };
    let mut socket = None;
    for port in (first..first.saturating_add(16)).chain(std::iter::once(0)) {
        if let Ok(bound) = UdpSocket::bind(SocketAddr::new(bind_ip, port)) {
            socket = Some(bound);
            break;
        }
    }
    let socket = socket?;
    let port = socket.local_addr().ok()?.port();
    let loopback = bind_ip.is_loopback();
    let lan = match bind_ip {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => nat::local_ipv4(),
    };
    let mut code = Code { port, public4: None, public_port: None, lan4: None, v6: None, secret: crypto::random() };
    let mut mapping = None;
    if !loopback {
        if let Some(lan) = lan.filter(|_| config.upnp || config.natpmp) {
            mapping = nat::map_port(lan, port, config.upnp, config.natpmp);
        }
        if let Some(mapped) = &mapping {
            code.public4 = mapped.public.filter(|ip| !nat::is_private(*ip));
            code.public_port = Some(mapped.port);
        }
        if code.public4.is_none() && !config.stun.is_empty() {
            match nat::stun(&socket, &config.stun) {
                Some(public) => {
                    log::info!("online: STUN ({}) sees this host at {public}", config.stun);
                    if !nat::is_private(*public.ip()) {
                        code.public4 = Some(*public.ip());
                        code.public_port = Some(public.port());
                    }
                }
                None => log::info!("online: STUN ({}) did not answer", config.stun),
            }
        }
    }
    // the host's own network: when there is no public address to give, or
    // the public one is only STUN's guess
    if code.public4.is_none() || mapping.is_none() {
        code.lan4 = lan.or(loopback.then_some(Ipv4Addr::LOCALHOST));
    }
    let mut sockets = vec![socket];
    if config.ipv6 && !loopback && mapping.is_none() {
        if let Some(v6) = nat::global_ipv6() {
            if let Ok(socket) = UdpSocket::bind(SocketAddr::new(IpAddr::V6(v6), port)) {
                code.v6 = Some(v6);
                sockets.push(socket);
            }
        }
    }
    // the code stays short: the LAN address only when it has to
    if code.public4.is_some() && code.v6.is_some() {
        code.lan4 = None;
    }
    Some(Prepared { sockets, code, mapping })
}

/// joiner: reaches the host the code names. The link goes to the game's
/// local wireless as soon as the host answers.
pub fn join(text: &str) -> Result<(), CodeError> {
    let code = Code::parse(text)?;
    stop("the joiner reaches a new host");
    WANTS_CODE.store(false, Ordering::Relaxed);
    let config = config();
    let (link, end) = Link::external(JOINER_MAC, "online battle");
    let shared: transport::Shared = Arc::new(Mutex::new(Progress::Connecting));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let session = Session { role: Role::Joiner, link, shared: shared.clone(), stop: stop_flag.clone() };
    let options = options(&config);
    let bind = config.bind;
    let spawned = std::thread::Builder::new()
        .name("online joiner".into())
        .spawn(move || transport::reach(code, bind, end, HOST_MAC, stop_flag, shared, options));
    if spawned.is_ok() {
        if let Ok(mut slot) = SESSION.lock() {
            *slot = Some(session);
        }
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

/// whether the joiner's link is up (connected to the host).
pub fn connected() -> bool {
    matches!(progress(), Progress::Connected { .. })
}

/// Haiku is closing: a session's other side is told at once (BYE), so it
/// does not wait out the timeout.
pub fn shutdown() {
    if SESSION.lock().ok().is_some_and(|slot| slot.is_some()) {
        stop("Haiku is closing");
        // the transport says goodbye within a few milliseconds
        std::thread::sleep(Duration::from_millis(150));
    }
}
