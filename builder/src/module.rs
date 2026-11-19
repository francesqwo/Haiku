//! oneexe: the game's code module (data/code/game.dll), built on the
//! player's PC from the game installed in data/ (or a dump), and what says
//! which Haiku.exe it is for.
//!
//! Haiku.exe ships the engine and Haiku's overrides already compiled, and
//! no game code. The module is the game's code, recompiled by 3dsrecomp
//! with the overrides left to the host (3dsrecomp hosted.rs): the
//! recompiler is told every override's address, translated to the game's
//! build (regions.rs), and the generated functions the overrides call by
//! name; the module has a slot in each override's place, which Haiku.exe
//! fills when it loads it (source/src/module.rs).
//!
//! The module only has to be built again when what it is made of changes:
//! the recompiler (its sources, recomp.h, the hints it carries), the set
//! of overrides' addresses and named functions, the region's map, the
//! layout of the tables (VERSION and 3dsrecomp's hosted::VERSION), or the
//! game. All of that is its stamp, which the exe computes from what it was
//! built with and compares with data/code/game.txt before loading it. A
//! Haiku.exe whose overrides changed only inside (the usual update) has the
//! same stamp, and plays at once.
//!
//! Building again is incremental: the C is written again (about a minute),
//! and only the files whose text changed are compiled; the objects, and a
//! hash of what each was compiled from, are kept in setup-work/module.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use crate::pipeline::{Progress, Shared};
use crate::regions::{self, Region};
use crate::{log, toolchain};

/// the module, in data/code
pub const FILE: &str = "game.dll";
/// what it was built for and from, beside it
pub const NOTE: &str = "game.txt";
/// the layout of what Haiku adds to the module (haiku_module.c, the note)
pub const VERSION: u32 = 1;

pub const FNV_START: u64 = 0xCBF2_9CE4_8422_2325;

pub fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash = (*hash ^ byte as u64).wrapping_mul(0x0100_0000_01B3);
    }
}

/// where the module goes: data/code
pub fn dir(root: &Path) -> PathBuf {
    root.join("data").join("code")
}

/// what Haiku.exe was built with, on the module's side of the boundary
/// (source/build.rs, hosted_build.rs)
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// every override's USA address (odd for Thumb)
    pub overrides: &'static [u32],
    /// every generated function the overrides call by name (USA)
    pub functions: &'static [u32],
    /// the recompiler's sources, recomp.h and hints.txt
    pub recompiler: u64,
    /// the overrides' addresses and named functions
    pub interface: u64,
}

static SPEC: OnceLock<Spec> = OnceLock::new();

/// Haiku.exe gives its spec once, at start
pub fn set_spec(spec: Spec) {
    let _ = SPEC.set(spec);
}

pub fn spec() -> Result<Spec, String> {
    SPEC.get().copied().ok_or_else(|| "this program has no overrides to build the game's module for".to_owned())
}

impl Spec {
    /// what a module for region has to say it was built for
    pub fn stamp(&self, region: Region) -> String {
        format!(
            "haiku-module {VERSION} hosted {} generation {} recompiler {:016X} interface {:016X} region {} map {:016X}",
            recomp3ds::hosted::VERSION,
            recomp3ds::abi::GENERATION,
            self.recompiler,
            self.interface,
            region.tag(),
            regions::map_hash(region)
        )
    }
}

/// data/code/game.txt
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub stamp: String,
    pub title: u64,
    /// what the code was made from (title, the code's hash)
    pub source: String,
}

impl Note {
    pub fn read(path: &Path) -> Option<Note> {
        let text = std::fs::read_to_string(path).ok()?;
        let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' '))).map(str::to_owned);
        Some(Note { stamp: field("stamp")?, title: u64::from_str_radix(&field("title")?, 16).ok()?, source: field("source").unwrap_or_default() })
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(
            path,
            format!(
                "# Haiku's module of the game's code (game.dll), recompiled on this PC from your own game.\n# Haiku.exe builds it again by itself when it has to.\nstamp {}\ntitle {:016X}\nsource {}\n",
                self.stamp, self.title, self.source
            ),
        )
    }
}

