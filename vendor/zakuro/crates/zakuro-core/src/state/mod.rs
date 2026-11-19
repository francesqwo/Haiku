//! saved states, the whole console as it was at a moment, to go back to. a
//! file holds a header, then the state of everything but memory as
//! postcard writes it, then each memory, all of them in LZ4. a state is for
//! the game it was saved from and the version of Zakuro that saved it.
//!
//! statesnd: after memory, a section of its own holds the native sound
//! layer (`pcos::snd::NativeSnd::saved`: the mixer's voices, their queues
//! and play positions), so the music and the sounds playing go on from the
//! saved moment when it is loaded, in this process or another. A state
//! from before has no such section and loads as it always did (the voices
//! start again with the game's next sound); a build from before reads the
//! five sections it knows and leaves the last one.
//!
//! statecheck: after that, when the frontend gave one (`set_tag`), a
//! section saying what the console was started with: for Haiku, the file
//! layer under the game (its mods, voices, own files, language). The
//! frontend reads it back with `tag` before loading and refuses a state
//! made with another layer. A state without it loads as before; a build
//! from before reads the sections it knows and leaves this one.

pub(crate) mod arrays;
// pulse: Divine Pulse's snapshots, kept in memory as the pages that changed
pub mod rewind;
// pulsefix: the overrides' own part of the console's state, and the check
// that a moment or a state came back whole
pub mod check;
pub mod keep;

/// pulsefix, tests: the tests that put a console back (states, moments) run
/// one at a time, since what the overrides keep (keep.rs) is the program's,
/// shared by every test of the process.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::cheats::Overwrote;
use crate::cro::CroManager;
use crate::kernel::Kernel;
use crate::memory::MapState;
use crate::services::fs::SavedFile;
use crate::services::ServiceState;
use crate::{GuestMemory, Pacing, System, HISTORY_LENGTH};

const MAGIC: &[u8; 8] = b"ZAKUROST";
/// what a state holds changes from one version to the next.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// everything but memory, as it is written.
#[derive(Serialize)]
struct SavedRef<'a> {
    cpu: &'a zakuro_cpu::Cpu,
    kernel: &'a Kernel,
    services: &'a ServiceState,
    files: Vec<(u32, SavedFile)>,
    cro: &'a CroManager,
    gpu: zakuro_gpu::state::GpuState,
    map: MapState,
    console: Console,
}

/// pulsefix (the check): the parts of a rest as `save_rest` wrote it, each
/// as a JSON value, whose maps are in key order: two equal consoles give
/// equal values, whatever order their HashMaps keep.
pub(crate) fn rest_parts(bytes: &[u8]) -> Result<Vec<(&'static str, serde_json::Value)>, String> {
    let saved: Saved = postcard::from_bytes(bytes).map_err(|error| format!("the rest could not be read, {error}"))?;
    fn value<T: Serialize>(part: &T) -> serde_json::Value {
        serde_json::to_value(part).unwrap_or_else(|error| serde_json::Value::String(format!("not comparable: {error}")))
    }
    Ok(vec![
        ("cpu", value(&saved.cpu)),
        ("kernel", value(&saved.kernel)),
        ("services", value(&saved.services)),
        ("files", value(&saved.files)),
        ("cro", value(&saved.cro)),
        ("gpu", value(&saved.gpu)),
        ("map", value(&saved.map)),
        ("console", value(&saved.console)),
    ])
}

/// the same, as it is read back.
#[derive(Deserialize)]
struct Saved {
    cpu: zakuro_cpu::Cpu,
    kernel: Kernel,
    services: ServiceState,
    files: Vec<(u32, SavedFile)>,
    cro: CroManager,
    gpu: zakuro_gpu::state::GpuState,
    map: MapState,
    console: Console,
}

/// the system's own, how far it ran and what it is due to do next.
#[derive(Serialize, Deserialize)]
struct Console {
    exited: bool,
    broke: bool,
    debug_output: String,
    fatal_errors: Vec<String>,
    lcd_force_black: bool,
    frames: u64,
    boot_clock: u64,
    next_frame_boundary: u64,
    next_audio_frame: u64,
    next_preempt: u64,
    #[serde(with = "arrays")]
    history: [u32; HISTORY_LENGTH],
    history_index: u64,
    interpreted_instructions: u64,
    recompiled_instructions: u64,
    /// what each enhancement overwrote, by its name, so that turning it
    /// off after loading puts that back too.
    enhancements: Vec<(String, BTreeMap<u32, Overwrote>)>,
}

