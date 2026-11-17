//! qol3: no console window. Haiku.exe is a Windows (GUI) program
//! (`windows_subsystem = "windows"` in main.rs), so starting it from
//! Explorer, a shortcut or Start.cmd opens no console beside the game.
//!
//! Output still goes where it is sent:
//!
//! - **Redirected** (`./Haiku.exe --headless ... > log 2>&1` from bash, a
//!   pipe, a file): Windows hands a GUI program the handles its parent
//!   redirected, and Rust's stdout and stderr write to whatever
//!   `GetStdHandle` gives at each write. Those handles are kept as they are.
//! - **Started from a console without redirection** (cmd, PowerShell,
//!   Windows Terminal): the program attaches to its parent's console
//!   (`AttachConsole(ATTACH_PARENT_PROCESS)`) and binds the standard handles
//!   that were not redirected to it (`CONOUT$`, `CONIN$`), so `--help`,
//!   `--install` and headless runs print there. (A console does not wait
//!   for a GUI program, so the prompt comes back before the output ends.)
//! - **`--headless` with no console and no redirection** (started from a
//!   shortcut, say): a console of its own is opened, so a test run is never
//!   silent.
//!
//! haiku.log (report.rs) is a file and is written in every case.

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    pub const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    pub const FILE_TYPE_UNKNOWN: u32 = 0;
    pub const FILE_TYPE_CHAR: u32 = 2;
    pub const GENERIC_READ: u32 = 0x8000_0000;
    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    pub const FILE_SHARE_READ: u32 = 1;
    pub const FILE_SHARE_WRITE: u32 = 2;
    pub const OPEN_EXISTING: u32 = 3;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn AttachConsole(process: u32) -> i32;
        pub fn AllocConsole() -> i32;
        pub fn GetStdHandle(which: u32) -> Handle;
        pub fn SetStdHandle(which: u32, handle: Handle) -> i32;
        pub fn GetFileType(handle: Handle) -> u32;
        pub fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
        pub fn OpenProcess(access: u32, inherit: i32, id: u32) -> Handle;
        pub fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
        pub fn CloseHandle(handle: Handle) -> i32;
    }
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
}

/// whether a standard handle goes nowhere a GUI program can write: none,
/// or a console it is not attached to. A file or a pipe was redirected.
#[cfg(windows)]
fn unbound(which: u32) -> bool {
    use win::*;
    // SAFETY: plain Win32 calls on the process's own standard handles
    unsafe {
        let handle = GetStdHandle(which);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return true;
        }
        matches!(GetFileType(handle), FILE_TYPE_UNKNOWN | FILE_TYPE_CHAR)
    }
}

/// binds a standard handle to the console now attached.
#[cfg(windows)]
fn bind(which: u32, device: &str, access: u32) {
    use win::*;
    let name: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated name; the handle is the process's from now on
    unsafe {
        let handle = CreateFileW(name.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null_mut(), OPEN_EXISTING, 0, std::ptr::null_mut());
        if handle != INVALID_HANDLE_VALUE {
            SetStdHandle(which, handle);
        }
    }
}

/// called first thing in main, before anything is printed.
#[cfg(windows)]
pub fn attach() {
    use win::*;
    let (input, output, error) = (unbound(STD_INPUT_HANDLE), unbound(STD_OUTPUT_HANDLE), unbound(STD_ERROR_HANDLE));
    if !output && !error {
        // both redirected (the gate scripts, `> log 2>&1`): nothing to do
        return;
    }
    // SAFETY: plain Win32 calls
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0
        || (std::env::args().any(|arg| arg == "--headless") && unsafe { AllocConsole() } != 0);
    if !attached {
        // started from Explorer or a shortcut: no console, as wanted
        return;
    }
    if output {
        bind(STD_OUTPUT_HANDLE, "CONOUT$", GENERIC_READ | GENERIC_WRITE);
    }
    if error {
        bind(STD_ERROR_HANDLE, "CONOUT$", GENERIC_READ | GENERIC_WRITE);
    }
    if input {
        bind(STD_INPUT_HANDLE, "CONIN$", GENERIC_READ | GENERIC_WRITE);
    }
}

#[cfg(not(windows))]
pub fn attach() {}

/// qol3: a Haiku.exe started by the Mods screen's restart
/// (fates_ui/mods.rs, fates_relaunch) waits for the one that started it to
/// be gone (HAIKU_RESTART_WAIT, its process id; at most 15 s), so the two
/// never have the window, the sound, the log or the saves at once.
pub fn wait_for_restart() {
    let Some(id) = std::env::var("HAIKU_RESTART_WAIT").ok().and_then(|id| id.parse::<u32>().ok()) else { return };
    std::env::remove_var("HAIKU_RESTART_WAIT");
    #[cfg(windows)]
    {
        use win::*;
        let started = std::time::Instant::now();
        // SAFETY: a handle of our own, closed once waited on
        unsafe {
            let process = OpenProcess(SYNCHRONIZE, 0, id);
            if !process.is_null() {
                WaitForSingleObject(process, 15_000);
                CloseHandle(process);
            }
        }
        eprintln!("haiku: restarted, process {id} closed after {} ms", started.elapsed().as_millis());
    }
    #[cfg(not(windows))]
    let _ = id;
}
