//! codemods: mods that change the game's code, run natively.
//!
//! Two kinds, both reached through the same redirect (recomp.h ABI 6,
//! `Host::redirect`): a function whose stale flag is set asks the host to
//! run the code for the address it was entered at in its place.
//!
//! - **Patch modules.** A mod's exefs code patch (code.ips, code.bps, a
//!   code.bin, an exheader) is applied to the code the game loads. The
//!   functions whose bytes changed are found by the recompiled code's own
//!   origins tables (Library::check), and their patched versions, recompiled
//!   by 3dsrecomp's patch builder into a small DLL ahead of time (the
//!   frontend's haiku_codemods), run in their place. Nothing is interpreted.
//! - **Native plugins.** DLLs built against `haiku_mod.h`: hooks before and
//!   after game functions, a per-frame callback, game memory and the mod's
//!   options.
//!
//! Without code mods nothing here runs: no function is flagged, and the
//! redirect is never asked.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use recomp_abi::{Code, Context, Entry};

use crate::System;

/// a tail, recomp.h's RecompTail.
pub type Tail = unsafe extern "C" fn(*mut Context, *const u32);

/// the program's tails registry (recomp.h, CALL_THEN), which a patch
/// module's code shares through its recomp_imports, and which plugins'
/// after-hooks wait in when the original leaves for the host.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Imports {
    pub tails_pending: *mut u32,
    pub tails_counting: *mut u8,
    pub tail_defer: unsafe extern "C" fn(*mut Context, u32, u32, Tail, *const c_char, *const u32),
    pub tails_due: unsafe extern "C" fn(*mut Context),
    pub tail_count: unsafe extern "C" fn(Tail, *const c_char, i32),
}

// SAFETY: pointers into the program's own static data and code, which only
// the thread running guest code touches
unsafe impl Send for Imports {}
unsafe impl Sync for Imports {}

static IMPORTS: OnceLock<Imports> = OnceLock::new();

/// the program linking the recompiled code gives its registry, as it gives
/// recomp_tails_forget (recompiled::set_tails_forget).
pub fn set_imports(imports: Imports) {
    let _ = IMPORTS.set(imports);
}

pub(crate) fn imports() -> Option<&'static Imports> {
    IMPORTS.get()
}

/// the patched versions of the game's functions a mod changed, a DLL built
/// by 3dsrecomp's patch builder.
pub struct PatchModule {
    _library: libloading::Library,
    /// every address it has code for, bit 0 set for Thumb, sorted.
    entries: Vec<(u32, Code)>,
    /// dispatch: the same entries in a hash table, which lookup answers
    /// from in a look or two (every call into a changed function asks it).
    index: crate::dispatch::AddressMap<Code>,
    pub path: PathBuf,
    /// codemods2: the changed functions' entries the patch left no code
    /// at, sorted.
    gone: Vec<u32>,
}

// SAFETY: the code and tables are read only once loaded
unsafe impl Send for PatchModule {}
unsafe impl Sync for PatchModule {}

impl std::fmt::Debug for PatchModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PatchModule({}, {} entries)", self.path.display(), self.entries.len())
    }
}