/// the ROM images and the RomFS layers the title's files read, in the
/// order a state names them by.
type Images = (Vec<Arc<zakuro_fs::RomImage>>, Vec<Arc<zakuro_fs::layered::Layered>>);

fn images(system: &System) -> Images {
    let (mut images, mut layers) = (Vec::new(), Vec::new());
    if let Some(title) = &system.title {
        images.push(title.shared_image());
        layers.extend(title.layered.clone());
        if let Some(update) = title.update() {
            images.push(update.title.shared_image());
            layers.extend(update.title.layered.clone());
        }
    }
    images.extend(system.dlc.iter().map(|dlc| dlc.image()));
    // os: DLC installed as folders
    layers.extend(system.dlc.iter().flat_map(|dlc| dlc.folders.iter().map(|(_, layered)| layered.clone())));
    (images, layers)
}

impl System {
    /// the console as it is, between frames, as a saved state.
    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let state = self.save_rest()?;
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(VERSION.len() as u32).to_le_bytes());
        out.extend_from_slice(VERSION.as_bytes());
        out.extend_from_slice(&self.kernel.program_id.to_le_bytes());
        section(&mut out, &state);
        let phys = &self.memory.phys;
        for memory in [&phys.fcram, &phys.vram, &phys.dsp_ram, &phys.axi_wram] {
            section(&mut out, memory);
        }
        // statesnd: the native sound layer, after memory
        section(&mut out, &self.services.dsp.native.saved()?);
        // statecheck: what the console was started with, last
        let tag = TAG.lock().map(|tag| tag.clone()).unwrap_or_default();
        if !tag.is_empty() {
            let mut bytes = TAG_MAGIC.to_vec();
            bytes.extend_from_slice(&tag);
            section(&mut out, &bytes);
        }
        // pulsefix: the overrides' own state (their records of guest calls
        // under way, the guest objects they hold) and the tails waiting,
        // last: a build from before reads the sections it knows
        if keep::active() {
            section(&mut out, &keep::section(&keep::keep())?);
        }
        Ok(out)
    }

    /// pulse: everything of a saved state but memory, as postcard writes it
    /// (what the host's GPU drew comes down into memory first, so that
    /// memory as it stands goes with it). `state::rewind` keeps memory
    /// itself, as the pages that changed.
    pub fn save_rest(&mut self) -> Result<Vec<u8>, String> {
        // what the host's GPU drew comes down first, so that memory holds
        // all of it
        let linear_base = self.kernel.linear_base;
        self.gpu.write_back_all(&mut GuestMemory { memory: &mut self.memory, linear_base, writes: 0 });
        let (images, layers) = images(self);
        let mut files = Vec::with_capacity(self.services.fs.files.len());
        for (&handle, file) in &self.services.fs.files {
            let saved = file.save((&images, &layers)).ok_or_else(|| format!("the open file {} reads from no image of the game's", file.path))?;
            files.push((handle, saved));
        }
        // pulsefix: in the handles' order, not the HashMap's, so the same
        // console writes the same bytes
        files.sort_by_key(|(handle, _)| *handle);
        let console = Console {
            exited: self.exited,
            broke: self.broke,
            debug_output: self.debug_output.clone(),
            fatal_errors: self.fatal_errors.clone(),
            lcd_force_black: self.lcd_force_black,
            frames: self.frames,
            boot_clock: self.boot_clock,
            next_frame_boundary: self.next_frame_boundary,
            next_audio_frame: self.next_audio_frame,
            next_preempt: self.next_preempt,
            history: self.history,
            history_index: self.history_index as u64,
            interpreted_instructions: self.interpreted_instructions,
            recompiled_instructions: self.recompiled_instructions,
            enhancements: self
                .cheats
                .iter()
                .filter(|cheat| cheat.builtin && !cheat.overwrote.is_empty())
                .map(|cheat| (cheat.name.clone(), cheat.overwrote.clone()))
                .collect(),
        };
        let saved = SavedRef {
            cpu: &self.cpu,
            kernel: &self.kernel,
            services: &self.services,
            files,
            cro: &self.cro,
            gpu: self.gpu.save_state(),
            map: self.memory.save_map(),
            console,
        };
        postcard::to_allocvec(&saved).map_err(|error| format!("the state could not be written, {error}"))
    }

    /// puts the console back as a saved state had it. nothing changes when
    /// the state is not one this game and this Zakuro can take.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut reader = Reader(bytes);
        if reader.take(MAGIC.len())? != MAGIC {
            return Err("it is not a saved state".into());
        }
        let length = reader.u32()? as usize;
        let version = String::from_utf8_lossy(reader.take(length)?).into_owned();
        if version != VERSION {
            return Err(format!("it was saved by Zakuro {version}, and this is {VERSION}"));
        }
        if reader.u64()? != self.kernel.program_id {
            return Err("it was saved from another game".into());
        }
        let state = reader.section()?;
        let memories = [reader.section()?, reader.section()?, reader.section()?, reader.section()?];
        let phys = &self.memory.phys;
        let sizes = [phys.fcram.len(), phys.vram.len(), phys.dsp_ram.len(), phys.axi_wram.len()];
        if memories.iter().zip(sizes).any(|(memory, size)| memory.len() != size) {
            return Err("the console's memory is not the size the state has".into());
        }
        // statesnd: the native sound layer's section, when the state has one
        let sound = if reader.0.is_empty() {
            log::info!("audio: the loaded state has no sound section (saved before statesnd); the voices playing start again with the game's next sound");
            None
        } else {
            match reader.section().and_then(|bytes| crate::pcos::snd::NativeSnd::read_saved(&bytes)) {
                Ok(saved) => Some(saved),
                Err(error) => {
                    log::warn!("audio: the loaded state's sound section was left out ({error}); the voices playing start again with the game's next sound");
                    None
                }
            }
        };
        // pulsefix: the overrides' own state, after the setup tag when there
        // is one; a state from before has none, and the overrides start
        // from nothing of their own (as when the program started), never
        // from what this moment left in them
        let mut kept = None;
        while !reader.0.is_empty() {
            let Ok(bytes) = reader.section() else { break };
            if bytes.starts_with(keep::SECTION_MAGIC) {
                match keep::read_section(&bytes) {
                    Ok(read) => kept = Some(read),
                    Err(error) => log::warn!("pulsefix: the loaded state's overrides section was left out ({error})"),
                }
            }
        }
        if kept.is_none() && keep::active() {
            log::info!("pulsefix: the loaded state keeps none of the overrides' own state (saved before pulsefix); they start from nothing of their own");
        }
        self.load_rest(&state, &mut |phys| {
            for (memory, store) in memories.iter().zip([&mut phys.fcram, &mut phys.vram, &mut phys.dsp_ram, &mut phys.axi_wram]) {
                store.copy_from_slice(memory);
            }
        })?;
        // statesnd: load_rest left the native layer new; the voices come
        // back as the state had them, going on from the saved moment (the
        // sound already made for the old moment went with the services)
        if let Some(saved) = sound {
            self.services.dsp.native.put_back_saved(saved, &mut self.memory);
        }
        // pulsefix: and the overrides' own state with it
        // (FATES_STATE_HOSTKEEP=0, tests: left as it is, as before pulsefix)
        if !std::env::var_os("FATES_STATE_HOSTKEEP").is_some_and(|v| v == "0") {
            keep::put_back(&kept.unwrap_or_else(keep::initial));
        }
        crate::pcos::snd::wav::state_loaded();
        Ok(())
    }

    /// pulsefix (tests, FATES_STATE_VERIFY=1): right after `load_state(bytes)`,
    /// whether the console is the state's, everything of it (state/check.rs).
    pub fn check_loaded_state(&mut self, bytes: &[u8]) -> Result<check::Report, String> {
        let mut reader = Reader(bytes);
        reader.take(MAGIC.len())?;
        let length = reader.u32()? as usize;
        reader.take(length)?;
        reader.u64()?;
        let rest = reader.section()?;
        let memories = [reader.section()?, reader.section()?, reader.section()?, reader.section()?];
        let sound = if reader.0.is_empty() { None } else { Some(reader.section()?) };
        let mut host = None;
        while !reader.0.is_empty() {
            let section = reader.section()?;
            if section.starts_with(keep::SECTION_MAGIC) {
                host = Some(keep::read_section(&section)?);
            }
        }
        check::compare_state(self, &check::StateParts { rest, memories, sound, host })
    }

    /// pulse: the half of load_state after memory: the rest of a state as
    /// save_rest wrote it. `memory` puts memory back first, once the rest
    /// is known to be readable (`state::rewind` passes one that does
    /// nothing, having put the pages back itself). nothing changes on an
    /// error.
    pub fn load_rest(&mut self, state: &[u8], memory: &mut dyn FnMut(&mut crate::memory::PhysicalMemory)) -> Result<(), String> {
        self.idle_interrupted(true);
        let saved: Saved = postcard::from_bytes(state).map_err(|error| format!("the state could not be read, {error}"))?;
        let (images, layers) = images(self);
        let mut files = std::collections::HashMap::new();
        for (handle, file) in saved.files {
            let file = file.open((&images, &layers)).ok_or("an open file of the state reads from no image of the game's")?;
            files.insert(handle, file);
        }

        // all of it read, the console takes it
        memory(&mut self.memory.phys);
        self.memory.load_map(saved.map)?;
        self.cpu = saved.cpu;
        self.kernel = saved.kernel;
        // hooks: tails waiting in the threads this state replaces
        crate::recompiled::forget_tails(0);
        // codemods: the code mods' changes to the code, which the state's
        // memory may not have
        crate::codemods::after_state_load(self);
        let user_dir = std::mem::take(&mut self.services.fs.user_dir);
        self.services = saved.services;
        self.services.fs.user_dir = user_dir;
        self.services.fs.files = files;
        // os: a state saved in another folder keeps its saves' host paths;
        // they go to this run's user folder instead, so a loaded state never
        // writes into the folder it was saved in
        crate::pcos::fs::rebase_host_paths(self);
        // the computer's camera on while the state has a camera capturing
        crate::services::cam::capturing(self);
        // the recompiled code of the modules goes where the state has them
        if let Some(library) = &mut self.recompiled {
            for module in &self.cro.modules {
                library.place(&module.name, 0, &mut self.memory);
            }
        }
        self.cro = saved.cro;
        if let Some(library) = &mut self.recompiled {
            for module in &self.cro.modules {
                library.place(&module.name, module.base, &mut self.memory);
            }
        }
        self.gpu.load_state(&saved.gpu)?;
        let console = saved.console;
        self.exited = console.exited;
        self.broke = console.broke;
        self.debug_output = console.debug_output;
        self.fatal_errors = console.fatal_errors;
        self.lcd_force_black = console.lcd_force_black;
        self.frames = console.frames;
        self.boot_clock = console.boot_clock;
        self.next_frame_boundary = console.next_frame_boundary;
        self.next_audio_frame = console.next_audio_frame;
        self.next_preempt = console.next_preempt;
        self.history = console.history;
        self.history_index = console.history_index as usize % HISTORY_LENGTH;
        self.interpreted_instructions = console.interpreted_instructions;
        self.recompiled_instructions = console.recompiled_instructions;
        // an enhancement off now, that was on when the state was saved, puts
        // back what it overwrote at the next frame
        let mut overwrote: std::collections::HashMap<String, BTreeMap<u32, Overwrote>> = console.enhancements.into_iter().collect();
        for cheat in self.cheats.iter_mut().filter(|cheat| cheat.builtin) {
            cheat.overwrote = overwrote.remove(&cheat.name).unwrap_or_default();
        }
        // what was about to be shown, and the pace of the pictures in
        // between, start again
        self.showing = [None, None, None];
        self.pacing = Pacing::default();
        self.prefetch_aborts.clear();
        self.undefined_seen.clear();
        Ok(())
    }
}

