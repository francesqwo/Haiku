//! Haiku Setup: builds Haiku (Fire Emblem Fates, recompiled for PC) from
//! the player's own dump of Fire Emblem Fates Special Edition (USA) and its
//! DLC, then starts it. The release holds no game code or data: the code is
//! recompiled here from the dump, and Haiku's game-derived files are made
//! from it (derive.rs).
//!
//! Without arguments it opens its window (ui.rs). For testing and scripts:
//!   Haiku Setup --check-game <dump>      what the dump is
//!   Haiku Setup --check-dlc <dlc>        what the DLC is
//!   Haiku Setup --build <dump> [--dlc <dlc>]...   build, printing progress
//!   Haiku Setup --voices <dump> [--dlc <dlc>] [--installed <romfs>] [--out <data>]   (jpvoice)
//!                                        take the Japanese voices out of a dump
//!   Haiku Setup --voices-dlc <dlc> [--out <data>]   (jpdlc) and of the Japanese DLC
//! Development (dev/builder/package.sh):
//!   --make-derived <repo> <romfs> <pack> [<report>]
//!   --make-derived-eur <dir> <EUR romfs> <pack> [<report>]   (eur2)
//!   --apply-derived <pack> <romfs> <out>

#![cfg_attr(windows, windows_subsystem = "windows")]

mod aes; // jpvoice
mod derive;
mod dlc;
mod dump;
mod eur; // eur: the EUR Special Edition
mod jpn; // jpprep: the Japanese game (Fire Emblem if), and which DLC fits which game
mod log;
mod lz11;
mod make;
mod pil;
mod pipeline;
mod spawn;
mod tex;
mod toolchain;
mod ui;
mod voices; // jpvoice: the optional Japanese voices (its screen: voices_ui.rs, in ui.rs)
#[cfg(windows)]
mod win;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// an error from the shared crates in the setup's words (they were written
/// for the runtime Haiku grew from, whose name the setup doesn't show).
pub fn clean_error(text: &str) -> String {
    text.replace("Zakuro needs", "Haiku needs").replace("Zakuro", "Haiku")
}

