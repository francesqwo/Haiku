//! online: the connection between two Haiku, over UDP.
//!
//! One UDP socket a side. Every packet starts with `HKO` and a kind:
//!
//! | kind | from | what |
//! |---|---|---|
//! | DISCOVER | joiner | "is session S on this network?", sent to the LAN's broadcast address and this PC |
//! | HERE | host | "it is, here", to a DISCOVER for its session |
//! | HELLO | joiner | session, joiner nonce, protocol, Haiku version, compatibility; HMAC-tagged with the session key |
//! | WELCOME | host | host nonce, protocol, version, the host's rules; tagged |
//! | REFUSE | host | why the host turned the joiner away (version, busy, mods; online2: with the host's gameplay mods); tagged |
//! | UNKNOWN | host | "no such session here": an old code (not tagged) |
//! | DATA | both | a counter and a frame sealed with the direction's key |
//!
//! The frames inside DATA make a reliable, ordered channel: numbered
//! fragments (MSG) acknowledged with a cumulative number and a bitmap of
//! the 64 after it (ACK), sent again after a timeout from the measured
//! round trip, and PING/PONG to keep the NAT mapping open and the peer
//! known to be there. BYE says a side left. What goes through the channel
//! is the consoles' own local wireless traffic (pcos/uds.rs's `Wireless`
//! messages), which the game expects to arrive in order; the game's own
//! session layer (Pia) keeps its own timeouts and resends on top.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::code::Code;
use super::crypto::{self, Keys, TAG};
use super::{Failure, Progress};
use crate::room::{LinkEnd, Mac};

const MAGIC: &[u8; 3] = b"HKO";
const DISCOVER: u8 = 1;
const HERE: u8 = 2;
const HELLO: u8 = 3;
const WELCOME: u8 = 4;
const REFUSE: u8 = 5;
const DATA: u8 = 6;
const UNKNOWN: u8 = 7;

/// the transport's protocol; Haiku with another one do not meet.
/// online2: 2, the host's rules are kept by both and the gameplay mods
/// must match.
pub const PROTOCOL: u16 = 2;

/// the most a fragment holds, so a sealed packet stays under the usual
/// 1280-byte path MTU with room for the headers.
const FRAGMENT: usize = 1100;
/// fragments in flight before waiting for acknowledgement.
const WINDOW: usize = 256;
const PING_EVERY: Duration = Duration::from_millis(1000);
const HELLO_EVERY: Duration = Duration::from_millis(300);
const DISCOVER_EVERY: Duration = Duration::from_millis(700);
/// how long a leaving side waits for what it sent to be acknowledged.
const FLUSH: Duration = Duration::from_millis(1500);

/// the frames inside DATA.
const F_MSG: u8 = 0;
const F_ACK: u8 = 1;
const F_PING: u8 = 2;
const F_PONG: u8 = 3;
const F_BYE: u8 = 4;

/// application messages, the first byte of a reassembled message.
const APP_LINK: u8 = 0;

/// what a joiner says of itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u16,
    pub version: String,
    pub compat: Compat,
}

/// what has to match, or be allowed, for two players to battle. Phase 1
/// sends it and checks the version; online2 fills the mods in (see
/// dev/reports/online2.md).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Compat {
    /// online2: the gameplay mods the game runs with, by id, name and
    /// content hash, sorted by id; both sides must have the same set
    /// (visual mods are left out).
    pub gameplay_mods: Vec<ModStamp>,
    /// phase 1's hook, not used: the game's own teams are checked in the
    /// game (Select Team and the teams' exchange, overrides/online.c),
    /// where the flag travels with each character.
    pub cheated_units: bool,
}

/// online2: a gameplay mod as the other side sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStamp {
    pub id: String,
    pub name: String,
    pub hash: u64,
}

/// online2: how two lists of gameplay mods differ, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModsDiff {
    /// on this side only.
    pub ours: Vec<String>,
    /// on the other side only.
    pub theirs: Vec<String>,
    /// on both, with different contents (another version of the mod).
    pub changed: Vec<String>,
}

