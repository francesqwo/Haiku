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
pub fn build(plan: &Plan, game: &Game, base: &patch::Base, compiler: &Compiler, stage: &(dyn Fn(Stage) + Sync), cancel: &AtomicBool) -> Result<Built, String> {
    let code = plan.code.clone().ok_or("nothing to build")?;
    if let Some(built) = built(plan.key).filter(|built| built.data_only || module_path(plan.key).is_file()) {
        return Ok(built);
    }
    let started = Instant::now();
    stage(Stage::Analysing);
    let exheader_bytes = plan.exheader.clone().unwrap_or_else(|| game.exheader.clone());
    let exheader = recomp3ds::rom::ExHeader::read(&exheader_bytes);
    let program = patch::program(&exheader, &code, &plan.changed);
    let (made, program) = patch::plan(base, program, &plan.changed);
    log::info!(
        "codemods: {} functions changed, {} to recompile, {} new instructions, {} entries with no code now, {} of Haiku's own replaced, analysed in {:.1} s",
        made.stale.len(),
        made.functions.len(),
        made.new_instructions,
        made.missing.len(),
        made.replaced.len(),
        started.elapsed().as_secs_f32()
    );
    if let Some(hole) = made.holes.first() {
        return Err(format!("the patched code goes to 0x{:08X}, where there is no code Haiku can recompile", hole & !1));
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
        let files = patch::source(&made, &program, base, base.digest(), plan.key)?;
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
        recomp3ds::compile::compile(compiler, &source, &sources, &library, &progress, true)?;
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
            let result = compiler.and_then(|compiler| {
                let run = std::panic::AssertUnwindSafe(|| build(&plan, &game, &base, &compiler, &set, &stop));
                std::panic::catch_unwind(run).unwrap_or_else(|_| Err("Haiku hit a bug compiling the code mods".to_owned()))
            });
            set(match result {
                Ok(_) => Stage::Done,
                Err(_) if stop.load(Ordering::Relaxed) => Stage::Failed("cancelled".to_owned()),
                Err(error) => {
                    log::warn!("codemods: the code mods could not be compiled: {error}");
                    Stage::Failed(error)
                }
            });
        });
        Job { stage, cancel, download, names }
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
        let names: Vec<String> = built.replaced.iter().map(|&at| match override_name(at) {
            Some(file) => format!("0x{:08X} ({file})", at & !1),
            None => format!("0x{:08X}", at & !1),
        }).collect();
        notes.push(format!("It changes {} functions Haiku has its own version of: {}. Haiku's are used.", names.len(), names.join(", ")));
    }
    notes
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
