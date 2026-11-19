//! Setting Haiku up from the player's dump, step by step:
//!
//! 1. check the whole dump (and DLC) against their own hashes;
//! 2. install the game's files into data/ (zakuro_fs::datadir, the same
//!    installer as `Haiku.exe --install`);
//! 3. install the DLC into data/dlc;
//! 4. make Haiku's game-derived files (assets/, and the amiibo icons the
//!    game's module carries) from them (derive.rs);
//! 5-7. recompile the game's code from data/ into its module,
//!    data/code/game.dll (module.rs): write it as C (3dsrecomp, the
//!    overrides left to Haiku.exe), compile what changed with the bundled
//!    GCC, link it.
//!
//! oneexe: Haiku.exe ships built (the engine and Haiku's overrides); no
//! Haiku.exe is linked here any more. A step that finished for the same
//! inputs is not done again: the install steps are keyed on the dump and
//! DLC (an earlier Haiku's keys are taken too, upgrade.rs), Haiku's files on
//! the recipes that make them, the module on its stamp (module.rs).
//! Compiling picks up where it stopped, and keeps every object whose C
//! didn't change, so building the module again is incremental. Nothing is
//! written outside the folder Haiku.exe is in (and, only when that folder's
//! path isn't ASCII, a junction in C:\Users\Public that is removed again).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, UNIX_EPOCH};

use crate::{derive, dlc, dump, log};

/// the keys' version (oneexe: 2; 1.x keyed everything on the payload,
/// upgrade.rs reads those)
pub const SETUP_VERSION: &str = "2";

#[derive(Clone)]
pub struct Paths {
    /// the folder Haiku.exe is in.
    pub root: PathBuf,
    /// versions: the folder of the version these paths are for,
    /// games\<version>\ (layout.rs), or root in a flat folder
    pub game: PathBuf,
    /// what the release brings: w64devkit/, derived*.pack, assets/.
    pub setup: PathBuf,
    /// intermediate files, and the state.
    pub work: PathBuf,
}