/// whether the module in dir is the one this Haiku.exe needs for the game
/// of title (data/install.txt's), and if not, why
pub fn check(dir: &Path, title: Option<u64>) -> Result<Note, String> {
    let spec = spec()?;
    let note = Note::read(&dir.join(NOTE)).ok_or("the game's module hasn't been built")?;
    if !dir.join(FILE).is_file() {
        return Err("the game's module is missing".into());
    }
    let region = Region::of(note.title).ok_or("the game's module is of another game")?;
    if note.stamp != spec.stamp(region) {
        return Err("the game's module was built for another version of Haiku".into());
    }
    if title.is_some_and(|title| title != note.title) {
        return Err("the game's module is of another game than the one installed".into());
    }
    Ok(note)
}

/// the game's code made from (title and the code's bytes): a module of
/// another is built again
fn source_key(title: &recomp3ds::rom::Title) -> Result<String, String> {
    let code = title.code().map_err(|e| format!("The game's code can't be read: {e}"))?;
    let mut hash = FNV_START;
    fnv(&mut hash, &code);
    Ok(format!("{:016X} {:016X} {}", title.program_id(), hash, code.len()))
}

/// the C file Haiku adds to the module: what the exe takes from it besides
/// the code (the amiibo icons, made from the dump)
fn haiku_module_c(amiibo: bool) -> String {
    let mut c = String::from("/* oneexe: Haiku's own additions to the game's module (haiku-setup module.rs). */\n#include \"recomp.h\"\n\n");
    if amiibo {
        c += "extern const uint16_t amiibo_icons[4][2304];\nRECOMP_EXPORT const void *const haiku_module_amiibo_icons = amiibo_icons;\n";
    } else {
        c += "RECOMP_EXPORT const void *const haiku_module_amiibo_icons = 0;\n";
    }
    c
}