/// the folder Haiku Setup.exe is in (HAIKU_SETUP_ROOT for tests).
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("HAIKU_SETUP_ROOT") {
        return PathBuf::from(root);
    }
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned)).unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    // SAFETY: plain call; output redirected to a file or pipe keeps working
    unsafe {
        AttachConsole(u32::MAX);
    }
}
#[cfg(not(windows))]
fn attach_console() {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first().cloned() else {
        ui::run(ui::Options::from_env());
        return;
    };
    if first.starts_with("--shots") || first == "--hidden" || first == "--script" || first == "--window" {
        ui::run(ui::Options::from_args(&args));
        return;
    }
    attach_console();
    let code = match first.as_str() {
        "--check-game" => check_game(&args[1..]),
        "--check-dlc" => check_dlc(&args[1..]),
        "--build" => build(&args[1..]),
        "--make-derived" => make_derived(&args[1..]),
        "--make-derived-eur" => make_derived_eur(&args[1..]), // eur2
        "--make-derived-jpn" => make_derived_jpn(&args[1..]), // jpprep
        "--apply-derived" => apply_derived(&args[1..]),
        "--voices" => voices::command(&args[1..]), // jpvoice
        "--voices-dlc" => voices::command_dlc(&args[1..]), // jpdlc
        // eur: translate a folder of overrides to the EUR addresses in place
        // (what the build does for an EUR dump), for checking
        "--eur-translate" => match (args.get(1), args.get(2)) {
            (Some(dir), Some(map)) => match eur::translate_overrides(Path::new(dir), Path::new(map)) {
                Ok(n) => {
                    println!("{n} files translated");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
            _ => {
                eprintln!("usage: --eur-translate <overrides dir> <map dir>");
                2
            }
        },
        // jpn: translate a folder of overrides to the Japanese addresses in
        // place (what the build does for a Japanese dump), for checking
        // against dev/region/translate.py; <setup dir> holds jpn/
        "--jpn-translate" => match (args.get(1), args.get(2)) {
            (Some(dir), Some(setup)) => match jpn::translate_overrides(Path::new(dir), Path::new(setup)) {
                Ok(n) => {
                    println!("{n} files translated");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
            _ => {
                eprintln!("usage: --jpn-translate <overrides dir> <setup dir>");
                2
            }
        },
        // jpprep: the menus' font from a RomFS (the Japanese one at install)
        "--jpn-font" => match (args.get(1), args.get(2)) {
            (Some(romfs), Some(assets)) => match jpn::make_font(Path::new(romfs), Path::new(assets)) {
                Ok(n) => {
                    println!("made {n} font sheets and the glyph metrics in {assets}/font");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
            _ => {
                eprintln!("usage: --jpn-font <romfs> <assets out>");
                2
            }
        },
        "--help" | "-h" | "/?" => {
            println!("Haiku Setup: run it without arguments. For scripts: --check-game <dump>, --check-dlc <dlc>, --build <dump> [--dlc <dlc>]...");
            0
        }
        other => {
            eprintln!("unknown option {other}; run without arguments for the window");
            2
        }
    };
    std::process::exit(code);
}

fn check_game(args: &[String]) -> i32 {
    let Some(path) = args.first() else { return 2 };
    match dump::check_game(Path::new(path)) {
        Ok(game) => {
            let region = if eur::game_is_eur(&game) { "Europe, CTR-P-BFZP" } else { "USA" }; // eur
            if jpn::game_is_jpn(&game) {
                // jpprep
                println!("ok: Japanese game (Fire Emblem if), {} ({}), {} MB", game.kind, game.path.display(), game.size / 1_000_000);
            } else {
                println!("ok: Fire Emblem Fates Special Edition ({region}), {} ({}), {} MB", game.kind, game.path.display(), game.size / 1_000_000);
            }
            if args.iter().any(|a| a == "--full") {
                match dump::verify_romfs(&game, &|_| {}) {
                    Ok(()) => println!("ok: every RomFS block matches its hash"),
                    Err(p) => {
                        println!("{}: {}", p.kind(), p.message());
                        return 1;
                    }
                }
            }
            0
        }
        Err(p) => {
            println!("{}: {}", p.kind(), p.message());
            1
        }
    }
}

fn check_dlc(args: &[String]) -> i32 {
    let Some(path) = args.first() else { return 2 };
    match dlc::check(Path::new(path)) {
        Ok(d) => {
            println!("ok: {}", d.describe());
            // jpprep: --for <data> checks it against an installed game, and
            // --install files it there (data/dlc/<title>), as the build does
            if let Some(data) = args.iter().position(|a| a == "--for").and_then(|i| args.get(i + 1)) {
                let data = Path::new(data);
                let Some(region) = jpn::installed_region(data) else {
                    println!("no game: {} has no install.txt naming a Fire Emblem Fates / if game", data.display());
                    return 1;
                };
                if let Err(why) = jpn::dlc_fits(region, d.title_id) {
                    println!("wrong game: {why}");
                    return 1;
                }
                println!("ok: it fits the {region:?} game in {}", data.display());
                if args.iter().any(|a| a == "--install") {
                    match dlc::install(&d, data) {
                        Ok(said) => println!("installed: {said}"),
                        Err(e) => {
                            println!("not installed: {e}");
                            return 1;
                        }
                    }
                    match zakuro_fs::eur::adapt_dlc(data) {
                        Ok(lines) => lines.iter().for_each(|line| println!("eur: {line}")),
                        Err(e) => println!("eur: {e}"),
                    }
                }
            }
            if args.iter().any(|a| a == "--full") {
                if let Err(p) = dlc::verify(&d, &|_| {}) {
                    println!("{}: {}", p.kind(), p.message());
                    return 1;
                }
                println!("ok: every content matches its hash");
            }
            0
        }
        Err(p) => {
            println!("{}: {}", p.kind(), p.message());
            1
        }
    }
}

fn build(args: &[String]) -> i32 {
    let paths = pipeline::Paths::new(root());
    log::open(&paths.log_file(), true);
    let Some(dump_path) = args.first() else { return 2 };
    let game = match dump::check_game(Path::new(dump_path)) {
        Ok(g) => g,
        Err(p) => {
            log::line(format!("{}: {}", p.kind(), p.message()));
            return 1;
        }
    };
    let mut dlcs = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--dlc" {
            if let Some(path) = args.get(i + 1) {
                match dlc::check(Path::new(path)) {
                    Ok(d) => {
                        log::line(format!("DLC: {}", d.describe()));
                        dlcs.push(d);
                    }
                    Err(p) => {
                        log::line(format!("{}: {}", p.kind(), p.message()));
                        return 1;
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    if dlcs.is_empty() {
        log::line("no DLC: the DLC maps won't be available");
    }
    let status: pipeline::Shared = Arc::new(Mutex::new(pipeline::Status::default()));
    let cancel = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| pipeline::run(&paths, &game, &dlcs, &status, &cancel));
        let mut last = String::new();
        while !job.is_finished() {
            let s = status.lock().unwrap().clone();
            let line = format!("{:3.0}% {} {}", s.percent, s.step, s.detail);
            let coarse = format!("{:.0}{}", s.percent, s.step);
            if coarse != last {
                println!("{line}");
                last = coarse;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        job.join().unwrap()
    });
    match result {
        Ok(said) => {
            log::line(format!("ok: {said}"));
            0
        }
        Err(e) => {
            log::line(format!("failed: {e}"));
            1
        }
    }
}

/// eur2: derived-eur.pack, the recipes of Haiku's files for the EUR game
/// (its five languages' text, its button prompt sheets), from a folder of
/// them made in development and the EUR game's RomFS.
fn make_derived_eur(args: &[String]) -> i32 {
    let [dir, romfs, out, rest @ ..] = args else {
        eprintln!("usage: --make-derived-eur <dir> <EUR romfs> <pack> [<report>]");
        return 2;
    };
    let made = make::make_eur(Path::new(dir), Path::new(romfs));
    let entries: Vec<derive::Entry> = made.iter().map(|m| m.entry.clone()).collect();
    let pack = derive::write_pack(&entries);
    std::fs::write(out, &pack).unwrap();
    let total: usize = made.iter().map(|m| m.size).sum();
    let own: usize = made.iter().map(|m| m.own).sum();
    println!("{} recipes, {} KB of files, {} KB of them Haiku's own (added bytes, residuals), pack {} KB", made.len(), total / 1024, own / 1024, pack.len() / 1024);
    if let Some(report) = rest.first() {
        let mut text = String::from("target	size	haiku_bytes	recipe
");
        for m in &made {
            text += &format!("{}	{}	{}	{}
", m.entry.target, m.size, m.own, m.note);
        }
        std::fs::write(report, text).unwrap();
    }
    0
}

/// jpprep: setup/derived-jpn.pack from Haiku's Japanese files and the
/// Japanese RomFS, as --make-derived-eur.
fn make_derived_jpn(args: &[String]) -> i32 {
    let [dir, romfs, out, rest @ ..] = args else {
        eprintln!("usage: --make-derived-jpn <dir> <Japanese romfs> <pack> [<repo> <USA romfs>]");
        return 2;
    };
    let mut made = make::make_jpn(Path::new(dir), Path::new(romfs));
    // jpn: and the UI sheets the Japanese game has its own of
    if let [repo, us_romfs, ..] = rest {
        let ui = make::make_jpn_ui(Path::new(repo), Path::new(romfs), Path::new(us_romfs));
        for m in &ui {
            println!("jpn: {}: {}", m.entry.target, m.note);
        }
        made.extend(ui);
    }
    let entries: Vec<derive::Entry> = made.iter().map(|m| m.entry.clone()).collect();
    let pack = derive::write_pack(&entries);
    if let Err(e) = std::fs::write(out, &pack) {
        eprintln!("{out}: {e}");
        return 1;
    }
    let total: usize = made.iter().map(|m| m.size).sum();
    let own: usize = made.iter().map(|m| m.own).sum();
    println!("{} recipes, {} KB of files, {} KB of them Haiku's own (added bytes), pack {} KB", made.len(), total / 1024, own / 1024, pack.len() / 1024);
    0
}

fn make_derived(args: &[String]) -> i32 {
    let [repo, romfs, out, rest @ ..] = args else {
        eprintln!("usage: --make-derived <repo> <romfs> <pack> [<report>]");
        return 2;
    };
    let made = make::make_all(Path::new(repo), Path::new(romfs));
    let entries: Vec<derive::Entry> = made.iter().map(|m| m.entry.clone()).collect();
    let pack = derive::write_pack(&entries);
    std::fs::write(out, &pack).unwrap();
    let total: usize = made.iter().map(|m| m.size).sum();
    let own: usize = made.iter().map(|m| m.own).sum();
    println!("{} recipes, {} KB of files, {} KB of them Haiku's own (added bytes, residuals), pack {} KB", made.len(), total / 1024, own / 1024, pack.len() / 1024);
    if let Some(report) = rest.first() {
        let mut text = String::from("target\tsize\thaiku_bytes\trecipe\n");
        for m in &made {
            text += &format!("{}\t{}\t{}\t{}\n", m.entry.target, m.size, m.own, m.note);
        }
        std::fs::write(report, text).unwrap();
    }
    0
}

fn apply_derived(args: &[String]) -> i32 {
    let [pack, romfs, out, ..] = args else {
        eprintln!("usage: --apply-derived <pack> <romfs> <out>");
        return 2;
    };
    let entries = match derive::read_pack(&std::fs::read(pack).unwrap()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // eur: from an EUR game what can be made, and the list of what can't
    // jpn: and from a Japanese one
    let lenient = Path::new(romfs).parent().is_some_and(|data| zakuro_fs::eur::is_eur_data(data) || jpn::installed_region(data) == Some(jpn::GameRegion::Jpn));
    if lenient {
        return match derive::apply_skipping(&entries, Path::new(romfs), Path::new(out), &|_, _| {}) {
            Ok((n, skipped)) => {
                println!("made {n} files; {} can't be made from this game's files:", skipped.len());
                skipped.iter().for_each(|s| println!("  {s}"));
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
    }
    match derive::apply(&entries, Path::new(romfs), Path::new(out), &|_, _| {}) {
        Ok(n) => {
            println!("made {n} files");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
