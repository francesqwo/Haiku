//! qol4: the user's fifth round of requests (dev/reports/qol4.md).
//!
//! - **Options in the Main Menu.** overrides/mods.c adds an "Options" entry
//!   between Mods and Exit to Desktop; its A sets `qol_title_options_request`
//!   and the options menu opens over the Main Menu.
//! - **The game's own soft reset.** On the console L+R+Start restarts the
//!   game (HomeButton::Tick -> nn::applet RestartApplication). On PC that
//!   request had no answer; overrides/qol.c now tells the host
//!   (`qol_restart_request`), and the host starts the game again in the same
//!   window, as the console starts it again ([`App::qol4_restart`]). Return
//!   to Title (the map's menu, the options' Gameplay tab) asks for it with
//!   `qol_soft_reset`, which the game's next Tick that allows a soft reset
//!   takes as L+R+Start.
//! - **Return to Title and Exit to Desktop in the map's menu**
//!   (`qol_map_items`, overrides/qol.c).
//! - **No "Loading extra data..." at each start** (`qol_ext_read_quiet`).
//!
//! Switches for tests: `FATES_QOL_TITLE_OPTIONS=0` leaves the Main Menu's
//! Options out, `FATES_QOL_MAP=0` the map menu's items, and
//! `FATES_QOL_EXT_READ_MSG=1` shows the extra data message again.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Instant;

use crate::qol::{read, write, Word};

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| !matches!(value.as_str(), "0" | "off" | "false" | ""))
}

/// puts the switches in the game, at start (from qol::apply).
pub fn apply() {
    write(Word::TitleOptions, env_flag("FATES_QOL_TITLE_OPTIONS").unwrap_or(true) as u32);
    write(Word::MapItems, env_flag("FATES_QOL_MAP").unwrap_or(true) as u32);
    write(Word::ExtReadQuiet, !env_flag("FATES_QOL_EXT_READ_MSG").unwrap_or(false) as u32);
    crate::boot3::apply(); // boot3: the ProcThread race repaired
}

/// the counts for the end of a headless run, after qol's.
pub fn summary() -> String {
    let r = |word| read(word).unwrap_or(0);
    format!("; qol4: soft resets {}, extra data message left out {}", r(Word::Restarts), r(Word::ExtReadHidden)) + &crate::boot3::summary() // boot3
}

/// the game asked to be started again (its soft reset).
pub fn restart_due() -> bool {
    read(Word::RestartRequest).unwrap_or(0) != 0
}

/// headless: run_headless goes on from the frame the game had when it was
/// started again, rather than loading FATES_LOAD's state over it.
static RESUMING: AtomicBool = AtomicBool::new(false);

pub fn take_resuming() -> bool {
    RESUMING.swap(false, Relaxed)
}

/// headless tests: FATES_QOL4_RESET_AT=n asks for Return to Title at
/// frame n, as the options' row does. boot3: `every:N` asks again N frames
/// after each soft reset the game took (and after the start), headless or
/// in a window, for the many-resets stress runs.
pub fn headless_frame(frame: u64) {
    use std::sync::atomic::AtomicU64;
    #[derive(Clone, Copy)]
    enum At {
        Once(u64),
        Every(u64),
    }
    static AT: std::sync::OnceLock<Option<At>> = std::sync::OnceLock::new();
    let at = *AT.get_or_init(|| {
        let value = std::env::var("FATES_QOL4_RESET_AT").ok()?;
        match value.strip_prefix("every:") {
            Some(every) => every.parse().ok().filter(|&n| n > 0).map(At::Every),
            None => value.parse().ok().map(At::Once),
        }
    });
    let due = match at {
        Some(At::Once(at)) => at == frame,
        Some(At::Every(every)) => {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            static SEEN: AtomicU64 = AtomicU64::new(0);
            let restarts = read(Word::Restarts).unwrap_or(0) as u64;
            if SEEN.swap(restarts, Relaxed) != restarts || NEXT.load(Relaxed) == 0 {
                NEXT.store(frame + every, Relaxed);
            }
            let due = frame >= NEXT.load(Relaxed);
            if due {
                // once until the game takes it
                NEXT.store(u64::MAX, Relaxed);
            }
            due
        }
        None => false,
    };
    if due {
        println!("qol4: frame {frame} Return to Title asked (FATES_QOL4_RESET_AT)");
        return_to_title("a test");
    }
}