impl Paths {
    /// the version that plays (layout::active), or the flat folder
    pub fn new(root: PathBuf) -> Paths {
        let game = crate::layout::game_dir(&root);
        Paths { setup: root.join("setup"), work: game.join("setup-work"), game, root }
    }
    /// versions: the same for one version, in games\<version>\; a
    /// development folder (no setup\) stays flat
    pub fn for_version(&self, version: crate::layout::Version) -> Paths {
        if !self.managed() {
            return self.clone();
        }
        let game = crate::layout::dir(&self.root, version);
        Paths { setup: self.setup.clone(), work: game.join("setup-work"), game, root: self.root.clone() }
    }
    pub fn data(&self) -> PathBuf {
        self.game.join("data")
    }
    /// Haiku's files made from the game
    pub fn assets(&self) -> PathBuf {
        self.game.join("assets")
    }
    /// the program to start to play: this one (Haiku.exe)
    pub fn exe(&self) -> PathBuf {
        std::env::current_exe().unwrap_or_else(|_| self.root.join("Haiku.exe"))
    }
    pub fn state_file(&self) -> PathBuf {
        self.work.join("state.txt")
    }
    /// oneexe: the setup's log, in setup-work (1.x wrote Haiku Setup.log
    /// beside Haiku Setup.exe; upgrade.rs moves it here)
    pub fn log_file(&self) -> PathBuf {
        // versions: one log for the whole folder
        self.root.join("setup-work").join("setup.log")
    }
    /// oneexe: the game's module and its note
    pub fn module_dir(&self) -> PathBuf {
        crate::module::dir(&self.game)
    }
    /// oneexe: the amiibo icons made from the dump, which the module carries
    pub fn amiibo(&self) -> PathBuf {
        self.work.join("amiibo_icons.c")
    }
    /// oneexe: a release install (setup/ beside Haiku.exe), which the setup
    /// keeps; a development or test folder has none
    pub fn managed(&self) -> bool {
        self.setup.join("payload.txt").is_file() || self.setup.join("w64devkit").is_dir()
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
    ("generate", 11.0),
    ("compile", 66.0),
    ("link", 2.0),
];

pub(crate) struct Progress {
    status: Shared,
    base: f32,
    weight: f32,
}

impl Progress {
    pub(crate) fn begin(status: &Shared, name: &str, step: &str) -> Progress {
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
    pub(crate) fn at(&self, fraction: f32, detail: impl Into<String>) {
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

/// a step done before for the same inputs (oneexe: or by Haiku 1.x, whose
/// keys were `<inputs>|1|<payload>`, for the steps that only depend on the
/// inputs: checking and installing the dump and DLC)
fn done(state: &State, step: &str, key: &str) -> bool {
    let Some(value) = state.get(&format!("done.{step}")) else { return false };
    if value == key {
        return true;
    }
    let inputs = key.strip_prefix(&format!("{SETUP_VERSION}|")).unwrap_or(key);
    matches!(step, "verify" | "game" | "dlc") && !inputs.is_empty() && value.strip_prefix(inputs).is_some_and(|rest| rest.starts_with("|1|"))
}

/// oneexe: what Haiku's game-derived files are made from: the game
/// installed (data/install.txt) and the recipes and own files this release
/// brings for it. A release whose recipes are the same keeps them.
pub fn assets_key(paths: &Paths) -> String {
    let mut hash = crate::module::FNV_START;
    crate::module::fnv(&mut hash, &std::fs::read(paths.data().join(zakuro_fs::datadir::MANIFEST_FILE)).unwrap_or_default());
    for pack in ["derived.pack", "derived-eur.pack", "derived-jpn.pack"] {
        crate::module::fnv(&mut hash, pack.as_bytes());
        crate::module::fnv(&mut hash, &std::fs::read(paths.setup.join(pack)).unwrap_or_default());
    }
    // the release's own assets, by name and size
    let mut files = Vec::new();
    let mut dirs = vec![paths.setup.join("assets")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push((path.strip_prefix(&paths.setup).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default(), entry.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    files.sort();
    for (name, size) in files {
        crate::module::fnv(&mut hash, name.as_bytes());
        crate::module::fnv(&mut hash, &size.to_le_bytes());
    }
    format!("{SETUP_VERSION}|assets|{hash:016X}")
}

/// oneexe: the title data/ holds (install.txt), if any
pub fn installed_title(data: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(data.join(zakuro_fs::datadir::MANIFEST_FILE)).ok()?;
    text.lines().find_map(|line| line.strip_prefix("title ")).and_then(|t| u64::from_str_radix(t.trim(), 16).ok())
}

/// oneexe: what Haiku.exe asks at start: whether it can play now, and if
/// not, what the setup has to do. A release install (setup/ beside it)
/// also has its game-derived files checked; a development folder only its
/// module.
#[derive(Debug, Clone, PartialEq)]
pub enum Ready {
    /// play
    Yes,
    /// no game installed: the setup's first screens
    Install,
    /// the game is installed, but Haiku's files or the game's module have to
    /// be made again (an update, an upgrade from 1.x): the setup does it by
    /// itself, then plays
    Update(String),
}

pub fn ready(paths: &Paths) -> Ready {
    let data = paths.data();
    if !zakuro_fs::datadir::is_game_dir(&data) {
        return Ready::Install;
    }
    if paths.managed() {
        let state = State::load(paths);
        if !done(&state, "assets", &assets_key(paths)) || !paths.amiibo().is_file() || !paths.assets().is_dir() {
            return Ready::Update("Haiku's files have to be made again".into());
        }
    }
    match crate::module::check(&paths.module_dir(), installed_title(&data)) {
        Ok(_) => Ready::Yes,
        Err(why) => Ready::Update(why),
    }
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

/// sets Haiku up from a dump and its DLC: installs them, then makes
/// Haiku's files and the game's module (finish). Err holds a message for
/// the player.
pub fn run(paths: &Paths, game: &dump::Game, dlcs: &[dlc::Dlc], status: &Shared, cancel: &AtomicBool) -> Result<String, String> {
    let started = Instant::now();
    // versions: a dump installs into its own version's folder, beside the
    // versions installed before (which it never touches); a dump of the
    // version installed already replaces that one
    let version = crate::layout::Version::of_region(game.region);
    let routed = paths.for_version(version);
    let paths = &routed;
    if paths.managed() {
        log::line(format!("versions: {} goes in {}", version.name(), paths.game.display()));
    }
    std::fs::create_dir_all(&paths.work).map_err(|e| format!("The folder {} can't be written ({e}). Put Haiku in a folder you can write to, such as one in your Documents.", paths.work.display()))?;
    let mut state = State::load(paths);
    state.set("dump", game.path.display().to_string());
    state.set("dlc", dlcs.iter().map(|d| d.path.display().to_string()).collect::<Vec<_>>().join("|"));
    state.save(paths);
    let stopped = || cancel.load(Ordering::Relaxed);
    let stop = || Err::<String, String>("Stopped. Start the build again to carry on where it stopped.".into());
    let game_key = format!("{SETUP_VERSION}|{}", file_key(&game.path));
    let dlc_files = dlcs.iter().map(|d| file_key(&d.path)).collect::<Vec<_>>().join(";");
    let verify_key = format!("{SETUP_VERSION}|{}", file_key(&game.path));
    log::line(format!("building from {} ({}, {} bytes), {} DLC", game.path.display(), game.kind, game.size, dlcs.len()));
    // eurdlc: a DLC that can't serve this game is said before anything is
    // checked, installed or removed (the DLC installed before stays)
    for d in dlcs {
        crate::jpn::dlc_fits(game.region, d.title_id).map_err(|why| format!("The DLC doesn't fit this game: {why}"))?;
    }

    // 1. the whole dump against its hashes (the DLC is checked when it is
    // installed, below)
    let p = Progress::begin(status, "verify", "Checking your dump");
    if done(&state, "verify", &verify_key) {
        log::line("checked before");
    } else {
        dump::verify_romfs(game, &|f| p.at(f, format!("Checking the game's data, {:.0}%", f * 100.0))).map_err(|e| e.message().to_owned())?;
        mark(&mut state, paths, "verify", &verify_key);
    }
    if stopped() {
        return stop();
    }

    // 2. the game's files
    let data = paths.data();
    let p = Progress::begin(status, "game", "Installing the game's files");
    if done(&state, "game", &game_key) && zakuro_fs::datadir::is_game_dir(&data) {
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
        mark(&mut state, paths, "game", &game_key);
    }
    if stopped() {
        return stop();
    }

    // 3. the DLC
    let dlc_key = format!("{SETUP_VERSION}|{dlc_files}");
    let p = Progress::begin(status, "dlc", if dlcs.is_empty() { "No DLC" } else { "Installing the DLC" });
    if done(&state, "dlc", &dlc_key) {
        log::line("DLC as before");
    } else {
        for d in dlcs {
            dlc::verify(d, &|f| p.at(0.5 * f, format!("Checking the DLC, {:.0}%", f * 100.0))).map_err(|e| e.message().to_owned())?;
        }
        let dlc_dir = data.join("dlc");
        // a Haiku folder picked as the DLC may be this very data/dlc
        let keep = dlcs.iter().any(|d| matches!(d.kind, dlc::Kind::Haiku) && d.path.starts_with(&dlc_dir));
        if !keep {
            remove_dir(&dlc_dir)?;
        }
        for (i, d) in dlcs.iter().enumerate() {
            p.at(0.5 + 0.5 * i as f32 / dlcs.len() as f32, format!("{} contents", d.contents.len()));
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
    let said = finish(paths, &mut state, status, cancel)?;
    // versions: the version just set up is the one that plays
    if paths.managed() {
        if let Err(e) = crate::layout::set_active(&paths.root, version) {
            log::line(format!("versions: the settings can't say {} plays ({e})", version.name()));
        }
    }
    let took = started.elapsed().as_secs_f32();
    log::line(format!("set up in {:.0} min {:.0} s: {said}", (took / 60.0).floor(), took % 60.0));
    Ok(format!("Done in {} min {} s.", (took / 60.0).floor() as u32, (took % 60.0) as u32))
}

/// oneexe: what Haiku.exe does by itself when a new release (or an earlier
/// Haiku's install) needs Haiku's files or the game's module made again:
/// the game installed in data/ is all it reads.
pub fn update(paths: &Paths, status: &Shared, cancel: &AtomicBool) -> Result<String, String> {
    let started = Instant::now();
    if !zakuro_fs::datadir::is_game_dir(&paths.data()) {
        return Err("No game is installed yet: set Haiku up from your dump first.".into());
    }
    std::fs::create_dir_all(&paths.work).map_err(|e| format!("The folder {} can't be written ({e}). Put Haiku in a folder you can write to, such as one in your Documents.", paths.work.display()))?;
    let mut state = State::load(paths);
    let said = finish(paths, &mut state, status, cancel)?;
    let took = started.elapsed().as_secs_f32();
    log::line(format!("updated in {:.0} min {:.0} s: {said}", (took / 60.0).floor(), took % 60.0));
    Ok(format!("Done in {} min {} s.", (took / 60.0).floor() as u32, (took % 60.0) as u32))
}

/// Haiku's game-derived files, and the game's module, each when what it is
/// made of changed.
fn finish(paths: &Paths, state: &mut State, status: &Shared, cancel: &AtomicBool) -> Result<String, String> {
    let stopped = || cancel.load(Ordering::Relaxed);
    let stop = || Err::<String, String>("Stopped. Start the build again to carry on where it stopped.".into());
    let data = paths.data();

    // 4. Haiku's game-derived files
    let p = Progress::begin(status, "assets", "Making Haiku's own files from the game's");
    let key = assets_key(paths);
    if done(state, "assets", &key) && paths.assets().is_dir() && paths.amiibo().is_file() {
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
        let assets = paths.assets();
        remove_dir(&assets)?;
        if paths.setup.join("assets").is_dir() {
            copy_dir(&paths.setup.join("assets"), &assets).map_err(|e| format!("copying Haiku's assets: {e}"))?;
        }
        copy_dir(&staging.join("assets"), &assets).map_err(|e| format!("copying Haiku's assets: {e}"))?;
        // oneexe: the amiibo icons, which the game's module carries (the
        // overrides themselves are in Haiku.exe, for every region)
        let icons = staging.join("overrides").join("amiibo_icons.c");
        std::fs::copy(&icons, paths.amiibo()).map_err(|e| format!("The amiibo icons can't be made from your dump ({}: {e})", icons.display()))?;
        // jpprep: the Japanese menus' font for Haiku's own menus (its kana
        // and kanji), made from the dump
        if jpn {
            match crate::jpn::make_font(&data.join("romfs"), &assets) {
                Ok(n) => log::line(format!("jpprep: the Japanese menus' font: {n} sheets")),
                Err(e) => log::line(format!("jpprep: the Japanese menus' font can't be made ({e}): Haiku's menus show Latin letters only")),
            }
        }
        remove_dir(&staging)?;
        log::line(format!("made {count} files"));
        mark(state, paths, "assets", &key);
    }
    if stopped() {
        return stop();
    }

    // 5-7. the game's module
    match crate::module::check(&paths.module_dir(), installed_title(&data)) {
        Ok(_) => {
            log::line("the game's module is built for this Haiku already");
            let mut s = status.lock().unwrap();
            s.percent = 100.0;
            s.step = "Done".into();
            s.detail.clear();
            Ok("Haiku's files are made; the game's code was built already.".into())
        }
        Err(why) => {
            log::line(format!("module: {why}: building it"));
            let said = crate::module::build(&data, &paths.work.join("module"), &paths.module_dir(), Some(&paths.amiibo()), &paths.setup, status, cancel)?;
            state.set("built", crate::module::Note::read(&paths.module_dir().join(crate::module::NOTE)).map(|n| n.stamp).unwrap_or_default());
            state.save(paths);
            let mut s = status.lock().unwrap();
            s.percent = 100.0;
            s.step = "Done".into();
            s.detail.clear();
            Ok(said)
        }
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