impl PatchModule {
    /// opens the module at path, which has to be made for this ABI, by the
    /// patch builder of version, for the program's code whose tables hash to
    /// base, and from the patched code whose key is key. it gets the
    /// program's tails registry.
    pub fn open(path: &Path, version: u32, base: u64, key: u64) -> Result<PatchModule, String> {
        let imports = *imports().ok_or("the program gave no tails registry for patch modules")?;
        // SAFETY: a library the patch builder made, whose symbols have the
        // types patch.rs writes, which the checks below make sure of
        unsafe {
            let library = libloading::Library::new(path).map_err(|error| format!("could not load {}, {error}", path.display()))?;
            let symbol = |name: &[u8]| -> Result<*const c_void, String> {
                library.get::<*const c_void>(name).map(|s| *s).map_err(|_| format!("{} is no patch module ({} missing)", path.display(), String::from_utf8_lossy(name)))
            };
            let abi = *(symbol(b"recomp_patch_abi")? as *const u32);
            if abi != recomp_abi::ABI {
                return Err(format!("{} was made for version {abi} of the interface, this is {}", path.display(), recomp_abi::ABI));
            }
            let made_by = *(symbol(b"recomp_patch_version")? as *const u32);
            if made_by != version {
                return Err(format!("{} was made by another version of the patch builder", path.display()));
            }
            if *(symbol(b"recomp_patch_base")? as *const u64) != base {
                return Err(format!("{} was made for another build of Haiku", path.display()));
            }
            if *(symbol(b"recomp_patch_key")? as *const u64) != key {
                return Err(format!("{} was made from other code", path.display()));
            }
            let count = *(symbol(b"recomp_patch_count")? as *const u32) as usize;
            let table = std::slice::from_raw_parts(symbol(b"recomp_patch_entries")? as *const Entry, count);
            let mut entries: Vec<(u32, Code)> = table.iter().map(|entry| (entry.address, entry.code)).collect();
            entries.sort_unstable_by_key(|(address, _)| *address);
            *(symbol(b"recomp_imports")? as *mut Imports) = imports;
            // codemods2: the entries the patch left no code at, if it says
            let gone = match (symbol(b"recomp_patch_gone_count"), symbol(b"recomp_patch_gone")) {
                (Ok(count), Ok(table)) => {
                    let mut gone = std::slice::from_raw_parts(table as *const u32, *(count as *const u32) as usize).to_vec();
                    gone.sort_unstable();
                    gone
                }
                _ => Vec::new(),
            };
            let index = crate::dispatch::AddressMap::new(entries.iter().copied());
            Ok(PatchModule { _library: library, entries, index, path: path.to_owned(), gone })
        }
    }

    /// codemods2: whether the patch left no code at the changed function
    /// entry key (bit 0 set for Thumb), which is data now, say.
    pub fn gone(&self, key: u32) -> bool {
        self.gone.binary_search(&key).is_ok()
    }

    /// the code for address, bit 0 set for Thumb.
    #[inline]
    pub fn lookup(&self, address: u32) -> Option<Code> {
        // dispatch: a hash table, not a search of the entries
        self.index.get(address)
    }

    /// vowperf: every entry, by address, for a sampling profiler naming the
    /// code a sample fell in.
    pub fn entries(&self) -> &[(u32, Code)] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---- plugins ----------------------------------------------------------------

/// the version of haiku_mod.h this build implements.
pub const API_VERSION: u32 = 1;

/// what a hook sees: the guest's registers, the first field of the context.
#[repr(C)]
pub struct Regs {
    pub r: [u32; 16],
}

/// a hook: before or after a game function, with its registers and the
/// pointer it was registered with. a before hook answering 1 skips the
/// function (it has set r0 itself), which returns to its caller.
pub type HookFn = unsafe extern "C" fn(*mut Regs, *mut c_void) -> i32;
pub type FrameFn = unsafe extern "C" fn(*mut c_void);

/// a plugin, as haiku_mod.h's HaikuMod: its id, for its options.
#[repr(C)]
pub struct ModHandle {
    pub index: u32,
}

/// haiku_mod.h's HaikuModApi.
#[repr(C)]
pub struct Api {
    pub version: u32,
    pub size: u32,
    pub log: unsafe extern "C" fn(*const ModHandle, *const c_char),
    pub hook_before: unsafe extern "C" fn(*const ModHandle, u32, HookFn, *mut c_void) -> i32,
    pub hook_after: unsafe extern "C" fn(*const ModHandle, u32, HookFn, *mut c_void) -> i32,
    pub on_frame: unsafe extern "C" fn(*const ModHandle, FrameFn, *mut c_void) -> i32,
    pub read: unsafe extern "C" fn(u32, *mut c_void, u32) -> i32,
    pub write: unsafe extern "C" fn(u32, *const c_void, u32) -> i32,
    pub option: unsafe extern "C" fn(*const ModHandle, *const c_char, i64) -> i64,
    pub frame: unsafe extern "C" fn() -> u64,
}

/// a plugin to load: the mod's id and name, its DLL, and its options as the
/// player set them.
#[derive(Debug, Clone, Default)]
pub struct PluginSpec {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub options: BTreeMap<String, i64>,
}

#[derive(Clone, Copy)]
pub(crate) struct Hook {
    pub function: HookFn,
    pub user: usize,
    pub plugin: u32,
}

/// the hooks at one address.
#[derive(Default, Clone)]
pub(crate) struct HookSet {
    pub before: Vec<Hook>,
    pub after: Vec<Hook>,
}

/// every plugin loaded for a game, and their hooks.
#[derive(Default)]
pub struct Plugins {
    pub(crate) hooks: BTreeMap<u32, HookSet>,
    /// the keys of hooks, by index, which an after-hook's tail carries.
    pub(crate) keys: Vec<u32>,
    frames: Vec<(FrameFn, usize, u32)>,
    loaded: Vec<(PluginSpec, libloading::Library)>,
    handles: Vec<Box<ModHandle>>,
    /// what went wrong, for the Mods screen.
    pub notes: Vec<String>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plugins({} loaded, {} hooked)", self.loaded.len(), self.hooks.len())
    }
}

