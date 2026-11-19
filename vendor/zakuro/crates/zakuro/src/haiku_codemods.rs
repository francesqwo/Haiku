//! codemods: Haiku's code mods: exefs code patches, recompiled, and native
//! plugins. See dev/reports/codemods.md.
//!
//! - **What a code mod is.** A mod (haiku_mods) with `code.ips`, `code.bps`
//!   (IPS, IPS32 or BPS patches of the decompressed code.bin), a `code.bin`
//!   of its own or an `exheader.bin`, in its folder or its `exefs/`, as
//!   Luma3DS takes them; and/or a native plugin, `plugin.dll` (or the DLL
//!   its mod.toml names), built against `haiku_mod.h`.
//! - **Patches.** The mods' patches are applied in load order to the game's
//!   code. Where the bytes of instructions changed, the recompiled code's
//!   origins tables say which functions those are; 3dsrecomp's patch
//!   builder (src/patch.rs) recompiles them, and any new code they reach,
//!   from the patched code into a patch module, a DLL compiled with a C
//!   compiler, cached under `cache/haiku/codemods/<key>/`, so each set of
//!   mods compiles once. At boot the patched code is mapped and the changed
//!   functions run from the module (recomp.h ABI 6, Host::redirect).
//!   Patches that change only data (constants in .rodata or .data, literal
//!   pools) need no module at all: the code reads them from memory.
//! - **The compiler.** One there is (HAIKU_CC or CC, a `tools/` folder
//!   beside Haiku.exe holding Zig or w64devkit, Zig downloaded before, gcc
//!   or clang on the PATH), or else Zig, downloaded once from ziglang.org
//!   after the player says yes on the Mods screen; never silently.
//! - **Safety.** A mod whose patch can't be applied, whose code can't be
//!   recompiled (an instruction Haiku leaves to an interpreter, code it has
//!   no way to reach), or whose module doesn't load, is left out whole, with
//!   a note on the Mods screen: the game boots without it. One that takes
//!   the game to code Haiku can't run while playing is turned off for the
//!   next start (zakuro_core::codemods::failed).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use recomp3ds::compile::Compiler;
use recomp3ds::patch;
use zakuro_core::codemods::{CodeMods, PatchModule, PluginSpec};

use crate::haiku_mods::ModList;

/// the file the game writes when a code mod took it to code it can't run,
/// beside mods.toml.
pub const FAILED_FILE: &str = "codemods-failed.txt";