impl ModsDiff {
    pub fn of(ours: &[ModStamp], theirs: &[ModStamp]) -> ModsDiff {
        let mut diff = ModsDiff::default();
        for mine in ours {
            match theirs.iter().find(|other| other.id.eq_ignore_ascii_case(&mine.id)) {
                None => diff.ours.push(mine.name.clone()),
                Some(other) if other.hash != mine.hash => diff.changed.push(mine.name.clone()),
                Some(_) => {}
            }
        }
        for other in theirs {
            if !ours.iter().any(|mine| mine.id.eq_ignore_ascii_case(&other.id)) {
                diff.theirs.push(other.name.clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.ours.is_empty() && self.theirs.is_empty() && self.changed.is_empty()
    }
}

/// the host's answer and its rules for the battle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u16,
    pub version: String,
    pub rules: Rules,
}

/// what the host decided for this battle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// online2: characters the Cheats tab changed may be in the teams
    /// (the host's "Characters modified with cheats: Allow / Don't allow").
    pub allow_cheated_units: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefuseReason {
    Version,
    Protocol,
    Busy,
    /// later phase: the gameplay mods differ.
    Mods,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Refusal {
    reason: RefuseReason,
    version: String,
    /// online2: the host's gameplay mods, so the joiner can say which differ
    /// (last, so a protocol-1 joiner still reads the reason and version)
    mods: Vec<ModStamp>,
}

/// a protocol-1 host's refusal
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefusalV1 {
    reason: RefuseReason,
    version: String,
}

/// what the transport needs to know of the program and the player's
/// settings.
#[derive(Debug, Clone)]
pub struct Options {
    pub version: String,
    pub compat: Compat,
    pub rules: Rules,
    /// no packet from the other side for this long: it is gone.
    pub timeout: Duration,
    /// how long a joiner tries to reach the host.
    pub connect_timeout: Duration,
    /// the joiner looks for the host on its own network (broadcast).
    pub lan_discovery: bool,
}

pub type Shared = Arc<Mutex<Progress>>;

fn set(shared: &Shared, progress: Progress) {
    if let Ok(mut now) = shared.lock() {
        *now = progress;
    }
}

fn header(kind: u8, session: &[u8; 4]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(MAGIC);
    packet.push(kind);
    packet.extend_from_slice(session);
    packet
}

/// a packet's kind and session, when it is one of ours.
fn parse_header(packet: &[u8]) -> Option<(u8, [u8; 4], &[u8])> {
    if packet.len() < 8 || &packet[..3] != MAGIC {
        return None;
    }
    Some((packet[3], packet[4..8].try_into().unwrap(), &packet[8..]))
}

fn body_with_len(packet: &mut Vec<u8>, body: &[u8]) {
    packet.extend_from_slice(&(body.len() as u16).to_le_bytes());
    packet.extend_from_slice(body);
}

fn take_body(rest: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u16::from_le_bytes(rest.get(..2)?.try_into().unwrap()) as usize;
    let body = rest.get(2..2 + len)?;
    Some((body, &rest[2 + len..]))
}

// -- the reliable channel -------------------------------------------------

struct Out {
    seq: u32,
    frame: Vec<u8>,
    sent: Option<Instant>,
    tries: u32,
}

/// numbered fragments, sent again until acknowledged, and taken in order.
struct Channel {
    next_seq: u32,
    pending: VecDeque<Out>,
    receive_next: u32,
    early: BTreeMap<u32, (bool, Vec<u8>)>,
    partial: Vec<u8>,
    srtt: Option<f64>,
    rttvar: f64,
    rto: Duration,
    ack_due: bool,
    /// when the oldest fragment still unacknowledged was first sent.
    stuck_since: Option<Instant>,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            next_seq: 0,
            pending: VecDeque::new(),
            receive_next: 0,
            early: BTreeMap::new(),
            partial: Vec::new(),
            srtt: None,
            rttvar: 0.0,
            rto: Duration::from_millis(300),
            ack_due: false,
            stuck_since: None,
        }
    }

    fn queue(&mut self, message: &[u8]) {
        let pieces: Vec<&[u8]> = if message.is_empty() { vec![&[][..]] } else { message.chunks(FRAGMENT).collect() };
        let count = pieces.len();
        for (i, piece) in pieces.into_iter().enumerate() {
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            let mut frame = Vec::with_capacity(6 + piece.len());
            frame.push(F_MSG);
            frame.extend_from_slice(&seq.to_le_bytes());
            frame.push((i + 1 == count) as u8);
            frame.extend_from_slice(piece);
            self.pending.push_back(Out { seq, frame, sent: None, tries: 0 });
        }
    }

    /// the frames to send now: new ones within the window, and those whose
    /// acknowledgement is overdue.
    fn due(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let rto = self.rto;
        for item in self.pending.iter_mut().take(WINDOW) {
            let send = match item.sent {
                None => true,
                Some(at) => now.duration_since(at) >= rto * (1u32 << (item.tries.max(1) - 1).min(3)),
            };
            if send {
                item.sent = Some(now);
                item.tries += 1;
                out.push(item.frame.clone());
            }
        }
        if let Some(first) = self.pending.front() {
            if self.stuck_since.is_none() {
                self.stuck_since = first.sent;
            }
        } else {
            self.stuck_since = None;
        }
        out
    }

    fn ack_frame(&self) -> Vec<u8> {
        let mut mask = 0u64;
        for (&seq, _) in self.early.range(self.receive_next.wrapping_add(1)..) {
            let offset = seq.wrapping_sub(self.receive_next).wrapping_sub(1);
            if offset < 64 {
                mask |= 1 << offset;
            }
        }
        let mut frame = vec![F_ACK];
        frame.extend_from_slice(&self.receive_next.to_le_bytes());
        frame.extend_from_slice(&mask.to_le_bytes());
        frame
    }

    fn acknowledged(&mut self, next: u32, mask: u64, now: Instant) {
        let before = self.pending.len();
        let mut sample = None;
        self.pending.retain(|item| {
            let below = next.wrapping_sub(item.seq).wrapping_sub(1) < 0x8000_0000;
            let offset = item.seq.wrapping_sub(next).wrapping_sub(1);
            let marked = offset < 64 && mask & (1 << offset) != 0;
            let done = below || marked;
            if done && item.tries == 1 {
                if let Some(at) = item.sent {
                    sample = Some(now.duration_since(at).as_secs_f64());
                }
            }
            !done
        });
        if let Some(rtt) = sample {
            match self.srtt {
                None => {
                    self.srtt = Some(rtt);
                    self.rttvar = rtt / 2.0;
                }
                Some(srtt) => {
                    self.rttvar = 0.75 * self.rttvar + 0.25 * (srtt - rtt).abs();
                    self.srtt = Some(0.875 * srtt + 0.125 * rtt);
                }
            }
            let rto = self.srtt.unwrap() + 4.0 * self.rttvar;
            self.rto = Duration::from_secs_f64(rto.clamp(0.05, 2.0));
        }
        if self.pending.len() != before {
            self.stuck_since = None;
        }
    }

    /// a fragment: the whole messages it completes, in order.
    fn received(&mut self, seq: u32, last: bool, data: &[u8]) -> Vec<Vec<u8>> {
        self.ack_due = true;
        let ahead = seq.wrapping_sub(self.receive_next);
        if ahead >= 0x8000_0000 {
            return Vec::new(); // already had it
        }
        if ahead > 4096 {
            return Vec::new();
        }
        self.early.insert(seq, (last, data.to_vec()));
        let mut messages = Vec::new();
        while let Some((last, data)) = self.early.remove(&self.receive_next) {
            self.receive_next = self.receive_next.wrapping_add(1);
            self.partial.extend_from_slice(&data);
            if last {
                messages.push(std::mem::take(&mut self.partial));
            }
        }
        messages
    }

    fn idle(&self) -> bool {
        self.pending.is_empty()
    }
}

// -- a connection ----------------------------------------------------------

/// the replay window of a direction: the highest counter seen and the 64
/// below it.
#[derive(Default)]
struct Replay {
    top: Option<u64>,
    seen: u64,
}

impl Replay {
    fn fresh(&self, counter: u64) -> bool {
        match self.top {
            None => true,
            Some(top) if counter > top => true,
            Some(top) => top - counter < 64 && self.seen & (1 << (top - counter)) == 0,
        }
    }

