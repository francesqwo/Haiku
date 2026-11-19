//! oneexe: Options > Interface > Haiku Setup. Haiku Setup is part of
//! Haiku.exe now (haiku-setup, `Haiku.exe --setup`): this row closes the game
//! and opens it, after the game's Yes/No (fates_ui/mods.rs's Confirm), for
//! Rebuild, Change dump or DLC, and the Japanese voices. Its Play starts the
//! game again. Shown in a release install (setup/ beside Haiku.exe).

use std::sync::atomic::{AtomicBool, Ordering};

static ASKING: AtomicBool = AtomicBool::new(false);

/// whether this Haiku.exe has its setup's files beside it (a release
/// install; a development folder has none)
pub fn available() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("setup").join("payload.txt").is_file())).unwrap_or(false)
        || std::env::var("HAIKU_SETUP_ENTRY").is_ok_and(|value| value == "1")
}

/// the Yes/No in front of it is up
pub fn asking() -> bool {
    ASKING.load(Ordering::Relaxed)
}

pub fn set_asking(on: bool) {
    ASKING.store(on, Ordering::Relaxed);
}

/// starts `Haiku.exe --setup`, which waits for this process to close
/// (HAIKU_RESTART_WAIT); false when it can't be started
pub fn open() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut command = std::process::Command::new(&exe);
    command.arg("--setup").env("HAIKU_RESTART_WAIT", std::process::id().to_string());
    if let Ok(folder) = std::env::current_dir() {
        command.current_dir(folder);
    }
    match command.spawn() {
        Ok(child) => {
            log::info!("oneexe: Haiku Setup was chosen in the options: started {} --setup (process {}), closing the game", exe.display(), child.id());
            true
        }
        Err(error) => {
            log::warn!("oneexe: Haiku Setup can't be started: {error}");
            false
        }
    }
}
