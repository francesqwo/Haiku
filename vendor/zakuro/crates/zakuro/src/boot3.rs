//! boot3: the window at launch (dev/reports/boot3.md).
//!
//! **Borderless fullscreen showed the taskbar after a restart.** qol4 keeps
//! the new window cloaked (DWM) until its first picture, so it is never seen
//! unpainted. The window is shown, activated and made borderless fullscreen
//! (winit marks it full-screen to the taskbar, ITaskbarList2::
//! MarkFullscreenWindow) while it is cloaked: the shell looks at whether the
//! active window is a full-screen one (the taskbar then drops below it) when
//! it is activated or marked, and a cloaked window is nothing it shows.
//! Uncloaked later, nothing told the shell again, so the taskbar stayed over
//! the game until the window mode was put to use again by hand (which
//! marks it again).
//!
//! Now, once qol4 uncloaks the window ([`revealed`]), a full-screen window is
//! marked again (unmarked, then marked: the shell looks at it then, with it
//! shown, active and at its full-screen place), and a window that is not in
//! front is asked to come to the front without forcing it: Windows decides,
//! as for any program's first window (one the user clicked away from in the
//! meantime stays behind, and its taskbar button flashes).
//!
//! The log says where the window is against its monitor, and whether it is
//! the foreground window, at launch (`boot3: launch window ...`).

use winit::window::Window;

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }
    #[repr(C)]
    pub struct MonitorInfo {
        pub size: u32,
        pub monitor: Rect,
        pub work: Rect,
        pub flags: u32,
    }
    #[repr(C)]
    pub struct Guid(u32, u16, u16, [u8; 8]);

    #[link(name = "user32")]
    extern "system" {
        pub fn SetForegroundWindow(hwnd: isize) -> i32;
        pub fn GetForegroundWindow() -> isize;
        pub fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        pub fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
        pub fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, context: u32, iid: *const Guid, out: *mut *mut c_void) -> i32;
    }
    pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const CLSCTX_ALL: u32 = 0x17;
    const COINIT_APARTMENTTHREADED: u32 = 2;
    /// CLSID_TaskbarList
    const TASKBAR_LIST: Guid = Guid(0x56FDF344, 0xFD6D, 0x11D0, [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90]);
    /// IID_ITaskbarList2
    const TASKBAR_LIST2: Guid = Guid(0x602D4995, 0xB13A, 0x429B, [0xA6, 0x6E, 0x19, 0x35, 0xE4, 0x4F, 0x43, 0x17]);

    pub fn hwnd(window: &winit::window::Window) -> Option<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        }
    }

    /// the window's rectangle and its monitor's.
    pub fn rects(hwnd: isize) -> (Rect, Rect) {
        let mut window = Rect::default();
        let mut info = MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, monitor: Rect::default(), work: Rect::default(), flags: 0 };
        // SAFETY: a window of this process, and out-structures of the sizes
        // the calls expect
        unsafe {
            GetWindowRect(hwnd, &mut window);
            GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        }
        (window, info.monitor)
    }

    /// ITaskbarList2::MarkFullscreenWindow(hwnd, false), then (hwnd, true):
    /// the shell looks at the window again. true when the taskbar took it.
    pub fn mark_fullscreen_again(hwnd: isize) -> bool {
        type Release = unsafe extern "system" fn(*mut c_void) -> u32;
        type HrInit = unsafe extern "system" fn(*mut c_void) -> i32;
        type Mark = unsafe extern "system" fn(*mut c_void, isize, i32) -> i32;
        // SAFETY: COM on this (the window's, the event loop's) thread, which
        // winit already initialised (a second initialisation is counted and
        // harmless; another mode is refused and harmless); the interface's
        // vtable laid out as ITaskbarList2 declares it: IUnknown's three,
        // ITaskbarList's five (HrInit first), then MarkFullscreenWindow
        unsafe {
            let _ = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
            let mut list: *mut c_void = std::ptr::null_mut();
            if CoCreateInstance(&TASKBAR_LIST, std::ptr::null_mut(), CLSCTX_ALL, &TASKBAR_LIST2, &mut list) < 0 || list.is_null() {
                return false;
            }
            let vtable = *(list as *const *const usize);
            let release: Release = std::mem::transmute(*vtable.add(2));
            let init: HrInit = std::mem::transmute(*vtable.add(3));
            let mark: Mark = std::mem::transmute(*vtable.add(8));
            let ok = init(list) >= 0 && mark(list, hwnd, 0) >= 0 && mark(list, hwnd, 1) >= 0;
            release(list);
            ok
        }
    }
}

/// where the window is against its monitor, and whether it is in front, for
/// the log.
#[cfg(windows)]
pub fn describe(window: &Window) -> String {
    let Some(hwnd) = win::hwnd(window) else { return "no window handle".into() };
    let (rect, monitor) = win::rects(hwnd);
    let covers = rect.left <= monitor.left && rect.top <= monitor.top && rect.right >= monitor.right && rect.bottom >= monitor.bottom;
    // SAFETY: a plain query
    let front = unsafe { win::GetForegroundWindow() } == hwnd;
    format!(
        "window ({},{})-({},{}), monitor ({},{})-({},{}), {}, {}",
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        monitor.left,
        monitor.top,
        monitor.right,
        monitor.bottom,
        if covers { "covers it" } else { "does not cover it" },
        if front { "in front (foreground)" } else { "not the foreground window" }
    )
}

#[cfg(not(windows))]
pub fn describe(_window: &Window) -> String {
    String::new()
}