/// the plugins' registration while one of them initialises.
struct Registering {
    plugins: *mut Plugins,
    specs: Vec<PluginSpec>,
}

thread_local! {
    static REGISTERING: Cell<*mut Registering> = const { Cell::new(std::ptr::null_mut()) };
    /// the system a hook or a frame callback runs in, for memory access.
    static CURRENT: Cell<*mut System> = const { Cell::new(std::ptr::null_mut()) };
}

/// the specs of the plugins loading or loaded, for their options and names.
static SPECS: Mutex<Vec<PluginSpec>> = Mutex::new(Vec::new());

fn spec_name(index: u32) -> String {
    SPECS.lock().ok().and_then(|specs| specs.get(index as usize).map(|spec| spec.name.clone())).unwrap_or_else(|| format!("plugin {index}"))
}

unsafe extern "C" fn api_log(handle: *const ModHandle, message: *const c_char) {
    if message.is_null() {
        return;
    }
    let index = if handle.is_null() { u32::MAX } else { unsafe { (*handle).index } };
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    log::info!("mod {}: {text}", spec_name(index));
}

/// registers a hook while plugins load, how many there are at address now,
/// or -1 when hooks can't be added (after loading).
fn register(handle: *const ModHandle, address: u32, function: HookFn, user: *mut c_void, after: bool) -> i32 {
    let registering = REGISTERING.with(Cell::get);
    if registering.is_null() || handle.is_null() {
        return -1;
    }
    // SAFETY: set by load_plugins for the length of the init calls
    let plugins = unsafe { &mut *(*registering).plugins };
    let plugin = unsafe { (*handle).index };
    let set = plugins.hooks.entry(address).or_default();
    let hook = Hook { function, user: user as usize, plugin };
    if after {
        set.after.push(hook);
    } else {
        set.before.push(hook);
    }
    (set.before.len() + set.after.len()) as i32
}

unsafe extern "C" fn api_hook_before(handle: *const ModHandle, address: u32, function: HookFn, user: *mut c_void) -> i32 {
    register(handle, address, function, user, false)
}

unsafe extern "C" fn api_hook_after(handle: *const ModHandle, address: u32, function: HookFn, user: *mut c_void) -> i32 {
    register(handle, address, function, user, true)
}