/// statecheck: the tag section's first bytes, changed when its layout does.
const TAG_MAGIC: &[u8; 8] = b"HKSETUP1";

/// statecheck: what the frontend says the console was started with, written
/// in every state saved from now on.
static TAG: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

/// statecheck: the tag every state saved from now on carries (empty: none).
pub fn set_tag(tag: Vec<u8>) {
    if let Ok(mut current) = TAG.lock() {
        *current = tag;
    }
}

/// statecheck: a state's tag, read without loading it: none for a state
/// saved without one (every state from before statecheck). The memories
/// are skipped over, not unpacked, so this is quick.
pub fn tag(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut reader = Reader(bytes);
    if reader.take(MAGIC.len())? != MAGIC {
        return Err("it is not a saved state".into());
    }
    let length = reader.u32()? as usize;
    reader.take(length)?;
    reader.u64()?;
    // the rest, the four memories and the sound
    for _ in 0..6 {
        if reader.0.is_empty() {
            return Ok(None);
        }
        reader.skip_section()?;
    }
    // the tag, then (pulsefix) the overrides' state, which is not a tag
    while !reader.0.is_empty() {
        let section = reader.section()?;
        if section.starts_with(keep::SECTION_MAGIC) {
            continue;
        }
        return match section.strip_prefix(TAG_MAGIC.as_slice()) {
            Some(tag) => Ok(Some(tag.to_vec())),
            None => Err("its setup section is of a layout this build doesn't know".into()),
        };
    }
    Ok(None)
}