/// Haiku's overrides by address, the file each is in, for the notes about
/// a mod changing a function Haiku has its own version of. the program
/// gives them (source/build.rs reads overrides/).
static OVERRIDE_NAMES: Mutex<Vec<(u32, &'static str)>> = Mutex::new(Vec::new());

pub fn set_override_names(names: &[(u32, &'static str)]) {
    if let Ok(mut all) = OVERRIDE_NAMES.lock() {
        *all = names.to_vec();
    }
}

fn override_name(address: u32) -> Option<&'static str> {
    OVERRIDE_NAMES.lock().ok()?.iter().find(|(at, _)| *at == address).map(|(_, name)| *name)
}

/// a mod's code changes, as files.
#[derive(Debug, Clone, Default)]
pub struct CodeFiles {
    pub code: Option<PathBuf>,
    pub patches: Vec<PathBuf>,
    pub exheader: Option<PathBuf>,
    pub plugin: Option<PathBuf>,
}

impl CodeFiles {
    pub fn changes_code(&self) -> bool {
        self.code.is_some() || !self.patches.is_empty() || self.exheader.is_some()
    }
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()).find(|path| path.file_name().is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// the code files in a mod's content folder: in it or in its exefs/; the
/// plugin named by mod.toml (plugin) or plugin.dll.
pub fn code_files(content: &Path, plugin: Option<&str>) -> CodeFiles {
    let look = |name: &str| find_ci(&content.join("exefs"), name).or_else(|| find_ci(content, name)).filter(|path| path.is_file());
    let mut files = CodeFiles { code: look("code.bin"), exheader: look("exheader.bin"), ..CodeFiles::default() };
    files.patches.extend(look("code.ips"));
    files.patches.extend(look("code.bps"));
    files.plugin = match plugin {
        Some(name) => Some(content.join(name)).filter(|path| path.is_file()),
        None => look("plugin.dll"),
    };
    files
}

/// the game's code and exheader as the recompiled code was made from.
pub struct Game {
    pub code: Arc<Vec<u8>>,
    pub exheader: Vec<u8>,
    pub layout: zakuro_fs::ncch::ExHeader,
}

impl Game {
    pub fn load(path: &Path) -> Result<Game, String> {
        let title = zakuro_fs::Title::load(path).map_err(|error| format!("could not read {}, {error}", path.display()))?;
        let code = title.code().map_err(|error| format!("could not read the game's code, {error}"))?;
        Ok(Game { code: Arc::new(code), exheader: title.exheader_bytes().to_vec(), layout: title.exheader.clone() })
    }

    /// where an offset of a code image is in memory.
    fn address_of(&self, offset: u32) -> u32 {
        let ex = &self.layout;
        let rodata = ex.text.num_pages * 0x1000;
        let data = rodata + ex.rodata.num_pages * 0x1000;
        if offset < rodata {
            ex.text.address + offset
        } else if offset < data {
            ex.rodata.address + offset - rodata
        } else {
            ex.data.address + offset - data
        }
    }

    /// the address ranges where two code images differ.
    fn changed(&self, patched: &[u8]) -> Vec<(u32, u32)> {
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for (start, end) in patch::changed_ranges(0, &self.code, patched) {
            // cut where the segments meet, which are apart in memory
            let ex = &self.layout;
            let cuts = [ex.text.num_pages * 0x1000, (ex.text.num_pages + ex.rodata.num_pages) * 0x1000];
            let mut from = start;
            for cut in cuts.into_iter().filter(|&cut| cut > start && cut < end) {
                ranges.push((self.address_of(from), self.address_of(from) + (cut - from)));
                from = cut;
            }
            ranges.push((self.address_of(from), self.address_of(from) + (end - from)));
        }
        ranges
    }
}

/// the game's code with one mod's changes, the exheader it gives.
fn apply(game: &Game, code: &[u8], files: &CodeFiles) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let mut code = code.to_vec();
    if let Some(path) = &files.code {
        let bytes = std::fs::read(path).map_err(|error| format!("its code.bin can't be read, {error}"))?;
        let ex = &game.layout;
        let needed = ((ex.text.num_pages + ex.rodata.num_pages) * 0x1000 + ex.data.size) as usize;
        code = if bytes.len() >= needed { bytes } else { zakuro_fs::lz77::decompress(&bytes).unwrap_or(bytes) };
    }
    for path in &files.patches {
        let patch = std::fs::read(path).map_err(|error| format!("{} can't be read, {error}", name(path)))?;
        let bps = name(path).to_ascii_lowercase().ends_with(".bps");
        let applied = if bps { zakuro_fs::patch::bps(&patch, &code) } else { zakuro_fs::patch::ips(&patch, &code) };
        code = applied.map_err(|error| format!("{} doesn't apply to this version of the game ({error})", name(path)))?;
    }
    let exheader = match &files.exheader {
        Some(path) => {
            let mut bytes = std::fs::read(path).map_err(|error| format!("its exheader.bin can't be read, {error}"))?;
            if bytes.len() < 0x400 {
                return Err("its exheader.bin is too short".to_owned());
            }
            bytes.truncate(0x400);
            Some(bytes)
        }
        None => None,
    };
    Ok((code, exheader))
}

fn name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// what the code mods turned on make of the game's code.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// the mods, by index in the list, and name.
    pub mods: Vec<(usize, String)>,
    pub code: Option<Arc<Vec<u8>>>,
    pub exheader: Option<Vec<u8>>,
    pub changed: Vec<(u32, u32)>,
    /// the patched code, the base and the builder, hashed.
    pub key: u64,
    pub plugins: Vec<PluginSpec>,
    /// notes by mod index: problems (which leave it out) and conflicts.
    pub problems: BTreeMap<usize, String>,
    /// codemods2: each mod's own changes, by name, to say which mod a build
    /// failure is about.
    pub own: Vec<(String, Vec<(u32, u32)>)>,
    pub notes: BTreeMap<usize, Vec<String>>,
}

impl Plan {
    /// whether there is code to recompile and load.
    pub fn patches(&self) -> bool {
        self.code.is_some()
    }

    pub fn names(&self) -> Vec<String> {
        self.mods.iter().map(|(_, name)| name.clone()).collect()
    }

    /// codemods2: the mods whose own changes meet any of the address
    /// ranges, all of them when none does.
    pub fn blame(&self, ranges: &[(u32, u32)]) -> Vec<String> {
        let blamed: Vec<String> = self
            .own
            .iter()
            .filter(|(_, own)| own.iter().any(|&(a, b)| ranges.iter().any(|&(c, d)| a < d && c < b)))
            .map(|(name, _)| name.clone())
            .collect();
        if blamed.is_empty() { self.names() } else { blamed }
    }
}

/// codemods2: why the code mods couldn't be compiled, in plain words for
/// the Mods screen, and the mods it is about (none: all of them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Failure {
    pub text: String,
    pub mods: Vec<String>,
}