unsafe extern "C" fn api_on_frame(handle: *const ModHandle, function: FrameFn, user: *mut c_void) -> i32 {
    let registering = REGISTERING.with(Cell::get);
    if registering.is_null() || handle.is_null() {
        return -1;
    }
    let plugins = unsafe { &mut *(*registering).plugins };
    plugins.frames.push((function, user as usize, unsafe { (*handle).index }));
    0
}

unsafe extern "C" fn api_read(address: u32, out: *mut c_void, size: u32) -> i32 {
    let system = CURRENT.with(Cell::get);
    if system.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: set while a hook or frame callback runs, on this thread
    let out = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, size as usize) };
    unsafe { (*system).memory.read_bytes(address, out) };
    1
}

unsafe extern "C" fn api_write(address: u32, data: *const c_void, size: u32) -> i32 {
    let system = CURRENT.with(Cell::get);
    if system.is_null() || data.is_null() {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) };
    unsafe { (*system).memory.write_bytes(address, data) };
    1
}

unsafe extern "C" fn api_option(handle: *const ModHandle, id: *const c_char, fallback: i64) -> i64 {
    if handle.is_null() || id.is_null() {
        return fallback;
    }
    let index = unsafe { (*handle).index } as usize;
    let id = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    SPECS.lock().ok().and_then(|specs| specs.get(index).and_then(|spec| spec.options.get(id.as_ref()).copied())).unwrap_or(fallback)
}

unsafe extern "C" fn api_frame() -> u64 {
    let system = CURRENT.with(Cell::get);
    if system.is_null() {
        return 0;
    }
    unsafe { (*system).frames }
}

static API: Api = Api {
    version: API_VERSION,
    size: std::mem::size_of::<Api>() as u32,
    log: api_log,
    hook_before: api_hook_before,
    hook_after: api_hook_after,
    on_frame: api_on_frame,
    read: api_read,
    write: api_write,
    option: api_option,
    frame: api_frame,
};

/// haiku_mod.h's entry point.
type Init = unsafe extern "C" fn(*const Api, *const ModHandle) -> i32;

/// loads the plugins and lets each register its hooks. a plugin that
/// doesn't load, is for another version of the API or fails its init is
/// left out with a note; its hooks, if it made any, go with it.
pub fn load_plugins(specs: &[PluginSpec]) -> Plugins {
    let mut plugins = Plugins::default();
    if let Ok(mut all) = SPECS.lock() {
        *all = specs.to_vec();
    }
    for (index, spec) in specs.iter().enumerate() {
        let index = index as u32;
        // SAFETY: a native plugin the player turned on runs with full trust,
        // as any DLL does; its entry point has haiku_mod.h's type
        let library = match unsafe { libloading::Library::new(&spec.path) } {
            Ok(library) => library,
            Err(error) => {
                plugins.notes.push(format!("{}: its plugin can't be loaded ({error}).", spec.name));
                continue;
            }
        };
        let init: Init = match unsafe { library.get::<Init>(b"haiku_mod_init") } {
            Ok(symbol) => *symbol,
            Err(_) => {
                plugins.notes.push(format!("{}: {} has no haiku_mod_init.", spec.name, spec.path.display()));
                continue;
            }
        };
        let wanted = unsafe { library.get::<*const u32>(b"haiku_mod_api_version") }.ok().map(|symbol| unsafe { **symbol });
        if wanted.is_some_and(|wanted| wanted > API_VERSION) {
            plugins.notes.push(format!("{}: it needs a newer Haiku (mod API {}, this is {API_VERSION}).", spec.name, wanted.unwrap_or(0)));
            continue;
        }
        let handle = Box::new(ModHandle { index });
        let before = (plugins.hooks.clone(), plugins.frames.len());
        let mut registering = Registering { plugins: &mut plugins, specs: specs.to_vec() };
        REGISTERING.with(|cell| cell.set(&mut registering));
        // SAFETY: as above
        let result = unsafe { init(&API, &*handle) };
        REGISTERING.with(|cell| cell.set(std::ptr::null_mut()));
        drop(registering);
        if result != 0 {
            plugins.hooks = before.0;
            plugins.frames.truncate(before.1);
            plugins.notes.push(format!("{}: its plugin didn't start (haiku_mod_init said {result}).", spec.name));
            continue;
        }
        log::info!("codemods: plugin {} loaded from {}", spec.name, spec.path.display());
        plugins.handles.push(handle);
        plugins.loaded.push((spec.clone(), library));
    }
    plugins.keys = plugins.hooks.keys().copied().collect();
    plugins
}