/// statecheck: the game's files as the console sees them: for the game, its
/// update and each DLC, the size of its image and, where mods or a folder
/// install laid a RomFS of its own over it, that RomFS's layout
/// (`Layered::layout_digest`: every file's name, offset and size). Two
/// consoles with the same list read every file at the same place.
pub fn file_layout(system: &System) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let layered = |layered: &Option<Arc<zakuro_fs::layered::Layered>>| layered.as_ref().map_or(0, |layered| layered.layout_digest());
    if let Some(title) = &system.title {
        out.push(("game".to_owned(), title.shared_image().len() as u64 ^ layered(&title.layered).rotate_left(1)));
        if let Some(update) = title.update() {
            out.push(("update".to_owned(), update.title.shared_image().len() as u64 ^ layered(&update.title.layered).rotate_left(1)));
        }
    }
    for dlc in &system.dlc {
        let mut digest = dlc.image().len() as u64;
        for (index, folder) in &dlc.folders {
            digest = digest.rotate_left(5) ^ (*index as u64) ^ folder.layout_digest().rotate_left(17);
        }
        out.push((format!("dlc {:016X}", dlc.title_id), digest));
    }
    out
}

/// a part of the file, how long it is in LZ4 and its bytes in LZ4.
fn section(out: &mut Vec<u8>, bytes: &[u8]) {
    let compressed = lz4_flex::compress_prepend_size(bytes);
    out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    out.extend_from_slice(&compressed);
}

