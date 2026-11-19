//! Building Haiku from the player's dump, step by step:
//!
//! 1. check the whole dump (and DLC) against their own hashes;
//! 2. install the game's files into data/ (zakuro_fs::datadir, the same
//!    installer as `Haiku.exe --install`);
//! 3. install the DLC into data/dlc;
//! 4. make Haiku's game-derived files (assets/, and the recompiler's
//!    amiibo_icons.c) from them (derive.rs);
//! 5. write the game's code as C (3dsrecomp, with Haiku's overrides);
//! 6. compile it with the bundled GCC;
//! 7. link it with the prebuilt engine (everything of Haiku that holds no
//!    game code) into Haiku.exe.
//!
//! A step that finished for the same dump and the same setup is not done
//! again, and compiling picks up where it stopped (an object is kept once
//! it is newer than its C), so a failed or cancelled build can be resumed.
//! Nothing is written outside the folder Haiku Setup is in (and, only when
//! that folder's path isn't ASCII, a junction in C:\Users\Public that is
//! removed again).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, UNIX_EPOCH};

use crate::{derive, dlc, dump, log, toolchain};

/// the setup's version: a different one builds everything again.
pub const SETUP_VERSION: &str = "1";

#[derive(Clone)]
pub struct Paths {
    /// the folder Haiku Setup.exe is in, where Haiku.exe goes.
    pub root: PathBuf,
    /// what the release brings: engine/, toolchain/, overrides/, derived.pack.
    pub setup: PathBuf,
    /// intermediate files, and the state.
    pub work: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Paths {
        Paths { setup: root.join("setup"), work: root.join("setup-work"), root }
    }
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn exe(&self) -> PathBuf {
        self.root.join("Haiku.exe")
    }
    pub fn state_file(&self) -> PathBuf {
        self.work.join("state.txt")
    }
    pub fn log_file(&self) -> PathBuf {
        self.root.join("Haiku Setup.log")
    }
}

/// what the setup remembers between runs: the dump and DLC chosen, and the
/// steps done for them.
#[derive(Default, Clone, Debug)]
pub struct State {
    pub values: BTreeMap<String, String>,
}

impl State {
    pub fn load(paths: &Paths) -> State {
        let mut values = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(paths.state_file()) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('\t') {
                    values.insert(k.to_owned(), v.to_owned());
                }
            }
        }
        State { values }
    }
    pub fn save(&self, paths: &Paths) {
        let _ = std::fs::create_dir_all(&paths.work);
        let text: String = self.values.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
        let _ = std::fs::write(paths.state_file(), text);
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_owned(), value.into());
    }
    pub fn dlc_paths(&self) -> Vec<PathBuf> {
        self.get("dlc").map(|v| v.split('|').filter(|s| !s.is_empty()).map(PathBuf::from).collect()).unwrap_or_default()
    }
}

/// what the UI shows while it builds.
#[derive(Default, Clone)]
pub struct Status {
    pub percent: f32,
    pub step: String,
    pub detail: String,
    pub finished: Option<Result<String, String>>,
}

pub type Shared = Arc<Mutex<Status>>;

/// the steps and their share of the time.
const STEPS: &[(&str, f32)] = &[
    ("verify", 7.0),
    ("game", 9.0),
    ("dlc", 3.0),
    ("assets", 2.0),
    ("generate", 9.0),
    ("compile", 64.0),
    ("link", 6.0),
];

struct Progress {
    status: Shared,
    base: f32,
    weight: f32,
}