impl From<String> for Failure {
    fn from(text: String) -> Failure {
        Failure { text, mods: Vec::new() }
    }
}

impl From<&str> for Failure {
    fn from(text: &str) -> Failure {
        Failure { text: text.to_owned(), mods: Vec::new() }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl Failure {
    /// whether it is about the mod named name.
    pub fn about(&self, name: &str) -> bool {
        self.mods.is_empty() || self.mods.iter().any(|mod_name| mod_name == name)
    }
}

fn failure_path(key: u64) -> PathBuf {
    cache_root().join(format!("{key:016x}")).join("failed.toml")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FailureFile {
    text: String,
    mods: Vec<String>,
}

/// codemods2: keeps why this set of code mods couldn't be compiled, so the
/// next start says so instead of "not compiled yet".
pub fn record_failure(key: u64, failure: &Failure) {
    let path = failure_path(key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = FailureFile { text: failure.text.clone(), mods: failure.mods.clone() };
    let _ = std::fs::write(path, toml::to_string(&file).unwrap_or_default());
}

/// codemods2: why this set of code mods couldn't be compiled last time.
pub fn last_failure(key: u64) -> Option<Failure> {
    let file: FailureFile = toml::from_str(&std::fs::read_to_string(failure_path(key)).ok()?).ok()?;
    Some(Failure { text: file.text, mods: file.mods })
}

/// codemods2: the note a mod gets when its code couldn't be compiled.
pub fn failure_note(failure: &Failure) -> String {
    format!("Off: its code can't be compiled. {}", failure.text)
}

/// the base the recompiled code linked in was made from.
pub fn base(linked: &recomp3ds::abi::Linked) -> Result<patch::Base, String> {
    let library = recomp3ds::abi::Library::linked(linked)?;
    let origins = library.origins(0).ok_or("the recompiled code doesn't say what it was made from")?;
    Ok(patch::Base::from_tables(origins, library.entries()))
}

/// works out what the code mods on in list make of the game's code, in
/// load order, each applied over the ones before it. a mod whose changes
/// can't be applied is left out (problems).
pub fn plan(list: &ModList, game: &Game, base_digest: u64) -> Plan {
    let mut plan = Plan::default();
    let mut code: Vec<u8> = game.code.to_vec();
    let mut exheader: Option<Vec<u8>> = None;
    // each mod's own changes, for the overlaps
    let mut own: Vec<(usize, Vec<(u32, u32)>)> = Vec::new();
    for (index, info) in list.mods.iter().enumerate() {
        if !list.enabled[index] || !list.can_enable(index) {
            continue;
        }
        let Some(files) = &info.code_files else { continue };
        if let Some(plugin) = &files.plugin {
            plan.plugins.push(PluginSpec { id: info.id.clone(), name: info.name.clone(), path: plugin.clone(), options: list.options_of(index) });
        }
        if !files.changes_code() {
            continue;
        }
        // eur2: a code mod made for the other build of the game is refused
        if let Some(problem) = crate::haiku_mods::code_region_problem(info.title.as_deref()) {
            log::warn!("mods: {}: {problem}", info.name);
            plan.problems.insert(index, problem);
            continue;
        }
        match apply(game, &code, files) {
            Ok((patched, theirs)) => {
                if let Ok((alone, _)) = apply(game, &game.code, files) {
                    own.push((index, game.changed(&alone)));
                }
                code = patched;
                if theirs.is_some() {
                    exheader = theirs;
                }
                plan.mods.push((index, info.name.clone()));
            }
            Err(problem) => {
                plan.problems.insert(index, problem);
            }
        }
    }
    plan.own = own.iter().map(|(index, ranges)| (list.mods[*index].name.clone(), ranges.clone())).collect();
    // two code mods changing the same bytes: the later one wins
    for (i, (first, ranges)) in own.iter().enumerate() {
        for (second, others) in &own[i + 1..] {
            let shared: u32 = ranges.iter().map(|&(a, b)| others.iter().map(|&(c, d)| b.min(d).saturating_sub(a.max(c))).sum::<u32>()).sum();
            if shared > 0 {
                let them = &list.mods[*second].name;
                plan.notes.entry(*first).or_default().push(format!("{shared} bytes of its code changes are changed again by {them}, which loads after it."));
                let us = &list.mods[*first].name;
                plan.notes.entry(*second).or_default().push(format!("It changes {shared} bytes of code that {us} changes too; its changes win."));
            }
        }
    }
    if code != *game.code || exheader.is_some() {
        plan.changed = game.changed(&code);
        let mut hash = recomp3ds::abi::CodeHash::default();
        hash.add(&code);
        hash.add(exheader.as_deref().unwrap_or(&[]));
        hash.add(&base_digest.to_le_bytes());
        hash.add(&patch::VERSION.to_le_bytes());
        hash.add(&recomp3ds::abi::ABI.to_le_bytes());
        plan.key = hash.value();
        plan.code = Some(Arc::new(code));
        plan.exheader = exheader;
    }
    plan
}

/// where the compiled patch modules are kept.
pub fn cache_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("haiku")).unwrap_or_else(|| crate::haiku_mods::root().join("cache")).join("codemods")
}

pub fn module_path(key: u64) -> PathBuf {
    cache_root().join(format!("{key:016x}")).join("patch.dll")
}

/// what a build found, kept beside the module.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Built {
    pub functions: usize,
    pub stale: usize,
    pub new_instructions: usize,
    /// Haiku's own functions whose originals the mods change.
    pub replaced: Vec<u32>,
    pub missing: usize,
    pub seconds: f32,
    /// no module is needed: the patches change data only.
    pub data_only: bool,
}

fn info_path(key: u64) -> PathBuf {
    cache_root().join(format!("{key:016x}")).join("built.toml")
}

pub fn built(key: u64) -> Option<Built> {
    toml::from_str(&std::fs::read_to_string(info_path(key)).ok()?).ok()
}

/// how far a build is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Downloading(f32),
    Analysing,
    Compiling { done: usize, total: usize },
    Done,
    Failed(String),
}

