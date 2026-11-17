//! online: what the player sees of an online battle, over the game's own
//! Local Battle screens (zakuro_core::netplay carries the battle).
//!
//! - **Host**: once the game's network is up, the code shows in the game's
//!   checkered message box at the top of the window, with Copy (a click,
//!   Ctrl+C, or the controller's X). It goes when the other player joins or
//!   the host leaves the wait.
//! - **Join**: when the game's list of networks opens, the game's own
//!   name-entry keyboard opens for the code (typing, pasting with Ctrl+V,
//!   the pointer and a controller all work). A mistyped code is caught at
//!   once; then "Connecting to the host…" shows until the host answers,
//!   and the host's network appears in the game's list. Cancel closes the
//!   box and backs out of the list (B to the game).
//! - **Problems**: a code that reaches no one, an expired one, a different
//!   version of Haiku, or a host already in a battle are said in the game's
//!   message box; the code box comes back after it. A battle the other
//!   player left, or a connection lost, is said in a notice while the game
//!   shows its own communication error.
//!
//! Headless tests drive the same paths: FATES_ONLINE_CODE_IN=file joins
//! with the code a host wrote with FATES_ONLINE_CODE_OUT, and
//! FATES_LIVE_INPUT=file takes button presses appended to a file while the
//! game runs (see `Live`).

use std::sync::Mutex;
use std::time::Instant;

use zakuro_core::netplay::{self, Failure, ModsDiff, Notice, Progress, RefuseReason, Role};
use zakuro_core::services::hid::{InputState, PadState};
use zakuro_core::services::keyboard::{Request, Valid};