/// asks the game for its soft reset: Return to Title.
pub fn return_to_title(from: &str) {
    log::info!("qol4: Return to Title was chosen in {from}: the game's soft reset at its next chance");
    write(Word::SoftReset, 1);
}

impl crate::App {
    /// after each emulated frame (from qol_poll): the Main Menu's Options,
    /// and the game's soft reset.
    pub(crate) fn qol4_poll(&mut self) {
        if read(Word::TitleOptionsRequest).unwrap_or(0) != 0 {
            write(Word::TitleOptionsRequest, 0);
            log::info!("qol4: Options was chosen in the Main Menu");
            self.fates.open_options = true;
        }
        if restart_due() && self.options.headless.is_none() {
            self.qol4_restart();
        }
        self.boot3_poll(); // boot3: a Return to Title the game never takes is logged, with a dump
    }

    /// the game's soft reset on PC: the game is started again from its first
    /// frame, in the same window, as the console starts it again. The
    /// process, the window, the sound, the settings, the mods and their
    /// texture packs stay (none of them changed; the mods page's restart is
    /// the one that starts a new process). A replay or recording goes on
    /// where it was, its frames counted on.
    pub(crate) fn qol4_restart(&mut self) {
        write(Word::RestartRequest, 0);
        write(Word::SoftReset, 0);
        let Some(game) = self.game.as_mut() else { return };
        let path = game.path.clone();
        let frame = game.frame;
        let replay = game.replay.take();
        let recorder = game.recorder.take();
        log::info!("qol4: frame {frame}: the game's soft reset: starting it again");
        let started = Instant::now();
        self.fates.options.hide();
        self.fates.swallow = true;
        // as the mods page's in-process restart: the native renderer leaves
        // out the SDK's set-up before the new boot's first frame, and the
        // overrides' tails waiting in the old boot's threads go
        zakuro_core::native_gfx::lease_boot(120);
        zakuro_core::recompiled::forget_tails(0);
        // a recording would start over in its file (it is carried on
        // instead), and a saved state (FATES_LOAD, tests) would be loaded
        // over the new boot: neither is seen by the new start
        let hidden: Vec<(&str, std::ffi::OsString)> = [("ZAKURO_RECORD", recorder.is_some()), ("FATES_LOAD", true)]
            .into_iter()
            .filter_map(|(name, hide)| Some((name, std::env::var_os(name).filter(|_| hide)?)))
            .collect();
        for (name, _) in &hidden {
            std::env::remove_var(name);
        }
        let result = self.play(&path);
        for (name, value) in hidden {
            std::env::set_var(name, value);
        }
        self.qol = Default::default();
        match result {
            Ok(()) => {
                if let Some(game) = &mut self.game {
                    game.frame = frame;
                    if replay.is_some() {
                        game.replay = replay;
                    }
                    if recorder.is_some() {
                        game.recorder = recorder;
                    }
                }
                log::info!("qol4: the game started again in {} ms", started.elapsed().as_millis());
            }
            Err(error) => self.menus.message = Some(error),
        }
    }

    /// headless: the soft reset between two stretches of run_headless.
    pub(crate) fn qol4_restart_headless(&mut self) {
        let frame = self.game.as_ref().map_or(0, |game| game.frame);
        println!("qol4: frame {frame} the game asked for its soft reset: started again");
        self.qol4_restart();
        RESUMING.store(true, Relaxed);
    }
}

// ---------------------------------------------------------------------------
// the window at launch
// ---------------------------------------------------------------------------