impl Plugins {
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.loaded.iter().map(|(spec, _)| spec.name.clone()).collect()
    }

    /// drops the hooks at address, which can't be hooked, with a note.
    pub(crate) fn refuse(&mut self, address: u32, why: &str) {
        if let Some(set) = self.hooks.remove(&address) {
            let mut names: Vec<String> = set.before.iter().chain(&set.after).map(|hook| spec_name(hook.plugin)).collect();
            names.dedup();
            for name in names {
                self.notes.push(format!("{name}: its hook at 0x{address:08X} can't run, {why}."));
            }
        }
        self.keys = self.hooks.keys().copied().collect();
    }
}

/// runs the plugins' frame callbacks, once a frame, before the game's code.
pub fn frame(system: &mut System) {
    let Some(library) = system.recompiled.as_ref() else { return };
    let Some(plugins) = library.plugins() else { return };
    if plugins.frames.is_empty() {
        return;
    }
    let frames: Vec<(FrameFn, usize, u32)> = plugins.frames.clone();
    let previous = CURRENT.with(|cell| cell.replace(system));
    for (function, user, _) in frames {
        // SAFETY: a plugin's callback, with the pointer it gave
        unsafe { function(user as *mut c_void) };
    }
    CURRENT.with(|cell| cell.set(previous));
}

/// runs a hook with memory access in system.
pub(crate) fn call_hook(system: *mut System, hook: &Hook, regs: *mut Regs) -> i32 {
    let previous = CURRENT.with(|cell| cell.replace(system));
    // SAFETY: a plugin's hook, with the registers of the context running it
    let result = unsafe { (hook.function)(regs, hook.user as *mut c_void) };
    CURRENT.with(|cell| cell.set(previous));
    result
}

// ---- what the loader is given -------------------------------------------------

/// what the mods turned on change of the game's code, worked out and
/// checked by the frontend before the game starts.
#[derive(Debug, Clone, Default)]
pub struct CodeMods {
    /// the game's code with the mods' patches applied, decompressed.
    pub code: Option<Arc<Vec<u8>>>,
    /// an exheader in place of the game's, its first 0x400 bytes.
    pub exheader: Option<Arc<Vec<u8>>>,
    /// the patched functions, compiled.
    pub module: Option<Arc<PatchModule>>,
    pub plugins: Vec<PluginSpec>,
    /// the mods, by name, for messages.
    pub names: Vec<String>,
    /// codemods2: the content folders of the mods whose code this is, which
    /// the loader leaves out of the game's files too when it can't use the
    /// code (a mod's files without its code would run the game's own code
    /// on the mod's data).
    pub content: Vec<std::path::PathBuf>,
}

impl CodeMods {
    pub fn is_empty(&self) -> bool {
        self.code.is_none() && self.exheader.is_none() && self.module.is_none() && self.plugins.is_empty()
    }
}

/// the bytes the code mods changed in the game's read-only code and data
/// (.text, .rodata), put back after a saved state is loaded: a state holds
/// the memory as it was saved, with or without them.
static PATCHED: Mutex<Vec<(u32, Vec<u8>)>> = Mutex::new(Vec::new());