/// what the frontend keeps of the online screens.
#[derive(Default)]
struct State {
    /// the code typed so far, kept when the box closes for a message.
    typed: String,
    /// the line the code box says under its letters.
    help: Option<String>,
    /// the game's B, pressed for a few frames to leave the list.
    press_b: u8,
    /// when the code was copied, for the button's "Copied".
    copied: Option<Instant>,
    /// a message for the game's message box.
    message: Option<String>,
    /// online2: the last message given, for the UI snapshots of a run.
    last_message: Option<String>,
    /// a moment's notice.
    toast: Option<String>,
    /// the box is open (the game gets no buttons meanwhile).
    typing: bool,
    /// the settings last given to netplay.
    configured: Option<netplay::Config>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// the player's online settings, for the transport.
pub fn configure(settings: &crate::settings::OnlineSettings) {
    let config = netplay::Config {
        port: settings.port,
        stun: settings.stun_server.clone(),
        upnp: settings.upnp,
        natpmp: settings.nat_pmp,
        ipv6: settings.ipv6,
        lan_discovery: settings.lan_discovery,
        timeout_secs: settings.timeout_secs,
        bind: None,
    };
    with(|state| {
        if state.configured.as_ref() != Some(&config) {
            state.configured = Some(config.clone());
            netplay::configure(config);
        }
    });
    // online2: the host's rule for its next battle
    netplay::set_host_rules(netplay::Rules { allow_cheated_units: settings.allow_cheated_units });
}

/// online2: the text the game's message window shows for a team refused
/// (overrides/online.c has it in French and Spanish too).
pub const CHEATS_NOT_ALLOWED: &str = "Characters modified with cheats are not allowed in this battle.";

/// online2: names, joined for a sentence.
fn names(list: &[String]) -> String {
    list.join(", ")
}

/// online2: what differs between two players' gameplay mods, from this side.
pub fn mods_text(diff: &ModsDiff, other: &str) -> String {
    let mut parts = Vec::new();
    if !diff.ours.is_empty() {
        parts.push(format!("Only you have: {}.", names(&diff.ours)));
    }
    if !diff.theirs.is_empty() {
        parts.push(format!("Only {other} has: {}.", names(&diff.theirs)));
    }
    if !diff.changed.is_empty() {
        parts.push(format!("Different versions: {}.", names(&diff.changed)));
    }
    parts.join(" ")
}

/// what a failure says to the player.
pub fn failure_text(failure: &Failure) -> String {
    match failure {
        Failure::Unreachable => "The host couldn't be reached. Check the code with the host; it may have expired, or the host's network may not let other players in.".into(),
        Failure::Expired => "That code has expired. Ask the host for the new code.".into(),
        Failure::Version { host, ours } => format!("The host is playing a different version of Haiku ({host}; yours is {ours}). Both players need the same version."),
        Failure::Busy => "The host is already battling another player.".into(),
        Failure::Mods(diff) => format!("The host's gameplay mods don't match yours. Both players need the same gameplay mods. {}", mods_text(diff, "the host")),
        Failure::PeerLeft => "The other player left the battle.".into(),
        Failure::Lost => "The connection to the other player was lost.".into(),
        Failure::Network(error) => format!("Online battles couldn't start: {error}."),
    }
}

/// each frame: the session's news turned into what the player sees.
pub fn poll() {
    netplay::poll();
    // the cheats' switches are off while the online Local Battle is up
    static BATTLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let battle = netplay::battle();
    if BATTLE.swap(battle, std::sync::atomic::Ordering::Relaxed) != battle {
        crate::cheats::online_changed(battle);
    }
    let notice = netplay::take_notice();
    with(|state| {
        match notice {
            Some(Notice::Failed(failure @ (Failure::PeerLeft | Failure::Lost))) => state.toast = Some(failure_text(&failure)),
            Some(Notice::Failed(failure)) => {
                // the box comes back after the message, saying it again
                state.help = Some(match &failure {
                    Failure::Expired => "That code has expired. Ask the host for the new one.".into(),
                    Failure::Version { .. } => "The host has another version of Haiku.".into(),
                    Failure::Busy => "The host is already battling.".into(),
                    Failure::Mods(_) => "The host's gameplay mods don't match yours.".into(),
                    _ => "The host couldn't be reached with that code.".into(),
                });
                state.message = Some(failure_text(&failure));
            }
            Some(Notice::Refused(reason, version)) => {
                state.toast = Some(match reason {
                    RefuseReason::Version | RefuseReason::Protocol => format!("A player with another version of Haiku ({version}) tried to join."),
                    RefuseReason::Busy => "Another player tried to join.".into(),
                    RefuseReason::Mods => "A player with other gameplay mods tried to join.".into(),
                })
            }
            // online2: the host sees which mods differed
            Some(Notice::RefusedMods(diff)) => state.toast = Some(format!("A player with other gameplay mods tried to join. {}", mods_text(&diff, "they"))),
            // online2: a team with a character changed by cheats arrived
            Some(Notice::CheatedTeams(sides)) => {
                let whose = match sides {
                    1 => "Your team has one, so the battle can't start.",
                    2 => "Your opponent's team has one, so the battle can't start.",
                    _ => "Both teams have one, so the battle can't start.",
                };
                state.message = Some(format!("{CHEATS_NOT_ALLOWED} {whose}"));
            }
            Some(Notice::Connected(_)) | None => {}
        }
    });
    test_join();
}

/// a message for the game's message box, once.
pub fn take_message() -> Option<String> {
    with(|state| {
        let message = state.message.take();
        if message.is_some() {
            state.last_message = message.clone();
        }
        message
    })
}

/// online2: the last message this run gave (or has waiting), for the UI
/// snapshots (FATES_UI_SNAPSHOT) of a headless test.
pub fn last_message() -> Option<String> {
    with(|state| state.message.clone().or_else(|| state.last_message.clone()))
}

/// a moment's notice, once.
pub fn take_toast() -> Option<String> {
    with(|state| state.toast.take())
}

/// the persistent line while connecting or preparing.
pub fn status_line() -> Option<String> {
    match netplay::progress() {
        Progress::Preparing => Some("Opening the connection…".into()),
        Progress::Connecting => Some("Connecting to the host…".into()),
        _ => None,
    }
}

/// the host's code panel: the code, whether the router opened the port,
/// and when it was copied; online2: the host's rule.
#[derive(Clone, Debug)]
pub struct HostPanel {
    pub code: String,
    pub mapped: bool,
    pub copied: bool,
    /// online2: characters modified with cheats are allowed.
    pub allow_cheats: bool,
}

pub fn host_panel() -> Option<HostPanel> {
    match netplay::progress() {
        Progress::Hosting { code, mapped } if netplay::role() == Some(Role::Host) => {
            let copied = with(|state| state.copied.is_some_and(|at| at.elapsed().as_secs_f32() < 2.0));
            Some(HostPanel { code, mapped, copied, allow_cheats: netplay::host_rules().allow_cheated_units })
        }
        _ => None,
    }
}

/// the host's Copy: the frontend put the code on the clipboard.
pub fn mark_copied() {
    log::info!("online: the code is copied");
    with(|state| state.copied = Some(Instant::now()));
}

/// the code box, when the game's list is open and no message is up.
pub fn keyboard_request(message_up: bool) -> Option<Request> {
    let wanted = netplay::wants_code() && netplay::battle() && netplay::role().is_none();
    with(|state| {
        state.typing = wanted && state.message.is_none() && !message_up;
        state.typing.then(|| sample_request(&state.typed))
    })
}

/// the line under the code box's letters.
pub fn keyboard_help() -> Option<String> {
    with(|state| Some(state.help.clone().unwrap_or_else(|| "Type or paste the code the host sent you.".into())))
}

/// whether the code box is open (the game waits for it).
pub fn typing() -> bool {
    with(|state| state.typing)
}

/// the code box closed: Join with the text, or Cancel.
pub fn answer(text: &str, button: usize) {
    with(|state| state.typed = text.to_owned());
    if button == 0 {
        log::info!("online: the code box was cancelled; back out of the game's list");
        netplay::cancel_code();
        with(|state| {
            state.typing = false;
            state.help = None;
            state.press_b = 6;
        });
        return;
    }
    match netplay::join(text) {
        Ok(()) => with(|state| {
            state.typing = false;
            state.help = None;
        }),
        Err(error) => {
            log::info!("online: the code {text:?} was not taken: {error:?}");
            let help = match error {
                netplay::CodeError::Character(c) => format!("'{c}' isn't in any code. Check it and try again."),
                _ => "That code isn't right. Check it and try again.".to_owned(),
            };
            with(|state| state.help = Some(help));
        }
    }
}

/// the game's input this frame: nothing while the code box is open, and
/// the B that backs out of the list after Cancel.
pub fn apply(input: &mut InputState) {
    with(|state| {
        if state.typing {
            *input = InputState { tilt: input.tilt, ..InputState::default() };
        }
        if state.press_b > 0 {
            state.press_b -= 1;
            if state.press_b >= 2 {
                input.buttons |= PadState::B;
            }
        }
    });
}

/// test only: FATES_ONLINE_CODE_IN=file: the code box takes the code a
/// host wrote there (FATES_ONLINE_CODE_OUT), as the player would type it.
pub fn test_join() {
    let Some(path) = std::env::var_os("FATES_ONLINE_CODE_IN") else { return };
    if !(netplay::wants_code() && netplay::battle() && netplay::role().is_none()) {
        return;
    }
    if let Ok(code) = std::fs::read_to_string(&path) {
        let code = code.trim();
        // a code the box already turned down is not typed again
        if !code.is_empty() && with(|state| !(state.help.is_some() && state.typed == code)) {
            println!("online: the test types the code {code}");
            answer(code, 1);
        }
    }
}

/// test only: FATES_LIVE_INPUT=file: commands appended to a file while
/// the game runs headless, one a line:
///
/// - `press A` / `press UP 3` (a button, held 6 frames, then 10 free, n
///   times; A B X Y L R START SELECT UP DOWN LEFT RIGHT, joined with +)
/// - `wait 120` (frames)
/// - `shot name` (both screens to FATES_LIVE_SHOTS/name-top.png, -bottom.png)
/// - `touch 160 120` (a tap of the lower screen)
pub struct Live {
    path: std::path::PathBuf,
    read: usize,
    queue: std::collections::VecDeque<String>,
    held: Option<(PadState, Option<(u16, u16)>, u32)>,
    gap: u32,
    wait: u32,
}

impl Live {
    pub fn from_env() -> Option<Live> {
        let path = std::path::PathBuf::from(std::env::var_os("FATES_LIVE_INPUT")?);
        println!("online: live input from {}", path.display());
        Some(Live { path, read: 0, queue: Default::default(), held: None, gap: 0, wait: 0 })
    }