    fn mark(&mut self, counter: u64) {
        match self.top {
            None => {
                self.top = Some(counter);
                self.seen = 1;
            }
            Some(top) if counter > top => {
                let shift = counter - top;
                self.seen = if shift >= 64 { 0 } else { self.seen << shift };
                self.seen |= 1;
                self.top = Some(counter);
            }
            Some(top) => self.seen |= 1 << (top - counter),
        }
    }
}

struct Connection {
    socket: UdpSocket,
    peer: SocketAddr,
    session: [u8; 4],
    host: bool,
    keys: Keys,
    counter: u64,
    replay: Replay,
    channel: Channel,
    heard: Instant,
    pinged: Instant,
    /// the WELCOME sent, for a HELLO that comes again (its answer lost).
    welcome: Option<(Vec<u8>, [u8; 16])>,
    /// the handshake key, for turning a second joiner away.
    auth: [u8; 32],
}

/// why a connection ended.
enum End {
    /// this side stopped it.
    Stopped,
    /// the other side said goodbye.
    PeerLeft,
    /// nothing from the other side for too long.
    Lost,
}

impl Connection {
    fn send_frame(&mut self, frame: &[u8]) {
        let mut packet = header(DATA, &self.session);
        packet.extend_from_slice(&self.counter.to_le_bytes());
        let key = if self.host { &self.keys.to_joiner } else { &self.keys.to_host };
        let sealed = crypto::seal(key, self.counter, &packet, frame);
        self.counter += 1;
        packet.extend_from_slice(&sealed);
        let _ = self.socket.send_to(&packet, self.peer);
        self.pinged = Instant::now();
    }