/// qol4 uncloaked the launch window (or showed it at once): a full-screen
/// window is marked full-screen again, a window not in front asks to come to
/// the front, and its place is logged.
pub fn revealed(window: &Window) {
    #[cfg(windows)]
    {
        let Some(hwnd) = win::hwnd(window) else { return };
        let fullscreen = window.fullscreen().is_some();
        let marked = fullscreen && win::mark_fullscreen_again(hwnd);
        // SAFETY: plain calls on a window of this process. No input is made
        // up to force it to the front (winit's focus_window would)
        let front = unsafe { win::GetForegroundWindow() == hwnd || win::SetForegroundWindow(hwnd) != 0 };
        log::info!(
            "boot3: launch window shown{}{}: {}",
            if !fullscreen { "" } else if marked { ", marked full-screen to the taskbar again" } else { ", the taskbar could not be told it is full-screen" },
            if front { "" } else { ", Windows keeps another window in front" },
            describe(window)
        );
    }
    #[cfg(not(windows))]
    let _ = window;
}

// ---------------------------------------------------------------------------
// Return to Title, watched
// ---------------------------------------------------------------------------

/// when Return to Title was asked and is still waiting for the game to take
/// it, and whether that was said in the log.
static ASKED: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);

impl crate::App {
    /// after each pass of the frontend (from qol4_poll): a Return to Title
    /// the game has not taken for 10 s is logged once, with a thread dump
    /// (the game allows its soft reset almost everywhere, within a frame or
    /// two; a wait that long means the game itself is stuck). Dev:
    /// FATES_BOOT3_TRACE=1 logs the game's progress every 2 s.
    pub(crate) fn boot3_poll(&mut self) {
        use crate::qol::{read, Word};
        note_repairs();
        let pending = read(Word::SoftReset).unwrap_or(0) != 0;
        {
            let mut asked = ASKED.lock().unwrap();
            match (*asked, pending) {
                (None, true) => *asked = Some((std::time::Instant::now(), false)),
                (Some(_), false) => *asked = None,
                (Some((since, false)), true) if since.elapsed() >= std::time::Duration::from_secs(10) => {
                    *asked = Some((since, true));
                    let frame = self.game.as_ref().map_or(0, |game| game.frame);
                    log::warn!("boot3: Return to Title has waited {:.1} s for the game to allow its soft reset (frame {frame}); a thread dump follows", since.elapsed().as_secs_f32());
                    zakuro_core::pcos::hang::request();
                }
                _ => {}
            }
        }
        static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *TRACE.get_or_init(|| std::env::var("FATES_BOOT3_TRACE").is_ok_and(|v| v == "1")) {
            static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
            let mut last = LAST.lock().unwrap();
            if last.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(2)) {
                *last = Some(std::time::Instant::now());
                if let Some(game) = &self.game {
                    log::info!(
                        "boot3-trace: frame {}, {:.1} fps, soft reset asked {pending}, restarts {}, extra data reads {}",
                        game.frame,
                        game.fps,
                        read(Word::Restarts).unwrap_or(0),
                        read(Word::ExtReadHidden).unwrap_or(0)
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the ProcThread race (overrides/boot3.c)
// ---------------------------------------------------------------------------

/// puts the switch in the game, at each start (from qol4::apply): on in
/// real time, where the race is; off in deterministic runs (replays, the
/// gate), which keep the game's own order. FATES_BOOT3_PROCTHREAD=0 turns it
/// off, =1 on in either mode (tests).
pub fn apply() {
    let guard = match std::env::var("FATES_BOOT3_PROCTHREAD").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        Ok("test") => {
            crate::qol::write(crate::qol::Word::ProcThreadGuard, 2);
            return;
        }
        _ => zakuro_core::pcos::rt::realtime(),
    };
    crate::qol::write(crate::qol::Word::ProcThreadGuard, guard as u32);
}

/// for the end of a headless run.
pub fn summary() -> String {
    use crate::qol::{read, Word};
    let r = |word| read(word).unwrap_or(0);
    format!(
        "; boot3: thread procs' workers held until marked running {} ({} sleeps, {} gave up)",
        r(Word::ProcThreadHeld),
        r(Word::ProcThreadSleeps),
        r(Word::ProcThreadGaveUp)
    )
}

/// in a window: workers held are logged once a session, and any that gave
/// up waiting.
fn note_repairs() {
    use crate::qol::{read, Word};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
    static SAID: AtomicBool = AtomicBool::new(false);
    static GAVE_UP: AtomicU32 = AtomicU32::new(0);
    let held = read(Word::ProcThreadHeld).unwrap_or(0);
    if held > 0 && !SAID.swap(true, Relaxed) {
        log::info!("boot3: a thread proc's worker waited to be marked running before its work ({held} so far), so it can no longer finish first");
    }
    let gave_up = read(Word::ProcThreadGaveUp).unwrap_or(0);
    if gave_up > GAVE_UP.swap(gave_up, Relaxed) {
        log::warn!("boot3: a thread proc's worker was not marked running within a second and went on ({gave_up} so far)");
    }
}

/// dev: FATES_BOTTOM_PROCS in a window too (the proc log headless runs
/// write), from emulate, to see where a windowed boot stops.
pub fn window_procs(memory: &mut zakuro_core::memory::Memory, frame: u64) {
    static LOG: std::sync::Mutex<Option<Option<crate::bottom_scene::ProcLog>>> = std::sync::Mutex::new(None);
    let mut log = LOG.lock().unwrap();
    if let Some(proc_log) = log.get_or_insert_with(crate::bottom_scene::ProcLog::from_env) {
        proc_log.frame(memory, frame);
    }
}