/// whether the window is kept from view until its first picture (on
/// Windows, by DWM cloaking), rather than shown at once, unpainted (white),
/// as before: FATES_SHOW_EARLY=1 has the old way, for comparing.
pub fn start_hidden() -> bool {
    !env_flag("FATES_SHOW_EARLY").unwrap_or(false)
}

static REVEALED: AtomicBool = AtomicBool::new(false);
static CLOAKED: AtomicBool = AtomicBool::new(false);
static PRESENTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static CREATED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

#[cfg(windows)]
mod dwm {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    /// DWMWA_CLOAK
    const CLOAK: u32 = 13;

    /// cloaks or uncloaks the window: a cloaked window is shown as far as
    /// Windows and the presenter know (its pictures are presented as
    /// usual), but the desktop composes none of it. true when it took.
    pub fn cloak(window: &winit::window::Window, on: bool) -> bool {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return false };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else { return false };
        let value: i32 = on as i32;
        // SAFETY: a window this process made, and a 4-byte BOOL attribute
        unsafe { DwmSetWindowAttribute(handle.hwnd.get(), CLOAK, (&value as *const i32).cast(), 4) >= 0 }
    }
}

#[cfg(not(windows))]
mod dwm {
    pub fn cloak(_window: &winit::window::Window, _on: bool) -> bool {
        false
    }
}

/// the window was made hidden (with_visible(!start_hidden())): it is shown,
/// cloaked, so the renderer presents to it as to any shown window, while
/// nothing of it is on the desktop until reveal. Without cloaking (another
/// system) it is shown at once, as before.
pub fn window_created(window: &winit::window::Window) {
    let _ = CREATED.set(Instant::now());
    if !start_hidden() {
        return;
    }
    if dwm::cloak(window, true) {
        CLOAKED.store(true, Relaxed);
    }
    window.set_visible(true);
}

/// after each present: a cloaked window is uncloaked once a picture was
/// presented in it (or after a few tries, so it is never left unseen).
/// Before, Windows showed the new window unpainted, white, from its
/// creation until the game's first picture: the renderer's start and the
/// game's loading.
pub fn reveal(window: &winit::window::Window, presented: bool) {
    if REVEALED.load(Relaxed) {
        return;
    }
    // trailer: FATES_STAY_CLOAKED=1 (dev) keeps a capture run's window off the desktop
    if crate::trailer_dump::stay_cloaked() && CLOAKED.load(Relaxed) {
        return;
    }
    let tries = PRESENTS.fetch_add(1, Relaxed) + 1;
    if !presented && tries < 30 {
        return;
    }
    REVEALED.store(true, Relaxed);
    let since = CREATED.get().map_or(0, |at| at.elapsed().as_millis());
    let presented = if presented { "" } else { ", none presented" };
    if CLOAKED.load(Relaxed) {
        dwm::cloak(window, false);
        log::info!("qol4: the window is shown after its first picture ({tries} presents, {since} ms after it was made{presented})");
    } else {
        log::info!("qol4: the window was shown at once, unpainted until its first picture {since} ms after it was made ({tries} presents{presented})");
    }
    crate::boot3::revealed(window); // boot3: the taskbar told again, the window to the front
}

impl crate::App {
    /// a key press or release: the options menu's key (settings.keys.options,
    /// Escape unless rebound) opens and closes the options and never reaches
    /// the game. Escape keeps its own path (the interface reads it, and it is
    /// the menus' Back); another key bound here is told to the interface.
    /// true when the key was the options menu's.
    pub(crate) fn qol4_options_key(&mut self, code: winit::keyboard::KeyCode, pressed: bool, repeat: bool) -> bool {
        if !crate::fates_ui::enabled() || self.game.is_none() || code == winit::keyboard::KeyCode::Escape || !self.settings.keys.options.contains(code) {
            return false;
        }
        if pressed && !repeat {
            self.fates.options_key = true;
        }
        true
    }
}