/// the loader notes what it mapped of the code mods' changes.
pub(crate) fn set_patched(exheader: &zakuro_fs::ncch::ExHeader, original: &[u8], code: Option<&[u8]>) {
    let mut ranges = Vec::new();
    if let Some(code) = code {
        let end = ((exheader.text.num_pages + exheader.rodata.num_pages) * 0x1000) as usize;
        let mut at = 0;
        while at < end.min(code.len()) {
            if original.get(at) == code.get(at) {
                at += 1;
                continue;
            }
            let start = at;
            while at < end.min(code.len()) && original.get(at) != code.get(at) {
                at += 1;
            }
            let offset = start as u32;
            let rodata = exheader.text.num_pages * 0x1000;
            let address = if offset < rodata { exheader.text.address + offset } else { exheader.rodata.address + offset - rodata };
            ranges.push((address, code[start..at].to_vec()));
        }
    }
    if let Ok(mut patched) = PATCHED.lock() {
        *patched = ranges;
    }
}

/// after a saved state is loaded: the code mods' changes to read-only
/// memory go back in, and the recompiled code checks itself again.
pub fn after_state_load(system: &mut System) {
    let ranges = PATCHED.lock().map(|ranges| ranges.clone()).unwrap_or_default();
    if ranges.is_empty() {
        return;
    }
    for (address, bytes) in &ranges {
        let start = address & !3;
        let end = (address + bytes.len() as u32 + 3) & !3;
        for word in (start..end).step_by(4) {
            let mut current = [0u8; 4];
            system.memory.read_bytes(word, &mut current);
            for (i, byte) in current.iter_mut().enumerate() {
                let at = word + i as u32;
                if at >= *address && at < address + bytes.len() as u32 {
                    *byte = bytes[(at - address) as usize];
                }
            }
            system.memory.write32_privileged(word, u32::from_le_bytes(current));
        }
    }
    if let Some(library) = system.recompiled.as_mut() {
        let stale = library.check(&mut system.memory);
        log::info!("codemods: the code mods' changes put back after the state was loaded, {stale} functions run patched");
    }
}

/// what happened to the code mods at the last boot, for the Mods screen.
static BOOT_NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn note(text: String) {
    log::warn!("codemods: {text}");
    if let Ok(mut notes) = BOOT_NOTES.lock() {
        notes.push(text);
    }
}

pub fn take_notes() -> Vec<String> {
    BOOT_NOTES.lock().map(|mut notes| std::mem::take(&mut *notes)).unwrap_or_default()
}

/// the code mods running, by name, and the file a failure is written to,
/// so that a code mod that reaches code Haiku can't run is turned off for
/// the next start instead of stopping the game again.
static RUNNING: Mutex<(Vec<String>, Option<PathBuf>)> = Mutex::new((Vec::new(), None));

/// codemods2: the addresses of Haiku's own functions (overrides) whose
/// originals the running code mods change. where they overlap the mod's
/// code wins: a Haiku feature that would change what the mod's version of
/// the function sees or does turns itself off (replaces).
static REPLACED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

pub fn set_replaced(addresses: Vec<u32>) {
    if let Ok(mut replaced) = REPLACED.lock() {
        *replaced = addresses;
    }
}

/// codemods2: whether a running code mod changes the original of Haiku's
/// function at address (bit 0 ignored).
pub fn replaces(address: u32) -> bool {
    REPLACED.lock().is_ok_and(|replaced| replaced.iter().any(|&at| at & !1 == address & !1))
}

pub fn set_running(names: Vec<String>, failure_file: Option<PathBuf>) {
    if let Ok(mut running) = RUNNING.lock() {
        *running = (names, failure_file);
    }
}

/// called where the game stops for code it can't run: with code mods on,
/// the failure is written down for the frontend, which turns them off at
/// the next start, and the message says so. the message to add, if any.
pub fn failed(address: u32) -> Option<String> {
    let running = RUNNING.lock().ok()?;
    let (names, file) = &*running;
    if names.is_empty() {
        return None;
    }
    if let Some(file) = file {
        let text = format!("# codemods: these code mods reached code Haiku can't run at 0x{address:08X}; the Mods screen turns them off\n{}\n", names.join("\n"));
        let _ = std::fs::write(file, text);
    }
    Some(format!(
        "The code mods turned on ({}) took the game to code Haiku can't run. They are turned off for the next start; turn them on again on the Mods screen.\n",
        names.join(", ")
    ))
}