impl Stage {
    pub fn fraction(&self) -> f32 {
        match self {
            Stage::Downloading(f) => *f,
            Stage::Analysing => 0.05,
            Stage::Compiling { done, total } => 0.15 + 0.85 * *done as f32 / (*total).max(1) as f32,
            Stage::Done | Stage::Failed(_) => 1.0,
        }
    }
}

/// recompiles the plan's changed functions into its patch module, unless
/// it is built already, telling stage how it goes.
pub fn build(plan: &Plan, game: &Game, base: &patch::Base, compiler: &Compiler, stage: &(dyn Fn(Stage) + Sync), cancel: &AtomicBool) -> Result<Built, Failure> {
    let code = plan.code.clone().ok_or("nothing to build")?;
    if let Some(built) = built(plan.key).filter(|built| built.data_only || module_path(plan.key).is_file()) {
        return Ok(built);
    }
    let started = Instant::now();
    stage(Stage::Analysing);
    let exheader_bytes = plan.exheader.clone().unwrap_or_else(|| game.exheader.clone());
    let exheader = recomp3ds::rom::ExHeader::read(&exheader_bytes);
    // codemods2: what the analysis is given, for 3dsrecomp's patch_plan
    // example, to look at a mod's plan away from the game
    if let Some(dir) = std::env::var_os("HAIKU_CODEMODS_DUMP").map(PathBuf::from) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("base.txt"), base.to_text());
        let _ = std::fs::write(dir.join("patched.bin"), code.as_slice());
        let _ = std::fs::write(dir.join("exheader.bin"), &exheader_bytes);
        let changed: String = plan.changed.iter().map(|(start, end)| format!("{start:08X} {end:08X}\n")).collect();
        let _ = std::fs::write(dir.join("changed.txt"), changed);
        log::info!("codemods: the analysis's input is in {}", dir.display());
    }
    let program = patch::program(&exheader, &code, &plan.changed);
    let (made, program) = patch::plan(base, program, &plan.changed);
    log::info!(
        "codemods: {} functions changed, {} to recompile, {} new instructions, {} entries with no code now, {} of Haiku's own replaced, {} dead ends, analysed in {:.1} s",
        made.stale.len(),
        made.functions.len(),
        made.new_instructions,
        made.missing.len(),
        made.replaced.len(),
        made.dead_ends.len(),
        started.elapsed().as_secs_f32()
    );
    for hole in made.holes.iter().take(20) {
        log::warn!("codemods: a hole: {:?} code at 0x{:08X} goes to 0x{:08X} ({:?}), from 0x{:08X} in the function at 0x{:08X}", hole.mode, hole.from, hole.to, hole.way, hole.from, hole.function);
    }
    if let Some(hole) = made.holes.first() {
        // codemods2: in plain words, on the mod whose code goes there
        let function = made.functions.get(&hole.function);
        let spans: Vec<(u32, u32)> = function.map(|f| f.instructions.iter().map(|&at| (at, at + 4)).collect()).unwrap_or_else(|| vec![(hole.from, hole.from + 4)]);
        let others = if made.holes.len() > 1 { format!(" (and {} more)", made.holes.len() - 1) } else { String::new() };
        return Err(Failure {
            text: format!(
                "It jumps to 0x{:08X}, where there's no game code{others}: is it for the USA Special Edition?",
                hole.to
            ),
            mods: plan.blame(&spans),
        });
    }
    let dir = cache_root().join(format!("{:016x}", plan.key));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|error| format!("could not make {}, {error}", dir.display()))?;
    let mut info = Built {
        functions: made.functions.len(),
        stale: made.stale.len(),
        new_instructions: made.new_instructions,
        replaced: made.replaced.clone(),
        missing: made.missing.len(),
        seconds: 0.0,
        data_only: made.stale.is_empty() && made.functions.is_empty(),
    };
    if !info.data_only {
        let module = patch::source(&made, &program, base, base.digest(), plan.key).map_err(|error| {
            log::warn!("codemods: {error}");
            Failure {
                text: format!(
                    "It uses {} instruction{} the PC can't run (the first at 0x{:08X}).",
                    error.count,
                    if error.count == 1 { "" } else { "s" },
                    error.first
                ),
                mods: plan.blame(&[(error.first, error.first + 4)]),
            }
        })?;
        if let Some(first) = module.left_to_host.first() {
            log::info!("codemods: {} instructions Haiku can't recompile are in code only guessed to be code, the first at 0x{first:08X}; the game stops there if it ever goes there", module.left_to_host.len());
        }
        let files = module.files;
        let source = dir.join("src");
        std::fs::create_dir_all(&source).map_err(|error| error.to_string())?;
        let mut sources = Vec::new();
        for (name, contents) in &files {
            std::fs::write(source.join(name), contents).map_err(|error| format!("could not write {name}, {error}"))?;
            if name.ends_with(".c") {
                sources.push(name.clone());
            }
        }
        let total = sources.len();
        stage(Stage::Compiling { done: 0, total });
        let progress = |done: usize, total: usize| {
            stage(Stage::Compiling { done, total });
            !cancel.load(Ordering::Relaxed)
        };
        let library = source.join("patch.dll");
        recomp3ds::compile::compile(compiler, &source, &sources, &library, &progress, true).map_err(|error| {
            if cancel.load(Ordering::Relaxed) {
                return Failure::from("Compiling was cancelled.");
            }
            log::warn!("codemods: the compiler failed: {error}");
            log::warn!("codemods: the compiler was {}", compiler.describe());
            Failure::from("The compiler failed; haiku.log says why.")
        })?;
        std::fs::rename(&library, module_path(plan.key)).map_err(|error| format!("could not keep the module, {error}"))?;
    }
    info.seconds = started.elapsed().as_secs_f32();
    let _ = std::fs::write(info_path(plan.key), toml::to_string(&info).unwrap_or_default());
    log::info!("codemods: built {} in {:.1} s", if info.data_only { "nothing (the patches change data only)".to_owned() } else { module_path(plan.key).display().to_string() }, info.seconds);
    stage(Stage::Done);
    Ok(info)
}