    /// a DATA packet: its frame, when it is genuine and new.
    fn open(&mut self, from: SocketAddr, packet: &[u8]) -> Option<Vec<u8>> {
        let (kind, session, rest) = parse_header(packet)?;
        if kind != DATA || session != self.session || rest.len() < 8 + TAG {
            return None;
        }
        let counter = u64::from_le_bytes(rest[..8].try_into().unwrap());
        if !self.replay.fresh(counter) {
            return None;
        }
        let key = if self.host { &self.keys.to_host } else { &self.keys.to_joiner };
        let frame = crypto::open(key, counter, &packet[..16], &rest[8..])?;
        self.replay.mark(counter);
        self.heard = Instant::now();
        // the other side's address changed (its NAT mapping moved): what
        // decrypts is it
        if from != self.peer {
            log::info!("online: the other player's address changed to {from}");
            self.peer = from;
        }
        Some(frame)
    }

    /// carries the consoles' traffic until one side leaves or is lost.
    fn run(mut self, end: &LinkEnd, peer_mac: Mac, stop: &AtomicBool, options: &Options, shared: &Shared, peer_version: String) -> End {
        let _ = self.socket.set_read_timeout(Some(Duration::from_millis(4)));
        let mut buffer = vec![0u8; 2048];
        let mut leaving: Option<Instant> = None;
        let mut last_rtt_report = Instant::now();
        loop {
            let now = Instant::now();
            if stop.load(Ordering::Relaxed) && leaving.is_none() {
                leaving = Some(now);
            }
            // what this console sent
            if leaving.is_none() {
                for (_, body) in end.take() {
                    let mut message = Vec::with_capacity(body.len() + 1);
                    message.push(APP_LINK);
                    message.extend_from_slice(&body);
                    self.channel.queue(&message);
                }
            }
            // what is due on the wire
            for frame in self.channel.due(now) {
                self.send_frame(&frame);
            }
            if self.channel.ack_due {
                self.channel.ack_due = false;
                let ack = self.channel.ack_frame();
                self.send_frame(&ack);
            }
            if now.duration_since(self.pinged) >= PING_EVERY {
                let mut ping = vec![F_PING];
                ping.extend_from_slice(&(now.elapsed().as_micros() as u64).to_le_bytes());
                self.send_frame(&ping);
            }
            if let Some(since) = leaving {
                if self.channel.idle() || now.duration_since(since) >= FLUSH {
                    for _ in 0..3 {
                        self.send_frame(&[F_BYE, 0]);
                    }
                    return End::Stopped;
                }
            }
            if now.duration_since(self.heard) >= options.timeout {
                log::info!("online: nothing from the other player for {:?}: the connection is lost", options.timeout);
                return End::Lost;
            }
            if self.channel.stuck_since.is_some_and(|at| now.duration_since(at) >= options.timeout) {
                log::info!("online: the other player stopped taking what this side sends: the connection is lost");
                return End::Lost;
            }
            if last_rtt_report.elapsed() >= Duration::from_secs(2) {
                last_rtt_report = Instant::now();
                let rtt = self.channel.srtt.map(|s| (s * 1000.0) as u32);
                set(shared, Progress::Connected { peer_version: peer_version.clone(), rtt_ms: rtt });
            }
            // what came
            match self.socket.recv_from(&mut buffer) {
                Ok((size, from)) => {
                    let packet = buffer[..size].to_vec();
                    if let Some((HELLO, session, _)) = parse_header(&packet) {
                        // the WELCOME was lost: the same answer again
                        if self.host && session == self.session {
                            if let Some((welcome, nonce)) = &self.welcome {
                                if packet.get(8..24) == Some(&nonce[..]) {
                                    let _ = self.socket.send_to(welcome, from);
                                } else {
                                    let refusal = refuse_packet(&self.session, &self.auth, RefuseReason::Busy, &options.version, &[], &packet);
                                    if let Some(refusal) = refusal {
                                        let _ = self.socket.send_to(&refusal, from);
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    let Some(frame) = self.open(from, &packet) else { continue };
                    match frame.first() {
                        Some(&F_MSG) if frame.len() >= 6 => {
                            let seq = u32::from_le_bytes(frame[1..5].try_into().unwrap());
                            let last = frame[5] != 0;
                            for message in self.channel.received(seq, last, &frame[6..]) {
                                if message.first() == Some(&APP_LINK) {
                                    end.deliver(peer_mac, message[1..].to_vec());
                                }
                            }
                        }
                        Some(&F_ACK) if frame.len() >= 13 => {
                            let next = u32::from_le_bytes(frame[1..5].try_into().unwrap());
                            let mask = u64::from_le_bytes(frame[5..13].try_into().unwrap());
                            self.channel.acknowledged(next, mask, Instant::now());
                        }
                        Some(&F_PING) => {
                            let mut pong = frame.clone();
                            pong[0] = F_PONG;
                            self.send_frame(&pong);
                        }
                        Some(&F_PONG) => {}
                        Some(&F_BYE) => {
                            log::info!("online: the other player left");
                            return End::PeerLeft;
                        }
                        _ => {}
                    }
                }
                // WouldBlock/TimedOut: nothing came; ConnectionReset: an
                // ICMP "port unreachable" of an earlier packet (Windows)
                Err(_) => {}
            }
        }
    }
}

fn refuse_packet(session: &[u8; 4], key: &[u8; 32], reason: RefuseReason, version: &str, mods: &[ModStamp], hello: &[u8]) -> Option<Vec<u8>> {
    let nonce = hello.get(8..24)?;
    let mut packet = header(REFUSE, session);
    let body = postcard::to_allocvec(&Refusal { reason, version: version.to_owned(), mods: mods.to_vec() }).ok()?;
    body_with_len(&mut packet, &body);
    let tag = crypto::tag(key, &[nonce, &packet]);
    packet.extend_from_slice(&tag);
    Some(packet)
}

// -- the host ---------------------------------------------------------------

/// the host's side: waits for the invited player on its sockets, then
/// carries the session. Runs on the transport's thread.
pub fn serve(sockets: Vec<UdpSocket>, secret: [u8; super::code::SECRET_LEN], end: LinkEnd, peer_mac: Mac, stop: Arc<AtomicBool>, shared: Shared, options: Options) {
    let session = crypto::session_id(&secret);
    let key = crypto::auth_key(&secret);
    for socket in &sockets {
        let _ = socket.set_nonblocking(true);
    }
    let mut buffer = vec![0u8; 2048];
    let mut last_unknown = Instant::now() - Duration::from_secs(1);
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut idle = true;
        for index in 0..sockets.len() {
            let Ok((size, from)) = sockets[index].recv_from(&mut buffer) else { continue };
            idle = false;
            let packet = buffer[..size].to_vec();
            let Some((kind, their_session, rest)) = parse_header(&packet) else { continue };
            if their_session != session {
                // an older code, or another host's: say so, now and then
                if kind == HELLO && last_unknown.elapsed() > Duration::from_millis(200) {
                    last_unknown = Instant::now();
                    let _ = sockets[index].send_to(&header(UNKNOWN, &their_session), from);
                    log::info!("online: {from} tried a code for another session (an old or mistyped code)");
                }
                continue;
            }
            match kind {
                DISCOVER => {
                    let _ = sockets[index].send_to(&header(HERE, &session), from);
                }
                HELLO => {
                    // session, nonce, body, tag
                    let Some(nonce) = rest.get(..16) else { continue };
                    let nonce: [u8; 16] = nonce.try_into().unwrap();
                    let Some((body, tail)) = take_body(&rest[16..]) else { continue };
                    if tail.len() != TAG {
                        continue;
                    }
                    let signed = &packet[..packet.len() - TAG];
                    if !crypto::same(&crypto::tag(&key, &[signed]), tail) {
                        log::info!("online: {from} sent a HELLO that is not from this session's code; ignored");
                        continue;
                    }
                    let Ok(hello) = postcard::from_bytes::<Hello>(body) else { continue };
                    // online2: the gameplay mods must be the same on both
                    let mods = ModsDiff::of(&options.compat.gameplay_mods, &hello.compat.gameplay_mods);
                    let refusal = if hello.protocol != PROTOCOL {
                        Some(RefuseReason::Protocol)
                    } else if hello.version != options.version {
                        Some(RefuseReason::Version)
                    } else if !mods.is_empty() {
                        Some(RefuseReason::Mods)
                    } else {
                        None
                    };
                    if let Some(reason) = refusal {
                        log::info!("online: {from} runs Haiku {} (protocol {}), this is {} (protocol {PROTOCOL}): turned away ({reason:?})", hello.version, hello.protocol, options.version);
                        if reason == RefuseReason::Mods {
                            log::info!("online: gameplay mods only here {:?}, only the joiner's {:?}, different {:?}", mods.ours, mods.theirs, mods.changed);
                        }
                        if let Some(answer) = refuse_packet(&session, &key, reason, &options.version, &options.compat.gameplay_mods, &packet) {
                            let _ = sockets[index].send_to(&answer, from);
                        }
                        super::note_refused(reason, &hello.version, mods);
                        continue;
                    }
                    // welcome: this side's nonce, its version and rules (the
                    // host's choice as it is now; both keep it)
                    let host_nonce: [u8; 16] = crypto::random();
                    let mut answer = header(WELCOME, &session);
                    answer.extend_from_slice(&host_nonce);
                    let rules = super::host_rules();
                    log::info!("online: the rules for this battle: {rules:?}");
                    super::note_rules(rules.clone());
                    let welcome = Welcome { protocol: PROTOCOL, version: options.version.clone(), rules };
                    let Ok(body) = postcard::to_allocvec(&welcome) else { continue };
                    body_with_len(&mut answer, &body);
                    let tag = crypto::tag(&key, &[&nonce, &answer]);
                    answer.extend_from_slice(&tag);
                    let _ = sockets[index].send_to(&answer, from);
                    log::info!("online: {from} joined (Haiku {})", hello.version);
                    let socket = match sockets[index].try_clone() {
                        Ok(socket) => socket,
                        Err(_) => continue,
                    };
                    let _ = socket.set_nonblocking(false);
                    let connection = Connection {
                        socket,
                        peer: from,
                        session,
                        host: true,
                        keys: Keys::new(&secret, &nonce, &host_nonce),
                        counter: 0,
                        replay: Replay::default(),
                        channel: Channel::new(),
                        heard: Instant::now(),
                        pinged: Instant::now(),
                        welcome: Some((answer, nonce)),
                        auth: key,
                    };
                    set(&shared, Progress::Connected { peer_version: hello.version.clone(), rtt_ms: None });
                    // the other sockets are not needed any more
                    let ended = connection.run(&end, peer_mac, &stop, &options, &shared, hello.version);
                    finish(&end, &shared, ended);
                    return;
                }
                _ => {}
            }
        }
        if idle {
            std::thread::sleep(Duration::from_millis(3));
        }
    }
}

fn finish(end: &LinkEnd, shared: &Shared, ended: End) {
    end.close();
    let failure = match ended {
        End::Stopped => None,
        End::PeerLeft => Some(Failure::PeerLeft),
        End::Lost => Some(Failure::Lost),
    };
    set(shared, match failure {
        Some(failure) => Progress::Closed(failure),
        None => Progress::Idle,
    });
}

// -- the joiner -------------------------------------------------------------

/// the joiner's side: reaches the host at the code's addresses (and on its
/// own network), then carries the session. Runs on the transport's thread.
pub fn reach(code: Code, bind: Option<IpAddr>, end: LinkEnd, peer_mac: Mac, stop: Arc<AtomicBool>, shared: Shared, options: Options) {
    let secret = code.secret;
    let session = crypto::session_id(&secret);
    let key = crypto::auth_key(&secret);
    let mut endpoints = code.endpoints();
    let local = bind.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let v4 = match UdpSocket::bind(SocketAddr::new(local, 0)) {
        Ok(socket) => socket,
        Err(error) => {
            log::warn!("online: no socket to reach the host from, {error}");
            set(&shared, Progress::Failed(Failure::Network(error.to_string())));
            end.close();
            return;
        }
    };
    let v6 = endpoints.iter().any(|e| e.is_ipv6()).then(|| UdpSocket::bind("[::]:0").ok()).flatten();
    let _ = v4.set_nonblocking(true);
    if let Some(v6) = &v6 {
        let _ = v6.set_nonblocking(true);
    }
    let lan = options.lan_discovery && !local.is_loopback();
    if lan {
        let _ = v4.set_broadcast(true);
    }
    let nonce: [u8; 16] = crypto::random();
    let hello = Hello { protocol: PROTOCOL, version: options.version.clone(), compat: options.compat.clone() };
    let mut packet = header(HELLO, &session);
    packet.extend_from_slice(&nonce);
    body_with_len(&mut packet, &postcard::to_allocvec(&hello).unwrap_or_default());
    let tag = crypto::tag(&key, &[&packet]);
    packet.extend_from_slice(&tag);
    let discover = header(DISCOVER, &session);
    log::info!("online: reaching the host at {endpoints:?}{}", if lan { " and on this network" } else { "" });

    let started = Instant::now();
    let (mut next_hello, mut next_discover) = (started, started);
    let mut unknown = false;
    let mut buffer = vec![0u8; 2048];
    loop {
        if stop.load(Ordering::Relaxed) {
            end.close();
            set(&shared, Progress::Idle);
            return;
        }
        let now = Instant::now();
        if now.duration_since(started) >= options.connect_timeout {
            let failure = if unknown { Failure::Expired } else { Failure::Unreachable };
            log::info!("online: the host could not be reached ({failure:?})");
            set(&shared, Progress::Failed(failure));
            end.close();
            return;
        }
        if now >= next_hello {
            next_hello = now + HELLO_EVERY;
            for endpoint in &endpoints {
                let socket = if endpoint.is_ipv6() { v6.as_ref() } else { Some(&v4) };
                if let Some(socket) = socket {
                    let _ = socket.send_to(&packet, endpoint);
                }
            }
        }
        if now >= next_discover && options.lan_discovery {
            next_discover = now + DISCOVER_EVERY;
            if lan {
                let _ = v4.send_to(&discover, SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), code.port));
            }
            // this PC: another Haiku here may be the host
            let _ = v4.send_to(&discover, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), code.port));
        }
        let mut idle = true;
        for socket in std::iter::once(&v4).chain(v6.as_ref()) {
            let Ok((size, from)) = socket.recv_from(&mut buffer) else { continue };
            idle = false;
            let reply = buffer[..size].to_vec();
            let Some((kind, their_session, rest)) = parse_header(&reply) else { continue };
            if their_session != session {
                continue;
            }
            match kind {
                HERE => {
                    if !endpoints.contains(&from) {
                        log::info!("online: the host is on this network at {from}");
                        endpoints.insert(0, from);
                        next_hello = now;
                    }
                }
                UNKNOWN => unknown = true,
                REFUSE => {
                    let Some((body, tail)) = take_body(rest) else { continue };
                    let signed = &reply[..reply.len() - tail.len()];
                    if tail.len() != TAG || !crypto::same(&crypto::tag(&key, &[&nonce, signed]), tail) {
                        continue;
                    }
                    // online2: a protocol-1 host's refusal has no mods
                    let refusal = match postcard::from_bytes::<Refusal>(body) {
                        Ok(refusal) => refusal,
                        Err(_) => match postcard::from_bytes::<RefusalV1>(body) {
                            Ok(old) => Refusal { reason: old.reason, version: old.version, mods: Vec::new() },
                            Err(_) => continue,
                        },
                    };
                    log::info!("online: the host turned this side away: {:?} (host Haiku {}, this {})", refusal.reason, refusal.version, options.version);
                    let failure = match refusal.reason {
                        RefuseReason::Version | RefuseReason::Protocol => Failure::Version { host: refusal.version, ours: options.version.clone() },
                        RefuseReason::Busy => Failure::Busy,
                        RefuseReason::Mods => {
                            let diff = ModsDiff::of(&options.compat.gameplay_mods, &refusal.mods);
                            log::info!("online: gameplay mods only here {:?}, only the host's {:?}, different {:?}", diff.ours, diff.theirs, diff.changed);
                            Failure::Mods(diff)
                        }
                    };
                    set(&shared, Progress::Failed(failure));
                    end.close();
                    return;
                }
                WELCOME => {
                    let Some(host_nonce) = rest.get(..16) else { continue };
                    let host_nonce: [u8; 16] = host_nonce.try_into().unwrap();
                    let Some((body, tail)) = take_body(&rest[16..]) else { continue };
                    let signed = &reply[..reply.len() - tail.len()];
                    if tail.len() != TAG || !crypto::same(&crypto::tag(&key, &[&nonce, signed]), tail) {
                        log::info!("online: {from} answered with a WELCOME that is not this session's; ignored");
                        continue;
                    }
                    let Ok(welcome) = postcard::from_bytes::<Welcome>(body) else { continue };
                    log::info!("online: connected to the host at {from} (Haiku {}, rules {:?})", welcome.version, welcome.rules);
                    super::note_rules(welcome.rules.clone());
                    let socket = match socket.try_clone() {
                        Ok(socket) => socket,
                        Err(_) => continue,
                    };
                    let _ = socket.set_nonblocking(false);
                    let connection = Connection {
                        socket,
                        peer: from,
                        session,
                        host: false,
                        keys: Keys::new(&secret, &nonce, &host_nonce),
                        counter: 0,
                        replay: Replay::default(),
                        channel: Channel::new(),
                        heard: Instant::now(),
                        pinged: Instant::now(),
                        welcome: None,
                        auth: key,
                    };
                    set(&shared, Progress::Connected { peer_version: welcome.version.clone(), rtt_ms: None });
                    let ended = connection.run(&end, peer_mac, &stop, &options, &shared, welcome.version);
                    finish(&end, &shared, ended);
                    return;
                }
                _ => {}
            }
        }
        if idle {
            std::thread::sleep(Duration::from_millis(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_delivers_in_order_through_loss_and_reordering() {
        let (mut a, mut b) = (Channel::new(), Channel::new());
        let big = vec![7u8; 5000];
        a.queue(b"one");
        a.queue(&big);
        a.queue(b"three");
        let now = Instant::now();
        let mut frames = a.due(now);
        // lose the second fragment, swap two others
        frames.remove(1);
        frames.swap(0, 2);
        let mut got = Vec::new();
        for frame in &frames {
            let seq = u32::from_le_bytes(frame[1..5].try_into().unwrap());
            got.extend(b.received(seq, frame[5] != 0, &frame[6..]));
        }
        assert!(got.is_empty() || got == vec![b"one".to_vec()]);
        let ack = b.ack_frame();
        a.acknowledged(u32::from_le_bytes(ack[1..5].try_into().unwrap()), u64::from_le_bytes(ack[5..13].try_into().unwrap()), now);
        // the lost one goes again after the timeout
        let again = a.due(now + Duration::from_secs(3));
        assert_eq!(again.len(), 1);
        for frame in &again {
            let seq = u32::from_le_bytes(frame[1..5].try_into().unwrap());
            got.extend(b.received(seq, frame[5] != 0, &frame[6..]));
        }
        assert_eq!(got, vec![b"one".to_vec(), big, b"three".to_vec()]);
    }

    #[test]
    fn the_replay_window_takes_each_counter_once() {
        let mut replay = Replay::default();
        for counter in [5u64, 3, 9, 4] {
            assert!(replay.fresh(counter));
            replay.mark(counter);
        }
        assert!(!replay.fresh(5) && !replay.fresh(9) && !replay.fresh(3));
        assert!(replay.fresh(6));
        replay.mark(200);
        assert!(!replay.fresh(9), "far below the window");
    }
}
