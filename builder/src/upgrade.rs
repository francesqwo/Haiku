//! oneexe: a Haiku 1.x install, extracted over by this release.
//!
//! Haiku 1.0 and 1.1 shipped `Haiku Setup.exe`, which linked a whole
//! Haiku.exe (engine and game code) from setup/engine and setup/overrides.
//! This release ships Haiku.exe built, and builds only the game's code
//! (data/code/game.dll). Extracting it over a 1.x folder replaces Haiku.exe,
//! SDL3.dll and setup/'s shared files, and leaves:
//!
//! - `Haiku Setup.exe`: removed. Run, it would link a 1.x Haiku.exe from the
//!   1.x engine left in setup/engine and put it over this one. Haiku.exe
//!   does everything it did (the setup window: Options, Interface, Haiku Setup, or
//!   `Haiku.exe --setup`);
//! - setup/engine, setup/overrides, setup/eur, setup/jpn, setup/SDL3.dll:
//!   1.x's payload for that link, removed (the maps are in Haiku.exe now,
//!   and SDL3.dll is beside it);
//! - setup-work/overrides, setup-work/derived, setup-work/link,
//!   setup-work/code, setup-work/regenerated, setup-work/link.rsp: 1.x's
//!   intermediate files, removed;
//! - `Haiku Setup.log`: moved to setup-work/setup-1.x.log;
//! - data/, assets/, mods/, cache/, userdata/haiku/mods.toml and
//!   setup-work's state: moved into games/<version>/ (layout.rs), the
//!   folder of the version installed there, each a rename.
//!
//! Kept as they are (moved, not changed): data/ (the installed game and DLC,
//! voices included), assets/ (made again only when this release's recipes
//! differ), setup-work/state.txt (whose install keys this release reads, so
//! the dump isn't checked or installed again), mods/ and mods.toml, cache/;
//! userdata/ (saves, settings) stays where it is. Nothing here changes a
//! save, a setting or a mod.

use std::path::Path;

use crate::log;

/// tidies a 1.x install; what it did, for the log (empty when there was
/// nothing). Errors (a file in use) are left for the next start.
pub fn tidy(root: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let setup = root.join("setup");
    // only a folder this release was extracted into: setup/ holds the new
    // payload and Haiku.exe is beside it
    if !setup.join("payload.txt").is_file() {
        return done;
    }
    let old_setup = root.join("Haiku Setup.exe");
    if old_setup.is_file() {
        match std::fs::remove_file(&old_setup) {
            Ok(()) => done.push("removed Haiku Setup.exe (1.x): Haiku.exe sets itself up now".to_owned()),
            Err(e) => done.push(format!("Haiku Setup.exe (1.x) can't be removed yet ({e})")),
        }
    }
    for dir in ["engine", "overrides", "eur", "jpn"] {
        let path = setup.join(dir);
        if path.is_dir() {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => done.push(format!("removed setup/{dir} (1.x)")),
                Err(e) => done.push(format!("setup/{dir} (1.x) can't be removed yet ({e})")),
            }
        }
    }
    if setup.join("SDL3.dll").is_file() && root.join("SDL3.dll").is_file() {
        if std::fs::remove_file(setup.join("SDL3.dll")).is_ok() {
            done.push("removed setup/SDL3.dll (1.x; SDL3.dll is beside Haiku.exe)".to_owned());
        }
    }
    let work = root.join("setup-work");
    for dir in ["overrides", "derived", "link", "code", "regenerated"] {
        let path = work.join(dir);
        if path.is_dir() && std::fs::remove_dir_all(&path).is_ok() {
            done.push(format!("removed setup-work/{dir} (1.x)"));
        }
    }
    let _ = std::fs::remove_file(work.join("link.rsp"));
    let old_log = root.join("Haiku Setup.log");
    if old_log.is_file() {
        let _ = std::fs::create_dir_all(&work);
        if std::fs::rename(&old_log, work.join("setup-1.x.log")).is_ok() {
            done.push("moved Haiku Setup.log to setup-work/setup-1.x.log".to_owned());
        }
    }
    // versions: the game, Haiku's files, the mods and caches of a folder
    // laid out flat (1.x, or this release before versions) into
    // games\<version>\ (layout.rs)
    done.extend(crate::layout::migrate(root));
    done.extend(crate::layout::sweep(root)); // versions2: what a removed version left
    if !done.is_empty() {
        // in the setup's log (the setup opens it again after, appending)
        log::open(&crate::pipeline::Paths::new(root.to_path_buf()).log_file(), false);
        for line in &done {
            log::line(format!("upgrade: {line}"));
        }
    }
    done
}