/// whether the plan's patch module is built (or none is needed).
pub fn is_built(plan: &Plan) -> bool {
    !plan.patches() || built(plan.key).is_some_and(|built| built.data_only || module_path(plan.key).is_file())
}

/// the compiler code mods are compiled with, when there is one: HAIKU_CC
/// or CC, a tools/ folder beside Haiku.exe (zig, or w64devkit), Zig
/// downloaded before, gcc or clang on the PATH.
pub fn compiler() -> Option<Compiler> {
    // for testing the download question: as if there were none
    if std::env::var("HAIKU_NO_COMPILER").is_ok_and(|value| value == "1") {
        return None;
    }
    if let Some(named) = std::env::var("HAIKU_CC").ok().filter(|value| !value.trim().is_empty()) {
        let mut words = named.split_whitespace();
        let program = words.next()?;
        let leading: Vec<&str> = words.collect();
        return Some(Compiler::new(program, &leading)).filter(Compiler::runs);
    }
    let tools = crate::haiku_mods::root().join("tools");
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tools) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("zig") {
                candidates.push(Compiler::new(path.join(if cfg!(windows) { "zig.exe" } else { "zig" }), &["cc"]));
            } else if name.starts_with("w64devkit") {
                candidates.push(Compiler::new(path.join("bin").join("gcc.exe"), &[]));
            }
        }
    }
    if let Some(downloaded) = crate::zig::tools_dir(None).and_then(|tools| crate::zig::installed(&tools)) {
        candidates.push(downloaded);
    }
    candidates.into_iter().find(|compiler| compiler.program.is_file() && compiler.runs()).or_else(|| Compiler::find().filter(Compiler::runs))
}

