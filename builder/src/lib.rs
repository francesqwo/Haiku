//! Haiku's setup: installs Fire Emblem Fates from the player's own dump
//! (the USA or European Special Edition, or the Japanese Fire Emblem if)
//! and its DLC, makes Haiku's game-derived files from it, and recompiles the
//! game's code into the module Haiku.exe loads (module.rs). The release
//! holds no game code or data.
//!
//! oneexe: this was Haiku Setup.exe, a program of its own that linked a
//! whole Haiku.exe. It is now a library inside Haiku.exe, which opens its
//! window on first launch (and whenever the game's module has to be built
//! again), and whose command line it keeps (`cli`):
//!   Haiku --check-game <dump>      what the dump is
//!   Haiku --check-dlc <dlc>        what the DLC is
//!   Haiku --build <dump> [--dlc <dlc>]...   install and build, printing progress
//!   Haiku --rebuild                 build the game's module again from data/
//!   Haiku --voices <dump> [--dlc <dlc>] [--installed <romfs>] [--out <data>]   (jpvoice)
//!                                        take the Japanese voices out of a dump
//!   Haiku --voices-dlc <dlc> [--out <data>]   (jpdlc) and of the Japanese DLC
//!   Haiku --setup                   the setup window (Rebuild, Change dump or DLC, Voices)
//! Development (dev/builder/package.sh, dev/recomp.sh):
//!   --make-module <dump or data dir> <out dir>   the game's module alone
//!   --make-derived <repo> <romfs> <pack> [<report>]
//!   --make-derived-eur <dir> <EUR romfs> <pack> [<report>]   (eur2)
//!   --apply-derived <pack> <romfs> <out>

mod aes; // jpvoice
mod cc; // compile32: running the compiler on the game's C (issue #13)
mod derive;
mod dlc;
mod dump;
mod eur; // eur: the EUR Special Edition
mod jpn; // jpprep: the Japanese game (Fire Emblem if), and which DLC fits which game
pub mod layout; // versions: USA, EUR and JPN side by side (games\<version>\)
pub mod log;
mod lz11;
mod make;
pub mod module; // oneexe: the game's code module, its stamp and its build
mod pil;
pub mod pipeline;
pub mod regions; // oneexe: the regions' address maps, in the exe
mod spawn;
mod tex;
mod toolchain;
pub mod ui;
pub mod upgrade; // oneexe: an install of an earlier Haiku, brought over
pub mod updscreen; // updscreen: the module built while the game's window shows it
mod voices; // jpvoice: the optional Japanese voices (its screen: voices_ui.rs, in ui.rs)
#[cfg(windows)]
mod win;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// oneexe: the recompiler, for the exe (its hosted tables' version)
pub use recomp3ds as recomp;

/// an error from the shared crates in the setup's words (they were written
/// for the runtime Haiku grew from, whose name the setup doesn't show).
pub fn clean_error(text: &str) -> String {
    text.replace("Zakuro needs", "Haiku needs").replace("Zakuro", "Haiku")
}

/// the folder Haiku.exe is in (HAIKU_SETUP_ROOT, else FATES_ROOT, for
/// tests). versions: HAIKU_HOME first, which the game sets to the Haiku
/// folder when it points FATES_ROOT at a version's (layout.rs), so a setup
/// or a restart started from the game finds the whole folder.
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("HAIKU_HOME").or_else(|| std::env::var_os("HAIKU_SETUP_ROOT")).or_else(|| std::env::var_os("FATES_ROOT")) {
        return PathBuf::from(root);
    }
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned)).unwrap_or_else(|| PathBuf::from("."))
}

/// oneexe: the setup's command line, inside Haiku.exe: the exit code when
/// args (without the program) is a setup command, None to go on to the
/// game. The window's options (--setup, --shots, --hidden, --script) open
/// it; it relaunches Haiku.exe to play when asked.
pub fn cli(args: &[String]) -> Option<i32> {
    let first = args.first()?.clone();
    if first == "--setup" || first.starts_with("--shots") || first == "--hidden" || first == "--script" || first == "--window" {
        ui::run(ui::Options::from_args(args), ui::Start::Setup);
        return Some(0);
    }
    let code = match first.as_str() {
        "--check-game" => check_game(&args[1..]),
        "--check-dlc" => check_dlc(&args[1..]),
        "--build" => build(&args[1..]),
        "--rebuild" if args.get(1).is_some_and(|a| a == "--progress") => updscreen::child(), // updscreen: for the game's window
        "--rebuild" => rebuild(), // oneexe
        "--versions" => versions(&args[1..]), // versions
        "--make-module" => make_module(&args[1..]), // oneexe
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
            println!("Haiku: run it without arguments to play (the setup opens the first time). For scripts: --check-game <dump>, --check-dlc <dlc>, --build <dump> [--dlc <dlc>]..., --rebuild, --voices <dump>, --voices-dlc <dlc>, --versions [usa|eur|jpn], --versions remove <usa|eur|jpn>, --setup");
            0
        }
        _ => return None,
    };
    Some(code)
}