/// builds the game's module from source (a dump, or the game installed as
/// a folder, data/) in work, and puts it and its note in out. amiibo is the
/// amiibo_icons.c made from the dump (derive.rs), compiled in. setup holds
/// the bundled compiler (setup/w64devkit).
pub fn build(source: &Path, work: &Path, out: &Path, amiibo: Option<&Path>, setup: &Path, status: &Shared, cancel: &AtomicBool) -> Result<String, String> {
    let started = Instant::now();
    let spec = spec()?;
    let stopped = || cancel.load(Ordering::Relaxed);
    let stop = || Err::<String, String>("Stopped. Start Haiku again to carry on where it stopped.".into());
    std::fs::create_dir_all(work).map_err(|e| format!("{} can't be written ({e})", work.display()))?;

    // 5. the game's code as C, the overrides left to Haiku.exe
    let p = Progress::begin(status, "generate", "Recompiling the game's code to C");
    p.at(0.02, "Reading the game's code");
    let title = recomp3ds::rom::Title::load(source).map_err(|e| format!("The game's code can't be read from {}: {e}", source.display()))?;
    let program_id = title.program_id();
    let region = Region::of(program_id).ok_or_else(|| format!("{} is not Fire Emblem Fates or if ({program_id:016X})", source.display()))?;
    let source_key = source_key(&title)?;
    drop(title);
    let stamp = spec.stamp(region);
    let overrides = spec.overrides.iter().map(|&us| regions::translate(region, us).map(|at| (None, at))).collect::<Result<Vec<(Option<String>, u32)>, String>>()?;
    let functions = spec.functions.iter().map(|&us| regions::translate(region, us)).collect::<Result<Vec<u32>, String>>()?;
    let mut extra = Vec::new();
    if let Some(amiibo) = amiibo {
        let text = std::fs::read_to_string(amiibo).map_err(|e| format!("{}: {e}", amiibo.display()))?;
        extra.push(("amiibo_icons.c".to_owned(), text));
    }
    extra.push(("haiku_module.c".to_owned(), haiku_module_c(amiibo.is_some())));
    log::line(format!("module: {} ({}), {} overrides, {} named functions, stamp {stamp}", source.display(), region.tag(), overrides.len(), functions.len()));
    // the recompiler also reads hints a run of 3dsrecomp left in APPDATA;
    // none from outside may change the code
    let appdata = std::env::var_os("APPDATA");
    std::env::set_var("APPDATA", work.join("appdata"));
    let code = work.join("c");
    std::fs::create_dir_all(&code).map_err(|e| e.to_string())?;
    // what was compiled from what (objects.tsv): a file whose text is the
    // same keeps its object
    let compiled_path = code.join("objects.tsv");
    let compiled: BTreeMap<String, String> = std::fs::read_to_string(&compiled_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(a, b)| (a.to_owned(), b.to_owned())))
        .collect();
    let events = |e: recomp3ds::build::Event| log::line(format!("recomp: {e:?}"));
    let hosted = recomp3ds::build::Hosted { overrides: &overrides, functions: &functions, stamp: &stamp, extra: &extra };
    let generated = std::thread::scope(|scope| {
        let job = scope.spawn(|| recomp3ds::build::generate_hosted(source, |_| code.clone(), &hosted, recomp3ds::Mods::default(), &events));
        let t0 = Instant::now();
        while !job.is_finished() {
            // about a minute on a fast machine
            let f = (t0.elapsed().as_secs_f32() / 75.0).min(0.95);
            p.at(f, "Writing the C");
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        job.join().unwrap()
    });
    match appdata {
        Some(value) => std::env::set_var("APPDATA", value),
        None => std::env::remove_var("APPDATA"),
    }
    let generated = generated.map_err(|e| format!("The game's code can't be recompiled: {}", crate::clean_error(&e)))?;
    if stopped() {
        return stop();
    }

    // 6. compiling what changed
    let p = Progress::begin(status, "compile", "Compiling the game's code");
    let tools = toolchain::Toolchain::prepare(setup, work)?;
    let jobs = toolchain::jobs();
    tools.enter(jobs);
    crate::spawn::quiet();
    let compiler = recomp3ds::compile::check(None).map_err(|e| format!("The bundled compiler doesn't run: {e}"))?;
    let version = {
        let mut command = crate::spawn::command(&tools.gcc);
        command.arg("-dumpfullversion");
        command.output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
    };
    let header = std::fs::read(code.join("recomp.h")).unwrap_or_default();
    let key_of = |source: &str| -> String {
        let mut hash = FNV_START;
        fnv(&mut hash, version.as_bytes());
        fnv(&mut hash, &header);
        fnv(&mut hash, &std::fs::read(code.join(source)).unwrap_or_default());
        format!("{hash:016X}")
    };
    let keys: Vec<(String, String)> = generated.sources.iter().map(|s| (s.clone(), key_of(s))).collect();
    let pending: Vec<String> = keys.iter().filter(|(s, k)| compiled.get(s) != Some(k) || !code.join(s).with_extension("o").is_file()).map(|(s, _)| s.clone()).collect();
    log::line(format!("module: {} C files, {} to compile (gcc {version}, {jobs} at once)", generated.sources.len(), pending.len()));
    let total = pending.len();
    let t0 = Instant::now();
    let progress = |done: usize, total: usize| {
        let eta = if done > 0 { t0.elapsed().as_secs_f32() / done as f32 * (total - done) as f32 } else { 0.0 };
        p.at(done as f32 / total.max(1) as f32, format!("{done} of {total} files{}", if done > 2 { format!(", about {} min left", (eta / 60.0).ceil()) } else { String::new() }));
        !cancel.load(Ordering::Relaxed)
    };
    if total > 0 {
        // what is compiled already stays so even if this stops halfway:
        // the list is written as files finish
        let mut kept: BTreeMap<String, String> = compiled.into_iter().filter(|(s, _)| !pending.contains(s)).collect();
        let write_kept = |kept: &BTreeMap<String, String>| {
            let text: String = kept.iter().map(|(s, k)| format!("{s}\t{k}\n")).collect();
            let _ = std::fs::write(&compiled_path, text);
        };
        write_kept(&kept);
        recomp3ds::compile::uncancel();
        let finished = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Relaxed) {
                    if cancel.load(Ordering::Relaxed) {
                        recomp3ds::compile::cancel_running();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
            let result = recomp3ds::compile::objects(&compiler, &code, &pending, &progress, true);
            finished.store(true, Ordering::Relaxed);
            result
        });
        recomp3ds::compile::uncancel();
        // the objects that were made, whatever came of the rest
        for (s, k) in &keys {
            if pending.contains(s) && code.join(s).with_extension("o").is_file() && result.is_ok() {
                kept.insert(s.clone(), k.clone());
            }
        }
        if result.is_err() {
            // a failed or stopped run: those compiled before it finished
            // are newer than their C, which is enough to keep them
            for (s, k) in &keys {
                let object = code.join(s).with_extension("o");
                let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                if pending.contains(s) && modified(&object).zip(modified(&code.join(s))).is_some_and(|(o, c)| o >= c) {
                    kept.insert(s.clone(), k.clone());
                }
            }
        }
        write_kept(&kept);
        result.map_err(|e| {
            if e == "stopped" {
                "Stopped. Start Haiku again to carry on where it stopped.".to_string()
            } else {
                format!("Compiling the game's code failed ({e}). The log has the compiler's messages.")
            }
        })?;
    }
    log::line(format!("module: compiled {total} files in {:.0} s", t0.elapsed().as_secs_f32()));
    if stopped() {
        return stop();
    }

    // 7. linking the module, then putting it in place
    let p = Progress::begin(status, "link", "Linking the game's code");
    p.at(0.1, "Linking");
    let objects: Vec<String> = generated.sources.iter().map(|s| Path::new(s).with_extension("o").to_string_lossy().replace('\\', "/")).collect();
    let partial = code.join(FILE);
    let _ = std::fs::remove_file(&partial);
    // the objects' names in a response file, the compiler run in their
    // folder with relative names (a folder named outside the ANSI code
    // page, toolchain.rs)
    let rsp: String = objects.iter().map(|o| format!("\"{o}\"\n")).collect();
    std::fs::write(code.join("link.rsp"), rsp).map_err(|e| e.to_string())?;
    let mut gcc = crate::spawn::command(&tools.gcc);
    gcc.current_dir(&code).args(["-shared", "-static-libgcc", "-o", FILE, "@link.rsp"]);
    let linked = crate::spawn::run(gcc, "link");
    if !linked.is_ok_and(|s| s.success()) || !partial.is_file() {
        return Err("Linking the game's code failed. The log has the linker's messages.".into());
    }
    drop(tools);
    p.at(0.8, "Putting the game's code in place");
    install(&partial, out, &Note { stamp, title: program_id, source: source_key })?;
    // the C is 0.4 GB, and written again by the next build; the objects
    // stay for it
    if std::env::var("HAIKU_SETUP_KEEP").map_or(true, |v| v != "1") {
        for source in &generated.sources {
            let _ = std::fs::remove_file(code.join(source));
        }
        let _ = std::fs::remove_dir_all(work.join("tmp"));
        let _ = std::fs::remove_dir_all(work.join("appdata"));
    }
    let took = started.elapsed().as_secs_f32();
    log::line(format!("module: built {} in {:.0} s ({total} files compiled)", out.join(FILE).display(), took));
    Ok(format!("The game's code is built ({total} files compiled, {} min {} s).", (took / 60.0).floor() as u32, (took % 60.0) as u32))
}

/// moves a module into out, the old one out of the way first (Windows lets
/// a loaded library be renamed, not replaced), then writes its note
fn install(partial: &Path, out: &Path, note: &Note) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{} can't be written ({e})", out.display()))?;
    let target = out.join(FILE);
    let note_path = out.join(NOTE);
    // no note while the module is being changed: a crash in between leaves
    // a module Haiku.exe builds again
    let _ = std::fs::remove_file(&note_path);
    for n in 0..16 {
        let _ = std::fs::remove_file(out.join(format!("game.old{n}.dll")));
    }
    if target.exists() {
        let free = (0..16).map(|n| out.join(format!("game.old{n}.dll"))).find(|p| !p.exists()).unwrap_or_else(|| out.join("game.old16.dll"));
        std::fs::rename(&target, &free).map_err(|e| format!("The old game's module can't be moved aside ({e}). Close Haiku if it is running and try again."))?;
    }
    std::fs::rename(partial, &target)
        .or_else(|_| std::fs::copy(partial, &target).map(|_| ()))
        .map_err(|e| format!("The game's module can't be put in place ({e})."))?;
    note.write(&note_path).map_err(|e| format!("{}: {e}", note_path.display()))?;
    Ok(())
}