/// reads a state's file from the start on.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        if self.0.len() < length {
            return Err("the state ends early".into());
        }
        let (taken, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// statecheck: past a section without unpacking it.
    fn skip_section(&mut self) -> Result<(), String> {
        let length = self.u64()?;
        self.take(usize::try_from(length).map_err(|_| "a part of the state is too long")?)?;
        Ok(())
    }

    fn section(&mut self) -> Result<Vec<u8>, String> {
        let length = self.u64()?;
        let compressed = self.take(usize::try_from(length).map_err(|_| "a part of the state is too long")?)?;
        lz4_flex::decompress_size_prepended(compressed).map_err(|error| format!("a part of the state is damaged, {error}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::sync::ResetType;
    use crate::memory::{MemoryRegion, MemoryState, Permission};
    use crate::services::tests::system_with_thread;

    /// what changes after a state is saved goes back to how it was when it
    /// is loaded, registers, memory, the memory map and the kernel's objects.
    #[test]
    fn a_state_puts_everything_back() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let page = system.memory.phys.allocate(MemoryRegion::Application, 0x2000).unwrap();
        system.memory.map(0x0800_0000, page.addr, 0x2000, Permission::RW, MemoryState::Private);
        system.memory.write_bytes(0x0800_0000, b"saved");
        system.cpu.regs[3] = 7;
        let (event, handle) = system.kernel.create_event(ResetType::OneShot, "saved");
        system.frames = 600;

        let state = system.save_state().unwrap();
        system.memory.write_bytes(0x0800_0000, b"after");
        system.memory.unmap(0x0800_1000, 0x1000);
        system.cpu.regs[3] = 9;
        system.kernel.handles.close(&mut system.kernel.objects, handle);
        system.frames = 900;

        system.load_state(&state).unwrap();
        let mut bytes = [0u8; 5];
        system.memory.read_bytes(0x0800_0000, &mut bytes);
        assert_eq!(&bytes, b"saved");
        assert!(system.memory.mapping_at(0x0800_1000).is_some());
        assert_eq!(system.cpu.regs[3], 7);
        assert_eq!(system.kernel.resolve(handle), Some(event));
        assert_eq!(system.frames, 600);
    }

    /// an enhancement on when a state was saved and off when it is loaded,
    /// in another session, puts back what it overwrote.
    #[test]
    fn a_state_keeps_what_an_enhancement_overwrote() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let page = system.memory.phys.allocate(MemoryRegion::Application, 0x1000).unwrap();
        system.memory.map(0x0800_0000, page.addr, 0x1000, Permission::RW, MemoryState::Private);
        system.memory.write_bytes(0x0800_0000, &[1]);
        let mut fps = crate::cheats::Cheat::new("60 FPS", vec!["28000000 00000000".into()], Vec::new());
        fps.builtin = true;
        fps.enabled = true;
        system.cheats = vec![fps];
        crate::cheats::run(&mut system);
        let state = system.save_state().unwrap();

        // Zakuro started again, with the enhancement off
        system.cheats[0].enabled = false;
        system.cheats[0].overwrote.clear();
        system.load_state(&state).unwrap();
        let mut byte = [9];
        system.memory.read_bytes(0x0800_0000, &mut byte);
        assert_eq!(byte, [0], "the state has what the enhancement wrote");
        crate::cheats::run(&mut system);
        system.memory.read_bytes(0x0800_0000, &mut byte);
        assert_eq!(byte, [1]);
    }

    /// a state of another game, or not a state at all, is turned down and
    /// nothing changes.
    #[test]
    fn a_state_of_another_game_is_turned_down() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let state = system.save_state().unwrap();
        system.cpu.regs[0] = 5;
        system.kernel.program_id = 0x0004_0000_0000_1234;
        assert!(system.load_state(&state).is_err());
        assert!(system.load_state(b"nothing").is_err());
        assert_eq!(system.cpu.regs[0], 5);
    }

    /// statesnd: the native sound layer comes back with a state, its voices
    /// as they were; a state from before (no sound section) still loads,
    /// with the layer new, as it always did.
    #[test]
    fn a_state_keeps_the_sound_and_one_without_it_still_loads() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let snd = &mut system.services.dsp.native;
        snd.initialized = true;
        snd.mixer.frames = 4321;
        snd.mixer.voices[5].enabled = true;
        snd.mixer.pending[7].rate = Some(0.75);
        let state = system.save_state().unwrap();
        let sound_section = {
            let mut out = Vec::new();
            super::section(&mut out, &system.services.dsp.native.saved().unwrap());
            out.len()
        };
        system.services.dsp.native = Default::default();

        system.load_state(&state).unwrap();
        let snd = &system.services.dsp.native;
        assert!(snd.initialized);
        assert_eq!(snd.mixer.frames, 4321);
        assert!(snd.mixer.voices[5].enabled);
        assert_eq!(snd.mixer.pending[7].rate, Some(0.75));

        // the same state as a build from before wrote it
        let before = &state[..state.len() - sound_section];
        system.load_state(before).unwrap();
        let snd = &system.services.dsp.native;
        assert!(!snd.initialized);
        assert_eq!(snd.mixer.frames, 0);
        // and a damaged section is left out, the rest loads
        let mut damaged = before.to_vec();
        super::section(&mut damaged, b"SNDNAT01 not postcard");
        system.load_state(&damaged).unwrap();
        assert!(!system.services.dsp.native.initialized);
    }

    /// statecheck: a state's setup tag is read back without loading it; a
    /// state without one (every state from before) has none and still
    /// loads; a state with one loads as before.
    #[test]
    fn a_state_keeps_its_setup_tag() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let saved_r2 = system.cpu.regs[2];
        let plain = system.save_state().unwrap();
        assert_eq!(super::tag(&plain).unwrap(), None);
        // the tag section as save_state writes it with a tag set (the tag
        // itself is a global, which other tests running at once must not see)
        let mut tagged = plain.clone();
        let mut bytes = super::TAG_MAGIC.to_vec();
        bytes.extend_from_slice(b"{\"mods\":1}");
        super::section(&mut tagged, &bytes);
        assert_eq!(super::tag(&tagged).unwrap().as_deref(), Some(&b"{\"mods\":1}"[..]));
        system.cpu.regs[2] = 11;
        system.load_state(&tagged).unwrap();
        assert_eq!(system.cpu.regs[2], saved_r2);
        system.load_state(&plain).unwrap();
        assert!(super::tag(b"nothing").is_err());
        // a section of another layout is an error, not a tag
        let mut other = plain.clone();
        super::section(&mut other, b"HKSETUP9 later");
        assert!(super::tag(&other).is_err());
    }
}