/// how big the compiler download is, in MB, none when there is none for
/// this system.
pub fn download_mb() -> Option<u64> {
    crate::zig::download_size().map(|bytes| bytes.div_ceil(1 << 20))
}

/// downloads Zig, which the player said yes to.
pub fn download(progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Compiler, String> {
    let tools = crate::zig::tools_dir(None).ok_or("there is no folder to keep the compiler in")?;
    crate::zig::download(&tools, progress, cancel)
}

/// a build on a thread of its own.
pub struct Job {
    pub stage: Arc<Mutex<Stage>>,
    pub cancel: Arc<AtomicBool>,
    pub download: bool,
    pub names: Vec<String>,
    /// codemods2: why it failed, and which mods that is about.
    pub failure: Arc<Mutex<Option<Failure>>>,
}

impl Job {
    /// builds plan's module, downloading the compiler first when compiler
    /// is none (the player said yes).
    pub fn start(plan: Plan, game: Arc<Game>, base: Arc<patch::Base>, compiler: Option<Compiler>) -> Job {
        let download = compiler.is_none();
        let stage = Arc::new(Mutex::new(if download { Stage::Downloading(0.0) } else { Stage::Analysing }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (state, stop) = (stage.clone(), cancel.clone());
        let names = plan.names();
        let names_kept = names.clone();
        let failure = Arc::new(Mutex::new(None));
        let failed = failure.clone();
        std::thread::spawn(move || {
            let set = |next: Stage| {
                if let Ok(mut stage) = state.lock() {
                    *stage = next;
                }
            };
            let compiler = match compiler {
                Some(compiler) => Ok(compiler),
                None => self::download(&|fraction| set(Stage::Downloading(fraction)), &stop),
            };
            let result = compiler.map_err(|error| {
                log::warn!("codemods: the compiler download failed: {error}");
                Failure::from("The compiler couldn't be downloaded.")
            }).and_then(|compiler| {
                let run = std::panic::AssertUnwindSafe(|| build(&plan, &game, &base, &compiler, &set, &stop));
                std::panic::catch_unwind(run).unwrap_or_else(|_| Err(Failure::from("Haiku hit a bug compiling it; haiku.log says more.")))
            });
            set(match result {
                Ok(_) => Stage::Done,
                Err(_) if stop.load(Ordering::Relaxed) => Stage::Failed("cancelled".to_owned()),
                Err(failure) => {
                    log::warn!("codemods: the code mods could not be compiled: {} (about {})", failure.text, if failure.mods.is_empty() { names.join(", ") } else { failure.mods.join(", ") });
                    record_failure(plan.key, &failure);
                    if let Ok(mut slot) = failed.lock() {
                        *slot = Some(failure.clone());
                    }
                    Stage::Failed(failure.text)
                }
            });
        });
        Job { stage, cancel, download, names: names_kept, failure }
    }

    pub fn stage(&self) -> Stage {
        self.stage.lock().map(|stage| stage.clone()).unwrap_or(Stage::Failed("lost".to_owned()))
    }
}

/// the code mods to boot with: the plan's patched code with its module,
/// which has to be built and load, and the plugins. a code mod that can't
/// is left out (the frontend leaves its files out too), with a note.
pub fn prepare(plan: &Plan, base_digest: u64) -> Result<CodeMods, String> {
    let mut mods = CodeMods { plugins: plan.plugins.clone(), names: plan.names(), ..CodeMods::default() };
    if !plan.patches() {
        return Ok(mods);
    }
    let built = built(plan.key).ok_or("not compiled yet")?;
    if !built.data_only {
        let module = PatchModule::open(&module_path(plan.key), patch::VERSION, base_digest, plan.key)?;
        mods.module = Some(Arc::new(module));
    }
    mods.code = plan.code.clone();
    mods.exheader = plan.exheader.clone().map(Arc::new);
    Ok(mods)
}

/// the notes about a built module, for the Mods screen.
pub fn built_notes(built: &Built) -> Vec<String> {
    let mut notes = Vec::new();
    if built.data_only {
        notes.push("Its patch changes only data: nothing to compile.".to_owned());
    } else {
        notes.push(format!(
            "Compiled: {} of the game's functions run patched{}.",
            built.stale,
            if built.new_instructions > 0 { format!(", {} new instructions", built.new_instructions) } else { String::new() }
        ));
    }
    if !built.replaced.is_empty() {
        // codemods2: the mod's code wins where they overlap
        let off: Vec<&str> = turned_off(&built.replaced);
        let files: std::collections::BTreeSet<&str> = built.replaced.iter().filter_map(|&at| override_name(at)).collect();
        let files: Vec<&str> = files.into_iter().collect();
        notes.push(format!(
            "It changes {} functions Haiku also hooks{}: its code runs.{}",
            built.replaced.len(),
            if files.is_empty() { String::new() } else { format!(" ({})", files.join(", ")) },
            if off.is_empty() { String::new() } else { format!(" Off while it runs: {}.", off.join(", ")) }
        ));
    }
    notes
}

/// codemods2: what happens to Haiku's own version of a function whose
/// original a code mod changes. the mod's code always runs (Haiku's
/// versions call the original, which is the mod's); a Haiku feature that
/// would change what that code sees turns off.
pub fn overlap(address: u32) -> (&'static str, Option<&'static str>) {
    match address & !1 {
        0x00420B74 => ("the title screen: Haiku's plain Fates logo is off, the mod's title shows", Some("the plain title logo")),
        0x001AE730 | 0x003D8C38 => ("the Randomizer works through it: the Randomizer is off", Some("the Randomizer")),
        0x0043F390 => ("the Main Menu: Haiku only places its Mods entry, the mod's menu runs", None),
        _ => match override_name(address) {
            Some("cheats.c") => ("a cheat works through it: the mod's code runs, and the cheat adds to it only when you turn it on", None),
            _ => ("Haiku's version runs the mod's inside it", None),
        },
    }
}

/// codemods2: the Haiku features the code mods turn off.
pub fn turned_off(replaced: &[u32]) -> Vec<&'static str> {
    let mut off: Vec<&'static str> = replaced.iter().filter_map(|&at| overlap(at).1).collect();
    off.dedup();
    off
}

/// codemods2: logs, for each of Haiku's functions the code mods change,
/// what wins.
pub fn log_overlaps(names: &[String], replaced: &[u32]) {
    for &at in replaced {
        let (what, _) = overlap(at);
        log::info!("codemods: {} changes 0x{:08X} ({}), which Haiku also hooks; {what}", names.join(", "), at & !1, override_name(at).unwrap_or("Haiku"));
    }
    let off = turned_off(replaced);
    if !off.is_empty() {
        log::warn!("codemods: Haiku features off while {} runs: {}", names.join(", "), off.join(", "));
    }
}

/// codemods2: the folder, under userdata/haiku, that holds the saves of
/// the code mods running (their ids, sorted, joined with +), none for the
/// game's own saves. A code mod changes what the game writes in its saves
/// and reads from them (FE1: 39 slots, its own units, classes and event
/// flags; its readme says its saves and the game's must never mix, the
/// Global save included), so each set of code mods has saves of its own,
/// beside the game's own folder (userdata/haiku/user), which is left
/// alone. Plugins and file-only mods use the game's own saves.
static SAVES: Mutex<Option<String>> = Mutex::new(None);

/// the folder name for a set of code mod ids: sorted, joined with +, each
/// kept to letters, digits and - _ . (anything else becomes _).
pub fn saves_folder(ids: &[String]) -> String {
    let mut ids: Vec<String> = ids
        .iter()
        .map(|id| id.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect::<String>())
        .map(|id| id.trim_matches('.').to_owned())
        .collect();
    ids.sort_by_key(|id| id.to_lowercase());
    ids.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    ids.join("+")
}

/// codemods2: the Mods screen's note on a mod that patches the code (the
/// log names the folder: userdata/haiku/user-mods/<ids>).
pub fn own_saves_note(_id: &str) -> String {
    "Uses its own saves, apart from the game's.".to_owned()
}

pub fn set_saves(ids: &[String]) {
    if let Ok(mut saves) = SAVES.lock() {
        *saves = (!ids.is_empty()).then(|| saves_folder(ids));
    }
}

/// codemods2: where the running game's saves are: data_dir/user-mods/<ids>
/// with code mods running, else none (data_dir/user, the game's own).
pub fn saves_dir(data_dir: &Path) -> Option<PathBuf> {
    SAVES.lock().ok()?.as_ref().map(|folder| data_dir.join("user-mods").join(folder))
}

/// codemods2: where the running game's save states go: beside its saves.
pub fn states_base(data_dir: &Path) -> PathBuf {
    saves_dir(data_dir).unwrap_or_else(|| data_dir.to_owned())
}

/// the code mods the last run turned off, written when the game reached
/// code it couldn't run: their names, and the address.
pub fn take_failed(data_dir: Option<&Path>) -> Option<(Vec<String>, String)> {
    let path = data_dir?.join(FAILED_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    let at = header.split_whitespace().find(|word| word.starts_with("0x")).unwrap_or("").trim_end_matches(';').to_owned();
    Some((lines.map(str::to_owned).filter(|line| !line.is_empty()).collect(), at))
}

/// where the game writes a failure.
pub fn failed_path(data_dir: Option<&Path>) -> Option<PathBuf> {
    data_dir.map(|dir| dir.join(FAILED_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// codemods2: a set of code mods has one save folder whatever their
    /// order, and an id can't leave userdata/haiku/user-mods.
    #[test]
    fn code_mods_keep_saves_by_the_sorted_set_of_ids() {
        assert_eq!(saves_folder(&["FE1".into()]), "FE1");
        assert_eq!(saves_folder(&["zeta".into(), "Alpha".into()]), saves_folder(&["Alpha".into(), "zeta".into()]));
        assert_eq!(saves_folder(&["zeta".into(), "Alpha".into()]), "Alpha+zeta");
        assert_eq!(saves_folder(&["../x y".into()]), "_x_y");
        let data = Path::new("userdata/haiku");
        set_saves(&["FE1".into()]);
        assert_eq!(saves_dir(data), Some(data.join("user-mods").join("FE1")));
        assert_eq!(states_base(data), data.join("user-mods").join("FE1"));
        set_saves(&[]);
        assert_eq!(saves_dir(data), None);
        assert_eq!(states_base(data), data.to_owned());
    }
}