impl Progress {
    fn begin(status: &Shared, name: &str, step: &str) -> Progress {
        let base: f32 = STEPS.iter().take_while(|(n, _)| *n != name).map(|(_, w)| w).sum();
        let weight = STEPS.iter().find(|(n, _)| *n == name).map_or(0.0, |(_, w)| *w);
        {
            let mut s = status.lock().unwrap();
            s.step = step.to_owned();
            s.detail.clear();
            s.percent = base;
        }
        log::line(format!("== {step}"));
        Progress { status: status.clone(), base, weight }
    }
    fn at(&self, fraction: f32, detail: impl Into<String>) {
        let mut s = self.status.lock().unwrap();
        s.percent = self.base + self.weight * fraction.clamp(0.0, 1.0);
        s.detail = detail.into();
    }
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// what identifies a file for the steps made from it.
fn file_key(path: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{}|{size}|{}", path.display(), mtime(path))
}

fn payload_key(paths: &Paths) -> String {
    let id = std::fs::read_to_string(paths.setup.join("payload.txt")).unwrap_or_default();
    format!("{SETUP_VERSION}|{}", id.lines().next().unwrap_or("dev"))
}

/// a step done before for the same inputs.
fn done(state: &State, step: &str, key: &str) -> bool {
    state.get(&format!("done.{step}")) == Some(key)
}

fn mark(state: &mut State, paths: &Paths, step: &str, key: &str) {
    state.set(&format!("done.{step}"), key);
    state.save(paths);
}

fn remove_dir(path: &Path) -> Result<(), String> {
    if path.exists() {
        // nothing Haiku Setup makes holds a junction, except the toolchain
        // links in C:\Users\Public, which are never inside these folders
        std::fs::remove_dir_all(path).map_err(|e| format!("{} can't be removed ({e}). Close anything using it and try again.", path.display()))?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// builds Haiku. Err holds a message for the player.
pub fn run(paths: &Paths, game: &dump::Game, dlcs: &[dlc::Dlc], status: &Shared, cancel: &AtomicBool) -> Result<String, String> {
    let started = Instant::now();
    std::fs::create_dir_all(&paths.work).map_err(|e| format!("The folder {} can't be written ({e}). Put Haiku in a folder you can write to, such as one in your Documents.", paths.work.display()))?;
    let mut state = State::load(paths);
    state.set("dump", game.path.display().to_string());
    state.set("dlc", dlcs.iter().map(|d| d.path.display().to_string()).collect::<Vec<_>>().join("|"));
    state.save(paths);
    let stopped = || cancel.load(Ordering::Relaxed);
    let stop = || Err::<String, String>("Stopped. Start the build again to carry on where it stopped.".into());
    let dump_key = format!("{}|{}", file_key(&game.path), payload_key(paths));
    log::line(format!("building from {} ({}, {} bytes), {} DLC", game.path.display(), game.kind, game.size, dlcs.len()));
    // eurdlc: a DLC that can't serve this game is said before anything is
    // checked, installed or removed (the DLC installed before stays)
    for d in dlcs {
        crate::jpn::dlc_fits(game.region, d.title_id).map_err(|why| format!("The DLC doesn't fit this game: {why}"))?;
    }

    // 1. the whole dump against its hashes
    let p = Progress::begin(status, "verify", "Checking your dump");
    if done(&state, "verify", &dump_key) {
        log::line("checked before");
    } else {
        dump::verify_romfs(game, &|f| p.at(f * 0.8, format!("Checking the game's data, {:.0}%", f * 100.0))).map_err(|e| e.message().to_owned())?;
        for d in dlcs {
            dlc::verify(d, &|f| p.at(0.8 + 0.2 * f, format!("Checking the DLC, {:.0}%", f * 100.0))).map_err(|e| e.message().to_owned())?;
        }
        mark(&mut state, paths, "verify", &dump_key);
    }
    if stopped() {
        return stop();
    }

    // 2. the game's files
    let data = paths.data();
    let p = Progress::begin(status, "game", "Installing the game's files");
    if done(&state, "game", &dump_key) && zakuro_fs::datadir::is_game_dir(&data) {
        log::line("installed before");
    } else {
        state.set("done.game", "");
        state.set("done.dlc", "");
        state.save(paths);
        remove_dir(&data.join("romfs"))?;
        let counter = &zakuro_fs::datadir::EXTRACTED;
        let before = counter.load(Ordering::Relaxed);
        let result = std::thread::scope(|scope| {
            let job = scope.spawn(|| zakuro_fs::datadir::install_game(&game.path, &data, false));
            while !job.is_finished() {
                let n = counter.load(Ordering::Relaxed) - before;
                p.at(n as f32 / 21880.0, format!("{n} of 21,880 files"));
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            job.join().unwrap()
        });
        let tally = result.map_err(|e| format!("The game's files can't be installed: {}", crate::clean_error(&e.to_string())))?;
        log::line(format!("game: {} files, {} folders, {} MB", tally.files, tally.dirs, tally.bytes / 1_000_000));
        // the installer names the source in install.txt; it stays on this PC
        mark(&mut state, paths, "game", &dump_key);
    }
    if stopped() {
        return stop();
    }

    // 3. the DLC
    let dlc_key = format!("{}|{}", dlcs.iter().map(|d| file_key(&d.path)).collect::<Vec<_>>().join(";"), payload_key(paths));
    let p = Progress::begin(status, "dlc", if dlcs.is_empty() { "No DLC" } else { "Installing the DLC" });
    if done(&state, "dlc", &dlc_key) {
        log::line("DLC as before");
    } else {
        let dlc_dir = data.join("dlc");
        // a Haiku folder picked as the DLC may be this very data/dlc
        let keep = dlcs.iter().any(|d| matches!(d.kind, dlc::Kind::Haiku) && d.path.starts_with(&dlc_dir));
        if !keep {
            remove_dir(&dlc_dir)?;
        }
        for (i, d) in dlcs.iter().enumerate() {
            p.at(i as f32 / dlcs.len() as f32, format!("{} contents", d.contents.len()));
            let said = dlc::install(d, &data).map_err(|e| format!("The DLC can't be installed: {e}"))?;
            log::line(format!("DLC {}: {said}", d.path.display()));
        }
        // jpprep: a DLC that can't serve this game (the Japanese DLC on the
        // USA or EUR game, a USA or EUR DLC on the Japanese one) is said
        // before it is used
        if let Some(region) = crate::jpn::installed_region(&data) {
            for d in dlcs {
                crate::jpn::dlc_fits(region, d.title_id).map_err(|why| format!("The DLC doesn't fit this game: {why}"))?;
            }
        }
        // eur: the DLC next to an EUR game, filed as it asks and with its
        // text in every EUR language folder (nothing for the USA game)
        for line in zakuro_fs::eur::adapt_dlc(&data).map_err(|e| format!("The DLC can't be fitted to the EUR game: {e}"))? {
            log::line(format!("eur: {line}"));
        }
        mark(&mut state, paths, "dlc", &dlc_key);
    }
    if stopped() {
        return stop();
    }

    // 4. Haiku's game-derived files
    let p = Progress::begin(status, "assets", "Making Haiku's own files from the game's");
    let overrides = paths.work.join("overrides");
    if done(&state, "assets", &dump_key) && paths.root.join("assets").is_dir() && overrides.join("amiibo_icons.c").is_file() {
        log::line("made before");
    } else {
        let pack_bytes = std::fs::read(paths.setup.join("derived.pack")).map_err(|e| format!("setup/derived.pack is missing ({e}). Extract the whole Haiku download again."))?;
        let pack = derive::read_pack(&pack_bytes)?;
        let staging = paths.work.join("derived");
        remove_dir(&staging)?;
        // eur: the EUR game's files differ in places (its own UI sheets,
        // fonts and message folders): what can't be made from them is
        // left out and listed in the log
        let eur = zakuro_fs::eur::is_eur_data(&data);
        // jpprep: the Japanese game, likewise (its own files, its own pack)
        let jpn = crate::jpn::installed_region(&data) == Some(crate::jpn::GameRegion::Jpn);
        let count = if jpn {
            let (count, skipped) = derive::apply_skipping(&pack, &data.join("romfs"), &staging, &|i, n| p.at(i as f32 / n as f32, format!("{i} of {n} files")))?;
            log::line(format!("jpprep: {} of Haiku's files can't be made from the Japanese game's (its USA English text and sheets)", skipped.len()));
            let jpn_pack = paths.setup.join("derived-jpn.pack");
            let extra = if jpn_pack.is_file() {
                let bytes = std::fs::read(&jpn_pack).map_err(|e| format!("setup/derived-jpn.pack: {e}"))?;
                let pack = derive::read_pack(&bytes)?;
                let (made, skipped) = derive::apply_skipping(&pack, &data.join("romfs"), &staging, &|i, n| p.at(i as f32 / n as f32, format!("{i} of {n} Japanese files")))?;
                log::line(format!("jpprep: {made} of Haiku's Japanese files made{}", if skipped.is_empty() { String::new() } else { format!("; not from this dump: {}", skipped.join(", ")) }));
                made
            } else {
                log::line("jpprep: no setup/derived-jpn.pack: Haiku's own lines (Kana, End Turn, Sunglasses, S-supports) and its menus' Japanese font are not made");
                0
            };
            count + extra
        } else if eur {
            let (count, skipped) = derive::apply_skipping(&pack, &data.join("romfs"), &staging, &|i, n| p.at(i as f32 / n as f32, format!("{i} of {n} files")))?;
            log::line(format!("eur: {} of Haiku's files can't be made from the EUR game's: {}", skipped.len(), skipped.join(", ")));
            // eur2: Haiku's own text in the EUR game's five languages and its
            // button prompt sheets (derived-eur.pack, made against the EUR
            // game), over those
            let eur_pack = paths.setup.join("derived-eur.pack");
            let extra = if eur_pack.is_file() {
                let bytes = std::fs::read(&eur_pack).map_err(|e| format!("setup/derived-eur.pack: {e}"))?;
                let pack = derive::read_pack(&bytes)?;
                let (made, skipped) = derive::apply_skipping(&pack, &data.join("romfs"), &staging, &|i, n| p.at(i as f32 / n as f32, format!("{i} of {n} EUR files")))?;
                log::line(format!("eur2: {made} of Haiku's EUR files made{}", if skipped.is_empty() { String::new() } else { format!("; not from this dump: {}", skipped.join(", ")) }));
                made
            } else {
                log::line("eur2: no setup/derived-eur.pack: Haiku's text stays English-only on the EUR game");
                0
            };
            count + extra
        } else {
            derive::apply(&pack, &data.join("romfs"), &staging, &|i, n| p.at(i as f32 / n as f32, format!("{i} of {n} files")))
                .map_err(|e| format!("Haiku's files can't be made from your dump: {e}"))?
        };
        // the release's own assets (no game data) under the derived ones
        let assets = paths.root.join("assets");
        remove_dir(&assets)?;
        if paths.setup.join("assets").is_dir() {
            copy_dir(&paths.setup.join("assets"), &assets).map_err(|e| format!("copying Haiku's assets: {e}"))?;
        }
        copy_dir(&staging.join("assets"), &assets).map_err(|e| format!("copying Haiku's assets: {e}"))?;
        // the recompiler's inputs: the overrides, and the one made here
        remove_dir(&overrides)?;
        copy_dir(&paths.setup.join("overrides"), &overrides).map_err(|e| format!("copying the overrides: {e}"))?;
        copy_dir(&staging.join("overrides"), &overrides).map_err(|e| format!("copying the overrides: {e}"))?;
        // eur: the overrides with the EUR game's addresses
        if eur {
            let n = crate::eur::translate_overrides(&overrides, &paths.setup.join("eur"))?;
            log::line(format!("eur: {n} override files translated to the EUR addresses"));
        }
        // jpprep: and the Japanese game's, and its menus' font for Haiku's
        // own menus (its kana and kanji), made from the dump
        if jpn {
            match crate::jpn::make_font(&data.join("romfs"), &assets) {
                Ok(n) => log::line(format!("jpprep: the Japanese menus' font: {n} sheets")),
                Err(e) => log::line(format!("jpprep: the Japanese menus' font can't be made ({e}): Haiku's menus show Latin letters only")),
            }
            let n = crate::jpn::translate_overrides(&overrides, &paths.setup)?;
            log::line(format!("jpprep: {n} override files translated to the Japanese addresses"));
        }
        remove_dir(&staging)?;
        log::line(format!("made {count} files"));
        mark(&mut state, paths, "assets", &dump_key);
    }
    if stopped() {
        return stop();
    }

    // Haiku.exe built from this dump by this setup: only the DLC changed
    // (Change dump or DLC), so the code is as it was
    if done(&state, "link", &dump_key) && paths.exe().is_file() {
        log::line("Haiku.exe is built from this dump already");
        state.set("built", format!("{}|{}", dump_key, file_key(&paths.exe())));
        state.save(paths);
        let mut s = status.lock().unwrap();
        s.percent = 100.0;
        s.step = "Done".into();
        s.detail.clear();
        let took = started.elapsed().as_secs_f32();
        return Ok(format!("Done in {} min {} s.", (took / 60.0).floor() as u32, (took % 60.0) as u32));
    }

    // 5. the game's code as C
    let p = Progress::begin(status, "generate", "Recompiling the game's code to C");
    let code = paths.work.join("code");
    let regenerated = paths.work.join("regenerated");
    p.at(0.05, "Reading the game's code");
    // the recompiler also reads hints a run of 3dsrecomp left in APPDATA;
    // none from outside may change the code
    std::env::set_var("APPDATA", paths.work.join("appdata"));
    remove_dir(&regenerated)?;
    let events = |e: recomp3ds::build::Event| log::line(format!("recomp: {e:?}"));
    let generated = std::thread::scope(|scope| {
        let job = scope.spawn(|| recomp3ds::build::generate(&game.path, |_| regenerated.clone(), Some(&overrides), recomp3ds::Mods::default(), &events));
        let t0 = Instant::now();
        while !job.is_finished() {
            // about a minute on a fast machine
            let f = (t0.elapsed().as_secs_f32() / 75.0).min(0.95);
            p.at(f, "Writing the C");
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        job.join().unwrap()
    })
    .map_err(|e| format!("The game's code can't be recompiled: {}", crate::clean_error(&e)))?;
    // only files that changed are copied over, so their objects stay
    std::fs::create_dir_all(&code).map_err(|e| e.to_string())?;
    let mut changed = Vec::new();
    let mut folders = vec![(regenerated.clone(), code.clone())];
    while let Some((from, to)) = folders.pop() {
        std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(&from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let target = to.join(entry.file_name());
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                folders.push((entry.path(), target));
                continue;
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
            if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
                std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
                changed.push(target);
            }
        }
    }
    remove_dir(&regenerated)?;
    let headers_changed = changed.iter().any(|p| p.extension().is_some_and(|e| e == "h"));
    let pending: Vec<String> = generated
        .sources
        .iter()
        .filter(|source| {
            let path = code.join(source);
            let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let stale = modified(&path.with_extension("o")).zip(modified(&path)).is_none_or(|(object, source)| object < source);
            headers_changed || changed.contains(&path) || stale
        })
        .cloned()
        .collect();
    log::line(format!("{} C files, {} changed, {} to compile", generated.sources.len(), changed.len(), pending.len()));
    if stopped() {
        return stop();
    }

    // 6. compiling
    let p = Progress::begin(status, "compile", "Compiling the game's code");
    let tools = toolchain::Toolchain::prepare(&paths.setup, &paths.work)?;
    let jobs = toolchain::jobs();
    tools.enter(jobs);
    log::line(format!("compiler {} ({} at once), temporary files in {}", tools.gcc.display(), jobs, tools.tmp.display()));
    crate::spawn::quiet();
    let compiler = recomp3ds::compile::check(None).map_err(|e| format!("The bundled compiler doesn't run: {e}"))?;
    let total = pending.len();
    let t0 = Instant::now();
    let progress = |done: usize, total: usize| {
        let eta = if done > 0 { t0.elapsed().as_secs_f32() / done as f32 * (total - done) as f32 } else { 0.0 };
        p.at(done as f32 / total.max(1) as f32, format!("{done} of {total} files{}", if done > 2 { format!(", about {} min left", (eta / 60.0).ceil()) } else { String::new() }));
        !cancel.load(Ordering::Relaxed)
    };
    if total > 0 {
        // Cancel stops the compilers running now too (compile::cancel_running)
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
        result.map_err(|e| {
            if e == "stopped" {
                "Stopped. Start the build again to carry on where it stopped.".to_string()
            } else {
                format!("Compiling the game's code failed ({e}). The log has the compiler's messages.")
            }
        })?;
    }
    log::line(format!("compiled {total} files in {:.0} s", t0.elapsed().as_secs_f32()));
    if stopped() {
        return stop();
    }

    // 7. linking
    let p = Progress::begin(status, "link", "Linking Haiku.exe");
    p.at(0.05, "Packing the game's code");
    let library = code.join("librecomp.a");
    let _ = std::fs::remove_file(&library);
    let objects: Vec<String> = generated.sources.iter().map(|s| Path::new(s).with_extension("o").to_string_lossy().replace('\\', "/")).collect();
    let mut ar = crate::spawn::command(&tools.ar);
    ar.current_dir(&code).arg("rcs").arg("librecomp.a").args(&objects);
    let status_ar = crate::spawn::run(ar, "ar");
    if !status_ar.is_ok_and(|s| s.success()) {
        return Err("Packing the game's code failed. The log has the tool's messages.".into());
    }
    p.at(0.2, "Linking");
    let engine = paths.setup.join("engine");
    // linked as Haiku.exe (the name the exe holds) in a folder of its own,
    // then moved in place
    let partial = paths.work.join("link").join("Haiku.exe");
    remove_dir(&paths.work.join("link"))?;
    std::fs::create_dir_all(paths.work.join("link")).map_err(|e| e.to_string())?;
    link(&tools, &engine, &code, &partial, &paths.work)?;
    p.at(0.9, "Putting Haiku.exe in place");
    let exe = paths.exe();
    if exe.exists() {
        std::fs::remove_file(&exe).map_err(|e| format!("The old Haiku.exe can't be replaced ({e}). Close Haiku if it is running and try again."))?;
    }
    std::fs::rename(&partial, &exe).map_err(|e| format!("Haiku.exe can't be put in place ({e})."))?;
    for dll in ["SDL3.dll"] {
        let from = paths.setup.join(dll);
        if from.is_file() {
            std::fs::copy(&from, paths.root.join(dll)).map_err(|e| format!("{dll} can't be copied ({e}). Close Haiku if it is running and try again."))?;
        }
    }
    state.set("built", format!("{}|{}", dump_key, file_key(&exe)));
    state.set("done.link", dump_key.clone());
    state.save(paths);
    drop(tools);
    // the C and the objects are 0.5 GB; a rebuild makes them again
    if std::env::var("HAIKU_SETUP_KEEP").map_or(true, |v| v != "1") {
        remove_dir(&code)?;
        let _ = remove_dir(&paths.work.join("tmp"));
        let _ = remove_dir(&paths.work.join("appdata"));
        let _ = remove_dir(&paths.work.join("link"));
    }
    {
        let mut s = status.lock().unwrap();
        s.percent = 100.0;
        s.step = "Done".into();
        s.detail.clear();
    }
    let took = started.elapsed().as_secs_f32();
    log::line(format!("built {} in {:.0} min {:.0} s", exe.display(), (took / 60.0).floor(), took % 60.0));
    Ok(format!("Built in {} min {} s.", (took / 60.0).floor() as u32, (took % 60.0) as u32))
}

/// links the prebuilt engine and the game's code: engine/link.txt holds
/// the linker's arguments as cargo gave them (one per line, paths relative
/// to engine/), with @RECOMP@ for the folder librecomp.a is in and @OUT@
/// for the program. gcc runs in engine/ and reads them from a response file.
fn link(tools: &toolchain::Toolchain, engine: &Path, code: &Path, out: &Path, work: &Path) -> Result<(), String> {
    let template = std::fs::read_to_string(engine.join("link.txt")).map_err(|e| format!("setup/engine/link.txt is missing ({e}). Extract the whole Haiku download again."))?;
    let relative = |to: &Path| -> String { relative_path(engine, to).to_string_lossy().replace('\\', "/") };
    let recomp = relative(code);
    let output = relative(out);
    let mut rsp = String::new();
    // gcc looks for its startup files and libraries from the long path of
    // gcc.exe, which a folder named outside the ANSI code page breaks: they
    // are named here from the ASCII path the toolchain runs from
    if let Some(tc) = tools.bin.parent() {
        let tc = tc.to_string_lossy().replace('\\', "/");
        for arg in [
            format!("-B{tc}/lib/gcc/x86_64-w64-mingw32/16.2.0/"),
            format!("-B{tc}/lib/"),
            format!("-L{tc}/lib/gcc/x86_64-w64-mingw32/16.2.0"),
            format!("-L{tc}/lib"),
            format!("--sysroot={tc}"),
        ] {
            rsp += &format!("\"{arg}\"\n");
        }
    }
    for arg in template.lines().filter(|l| !l.is_empty()) {
        let arg = arg.replace("@RECOMP@", &recomp).replace("@OUT@", &output);
        // the response file's quoting: backslashes and quotes escaped
        rsp += &format!("\"{}\"\n", arg.replace('\\', "\\\\").replace('"', "\\\""));
    }
    let rsp_path = work.join("link.rsp");
    std::fs::write(&rsp_path, rsp).map_err(|e| e.to_string())?;
    let rsp_rel = relative(&rsp_path);
    log::line(format!("linking in {} with @{rsp_rel}", engine.display()));
    let mut gcc = crate::spawn::command(&tools.gcc);
    gcc.current_dir(engine).arg(format!("@{rsp_rel}"));
    let status = crate::spawn::run(gcc, "link");
    match status {
        Ok(s) if s.success() && out.is_file() => Ok(()),
        Ok(s) => Err(format!("Linking Haiku.exe failed ({s}). The log has the linker's messages.")),
        Err(e) => Err(format!("The linker doesn't run ({e}).")),
    }
}

/// to, relative to from (both absolute, on one drive).
pub fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part.as_os_str());
    }
    out
}