/// versions: `--versions` lists the versions installed and the one that
/// plays; `--versions <usa|eur|jpn>` makes that one play (as the options'
/// Game Version does).
fn versions(args: &[String]) -> i32 {
    let root = root();
    let installed = layout::installed(&root);
    // versions2: `--versions remove <usa|eur|jpn>`, Haiku Setup's Remove a
    // version without its window (its games\<version>\ deleted, the saves
    // kept; never the only one)
    if args.first().map(String::as_str) == Some("remove") {
        let Some(version) = args.get(1).and_then(|tag| layout::Version::from_tag(tag)) else {
            eprintln!("usage: --versions remove <usa|eur|jpn>");
            return 2;
        };
        let paths = pipeline::Paths::new(root.clone());
        log::open(&paths.log_file(), false);
        log::line(format!("--versions remove {}", version.tag()));
        let folder = match layout::remove(&root, version) {
            Ok(folder) => folder,
            Err(e) => {
                log::line(format!("  not removed: {e}"));
                eprintln!("{e}");
                return 1;
            }
        };
        let deleted = layout::delete_removed(&folder);
        log::line(format!("  removed {} ({}); playing {}", version.tag(), deleted.as_ref().map_or_else(|e| format!("some files left, deleted at the next start: {e}"), |_| "deleted".into()), layout::active(&root).map(|v| v.tag()).unwrap_or("none")));
        println!("removed {} (games\\{}); the saves are kept", version.name(), version.folder());
        if let Err(e) = deleted {
            println!("some of its files are left ({e}); they are deleted at the next start");
        }
        return versions(&[]);
    }
    if let Some(want) = args.first() {
        let Some(version) = layout::Version::from_tag(want) else {
            eprintln!("usage: --versions [usa|eur|jpn]");
            return 2;
        };
        if !installed.contains(&version) {
            eprintln!("{} isn't installed (installed: {})", version.name(), installed.iter().map(|v| v.tag()).collect::<Vec<_>>().join(", "));
            return 1;
        }
        if let Err(e) = layout::set_active(&root, version) {
            eprintln!("the settings can't be written: {e}");
            return 1;
        }
    }
    let active = layout::active(&root);
    for v in &installed {
        println!("{}{} in {}", v.tag(), if Some(*v) == active { " (plays)" } else { "" }, layout::dir(&root, *v).display());
    }
    if installed.is_empty() {
        println!("no version in {} (a flat folder plays from data\\)", root.join(layout::GAMES).display());
    }
    0
}

/// oneexe: the game's module built again from the game installed in data/,
/// as Haiku.exe does by itself when the module is out of date.
fn rebuild() -> i32 {
    let paths = pipeline::Paths::new(root());
    log::open(&paths.log_file(), true);
    let status: pipeline::Shared = Arc::new(Mutex::new(pipeline::Status::default()));
    let cancel = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| pipeline::update(&paths, &status, &cancel));
        print_progress(&job, &status);
        job.join().unwrap()
    });
    finish(result)
}

/// oneexe: the game's module alone, from a dump or a game installed as a
/// folder, into a folder of its own (development: dev/recomp.sh). The
/// amiibo icons come from <out>/amiibo_icons.c when it is there.
fn make_module(args: &[String]) -> i32 {
    let (Some(source), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: --make-module <dump or data dir> <out dir>");
        return 2;
    };
    let out = std::path::absolute(out).unwrap_or_else(|_| PathBuf::from(out));
    let source = std::path::absolute(source).unwrap_or_else(|_| PathBuf::from(source));
    let source = source.as_path();
    let _ = std::fs::create_dir_all(&out);
    log::open(&out.join("make-module.log"), true);
    let status: pipeline::Shared = Arc::new(Mutex::new(pipeline::Status::default()));
    let cancel = AtomicBool::new(false);
    let amiibo = out.join("amiibo_icons.c");
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| module::build(source, &out.join("work"), &out, amiibo.is_file().then_some(amiibo.as_path()), &root().join("setup"), &status, &cancel));
        print_progress(&job, &status);
        job.join().unwrap()
    });
    finish(result)
}

fn finish(result: Result<String, String>) -> i32 {
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

fn print_progress<T>(job: &std::thread::ScopedJoinHandle<T>, status: &pipeline::Shared) {
    let mut last = String::new();
    while !job.is_finished() {
        let s = status.lock().unwrap().clone();
        let coarse = format!("{:.0}{}", s.percent, s.step);
        if coarse != last {
            println!("{:3.0}% {} {}", s.percent, s.step, s.detail);
            last = coarse;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
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
        print_progress(&job, &status);
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