    fn button(name: &str) -> PadState {
        name.split('+').fold(PadState::empty(), |all, part| {
            all | match part.trim().to_ascii_uppercase().as_str() {
                "A" => PadState::A,
                "B" => PadState::B,
                "X" => PadState::X,
                "Y" => PadState::Y,
                "L" => PadState::L,
                "R" => PadState::R,
                "START" => PadState::START,
                "SELECT" => PadState::SELECT,
                "UP" => PadState::UP,
                "DOWN" => PadState::DOWN,
                "LEFT" => PadState::LEFT,
                "RIGHT" => PadState::RIGHT,
                _ => PadState::empty(),
            }
        })
    }

    /// the input for this frame, and a picture asked for.
    pub fn frame(&mut self, system: &mut zakuro_core::System, frame: u64) -> InputState {
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            if text.len() > self.read {
                let fresh = &text[self.read..];
                // whole lines only
                if let Some(end) = fresh.rfind('\n') {
                    for line in fresh[..end].lines() {
                        let line = line.trim();
                        if !line.is_empty() && !line.starts_with('#') {
                            self.queue.push_back(line.to_owned());
                        }
                    }
                    self.read += end + 1;
                }
            }
        }
        let mut input = InputState::default();
        if let Some((buttons, touch, left)) = self.held.as_mut() {
            input.buttons = *buttons;
            input.touch = *touch;
            *left -= 1;
            if *left == 0 {
                self.held = None;
                self.gap = 10;
            }
            return input;
        }
        if self.gap > 0 {
            self.gap -= 1;
            return input;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return input;
        }
        let Some(command) = self.queue.pop_front() else { return input };
        println!("online: live frame {frame}: {command} ({:?})", netplay::progress());
        let mut words = command.split_whitespace();
        match words.next().unwrap_or("") {
            "press" => {
                let buttons = Self::button(words.next().unwrap_or(""));
                let times: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                for _ in 1..times {
                    self.queue.push_front(format!("press {}", command.split_whitespace().nth(1).unwrap_or("")));
                }
                self.held = Some((buttons, None, 6));
            }
            "hold" => {
                let buttons = Self::button(words.next().unwrap_or(""));
                let frames: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(30);
                self.held = Some((buttons, None, frames.max(1)));
            }
            "touch" => {
                let x = words.next().and_then(|n| n.parse().ok()).unwrap_or(160);
                let y = words.next().and_then(|n| n.parse().ok()).unwrap_or(120);
                self.held = Some((PadState::empty(), Some((x, y)), 6));
            }
            "wait" => self.wait = words.next().and_then(|n| n.parse().ok()).unwrap_or(60),
            "shot" => {
                let name = words.next().unwrap_or("shot");
                let dir = std::path::PathBuf::from(std::env::var_os("FATES_LIVE_SHOTS").unwrap_or_else(|| ".".into()));
                let _ = std::fs::create_dir_all(&dir);
                for (screen, suffix) in [(zakuro_common::Screen::Top, "top"), (zakuro_common::Screen::Bottom, "bottom")] {
                    let (pixels, width, height) = system.read_screen_scaled(screen);
                    let path = dir.join(format!("{name}-{suffix}.png"));
                    if let Err(error) = image::save_buffer(&path, &pixels, width, height, image::ColorType::Rgba8) {
                        eprintln!("could not write {}: {error}", path.display());
                    }
                }
                println!("online: live frame {frame}: shot {name}");
            }
            other => println!("online: live: unknown command {other}"),
        }
        input
    }
}

/// the code box's request, with some text typed (for the UI snapshots).
pub fn sample_request(text: &str) -> Request {
    Request {
        hint: "Enter the host's code".into(),
        text: text.into(),
        max_length: 72,
        buttons: vec!["Cancel".into(), "Join".into()],
        valid: Valid::NotEmptyNotBlank,
    }
}