/// the bytes of a code image at an address, as the loader maps the
/// segments the exheader describes, zero outside them.
pub fn read_image(exheader: &zakuro_fs::ncch::ExHeader, code: &[u8], address: u32, out: &mut [u8]) {
    const PAGE: u32 = 0x1000;
    let segments = [
        (exheader.text, 0u32),
        (exheader.rodata, exheader.text.num_pages * PAGE),
        (exheader.data, (exheader.text.num_pages + exheader.rodata.num_pages) * PAGE),
    ];
    for (i, byte) in out.iter_mut().enumerate() {
        let at = address.wrapping_add(i as u32);
        *byte = segments
            .iter()
            .find(|(info, _)| at >= info.address && at < info.address + info.num_pages * PAGE)
            .and_then(|(info, offset)| code.get((offset + at - info.address) as usize).copied())
            .unwrap_or(0);
    }
}

/// works out what the code mods change before the game's code is mapped:
/// their exheader goes in place of the game's when it keeps the segments
/// where the recompiled code has them, and their code is checked against
/// the recompiled code: every function it changes has to have a version in
/// the patch module. anything that doesn't hold is left out with a note,
/// and the game runs its own code. the code to map and the module.
pub fn prepare(
    title: &mut zakuro_fs::Title,
    mods: &CodeMods,
    linked: Option<&recomp_abi::Linked>,
) -> (Option<Arc<Vec<u8>>>, Option<Arc<PatchModule>>) {
    let Some(code) = mods.code.clone() else {
        return (None, None);
    };
    let names = mods.names.join(", ");
    let mut exheader = title.exheader.clone();
    if let Some(bytes) = &mods.exheader {
        match zakuro_fs::ncch::ExHeader::parse(bytes) {
            Ok(mut theirs) => {
                let same = |a: zakuro_fs::ncch::CodeSetInfo, b: zakuro_fs::ncch::CodeSetInfo| a.address == b.address;
                let keeps = same(theirs.text, exheader.text) && same(theirs.rodata, exheader.rodata) && same(theirs.data, exheader.data);
                let fits = theirs.text.num_pages <= exheader.text.num_pages && theirs.rodata.num_pages <= exheader.rodata.num_pages;
                if !keeps || !fits {
                    note(format!("{names}: its exheader moves the game's code or data, which the recompiled game can't follow; the code mods are left out."));
                    return (None, None);
                }
                theirs.compress_code = exheader.compress_code;
                exheader = theirs;
            }
            Err(error) => {
                note(format!("{names}: its exheader can't be read ({error}); the code mods are left out."));
                return (None, None);
            }
        }
    }
    let Some(linked) = linked else {
        note(format!("{names}: there is no recompiled code to check them against; the code mods are left out."));
        return (None, None);
    };
    let mut library = match crate::recompiled::Library::linked(linked) {
        Ok(library) => library,
        Err(error) => {
            note(format!("{names}: {error}; the code mods are left out."));
            return (None, None);
        }
    };
    let stale = library.check_image(&exheader, &code);
    let module = mods.module.clone();
    match (&module, stale) {
        (_, 0) => {}
        (None, stale) => {
            note(format!("{names}: {stale} functions they change aren't compiled; the code mods are left out."));
            return (None, None);
        }
        (Some(module), _) => {
            if let Err(error) = library.attach_patch(module.clone()) {
                note(format!("{names}: {error}; the code mods are left out."));
                return (None, None);
            }
        }
    }
    log::info!("codemods: {names}: {stale} functions of the game's code changed, every one recompiled");
    title.exheader = exheader;
    (Some(code), module)
}
