//! pulse: save states, the Cheats tab's own section (Options > Cheats >
//! Save States, off by default; fates_ui/states.rs draws it).
//!
//! - Eight slots in `userdata/haiku/states/` beside the program (this
//!   install's own), `slot1.hstate` to `slot8.hstate`. Each holds a header,
//!   a thumbnail of the top screen (the app's own capture of the game's
//!   picture) and the whole console as `System::save_state` writes it.
//! - The header says what it is for: its format, the game build (the
//!   program id and a hash of the game's code), the Haiku and Zakuro
//!   versions, the randomizer's rules version and run, when it was saved and
//!   where (the chapter and turn, or My Castle). A state made by another
//!   build or a newer randomizer is refused with a message, and nothing
//!   changes.
//! - Loading puts everything back that the game and Haiku keep: the
//!   console, the randomizer's run (its bookkeeping made again from the
//!   save's marker in memory, `randomizer::state_loaded`), the cheats'
//!   marks on units (they are in the units), the pictures' pacing and the
//!   bottom screen (started over, as after any loaded state). The game's own
//!   save files are never written by a save state; the game's next real save
//!   goes where the game puts it.
//! - Never in an online battle.
//! - statecheck: the header also records the file layer the game was
//!   started with (`statecheck::Setup`: the mods, voices, Haiku's own files,
//!   the language), and a state made with another is refused, naming what
//!   changed; the menu marks such a slot. One made before this loads with a
//!   notice.
//! - Quick save and quick load (Controls: "Quick save state", "Quick load
//!   state"; F5 and F8 on the keyboard, nothing on a controller) use the
//!   Hotkey Slot, and work only while Save States is on.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zakuro_core::System;

pub const SLOTS: usize = 8;
const MAGIC: &[u8; 8] = b"HAIKUSS1";
/// what a slot's file holds changes from one format to the next
pub const FORMAT: u32 = 1;
const THUMB_W: u32 = 240;

/// a slot's header.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub format: u32,
    /// the game: its program id, and a hash of its code
    pub program_id: String,
    pub game_build: String,
    pub haiku: String,
    pub zakuro: String,
    /// the randomizer's rules this build knows, and the run on (seed,
    /// options, rules) when it was saved
    pub rando_rules: u32,
    pub rando: Option<(u32, u32, u32)>,
    pub saved_unix: u64,
    /// where: the chapter's name and turn, My Castle, or the menus
    pub place: String,
    pub cid: String,
    pub turn: Option<u16>,
    pub thumb_w: u32,
    pub thumb_h: u32,
    /// statecheck: the file layer it was made with (none: made before
    /// Haiku checked)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<crate::statecheck::Setup>,
}

/// a slot as the menu shows it.
#[derive(Clone, Debug)]
pub struct Slot {
    pub meta: Option<Meta>,
    /// the thumbnail, RGBA
    pub thumb: Option<(Vec<u8>, u32, u32)>,
    pub modified: Option<SystemTime>,
    pub size: u64,
    /// why it can't be loaded here
    pub problem: Option<String>,
    /// statecheck: it was made with another file layer: the short reason
    /// (the card's), with the whole one in `problem`
    pub mismatch: Option<String>,
    /// statecheck: made before Haiku checked the layer
    pub unchecked: bool,
}

/// statecheck: why a load didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// the state was made with another file layer: the message says what
    /// changed (a dialog, not a passing notice)
    Setup(String),
    Other(String),
}

impl LoadError {
    pub fn text(&self) -> &str {
        match self {
            LoadError::Setup(text) | LoadError::Other(text) => text,
        }
    }
}

impl From<String> for LoadError {
    fn from(text: String) -> LoadError {
        LoadError::Other(text)
    }
}

impl From<&str> for LoadError {
    fn from(text: &str) -> LoadError {
        LoadError::Other(text.to_owned())
    }
}

/// what the menu asked for, done by the app between two frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Save(usize),
    Load(usize),
    Delete(usize),
}

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    requests: Vec<Op>,
    notices: Vec<String>,
    slots: Vec<Option<Slot>>,
    /// when each slot's file was read for the menu
    read_at: Vec<Option<SystemTime>>,
    haiku: String,
    game_build: Option<String>,
    /// a load just happened: the app restarts the pacing and the sound
    loaded: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(|| State { slots: vec![None; SLOTS], read_at: vec![None; SLOTS], ..State::default() }))
}

/// the program's version, for the header (source/src/main.rs).
pub fn set_haiku_version(version: &str) {
    with(|s| s.haiku = version.to_owned());
}

/// where the slots are: userdata/haiku/states.
pub fn set_dir(data_dir: Option<&Path>) {
    with(|s| s.dir = data_dir.map(|d| d.join("states")));
}

pub fn path(slot: usize) -> Option<PathBuf> {
    with(|s| s.dir.as_ref().map(|d| d.join(format!("slot{}.hstate", slot + 1))))
}

pub fn request(op: Op) {
    with(|s| s.requests.push(op));
}

pub fn take_requests() -> Vec<Op> {
    with(|s| std::mem::take(&mut s.requests))
}

pub fn take_notice() -> Option<String> {
    with(|s| (!s.notices.is_empty()).then(|| s.notices.remove(0)))
}

pub fn push_notice(text: &str) {
    notice(text.to_owned());
}

pub fn notice(text: String) {
    log::info!("states: {text}");
    println!("states: {text}");
    with(|s| s.notices.push(text));
}

pub fn take_loaded() -> bool {
    with(|s| std::mem::take(&mut s.loaded))
}

/// a hash of the game's code (its ExeFS .code), so a state of another
/// build of the game is told apart.
fn game_build(system: &System) -> String {
    if let Some(build) = with(|s| s.game_build.clone()) {
        return build;
    }
    use sha2::Digest;
    let code = system.title.as_ref().and_then(|title| title.exefs_file("code")).unwrap_or_default();
    let digest = sha2::Sha256::digest(&code);
    let build: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    with(|s| s.game_build = Some(build.clone()));
    build
}

fn program_id(system: &System) -> String {
    system.title.as_ref().map_or_else(String::new, |title| format!("{:016X}", title.program_id()))
}

/// where the game is, for the header: the chapter and turn, My Castle, or
/// the menus.
fn place(system: &mut System) -> (String, String, Option<u16>) {
    let m = &mut system.memory;
    let situation = crate::pulse::situation(m);
    let cid = crate::pulse::chapter_cid(m);
    let castle = read32(m, zakuro_core::region::us(0x006D_1B70)) != 0 && read32(m, zakuro_core::region::us(0x006D_2A10)) != 0;
    let name = crate::pulse::chapter_name(system, &cid);
    match situation {
        Some((turn, _)) => (if name.is_empty() { cid.clone() } else { name }, cid, Some(turn)),
        None if castle => ("My Castle".to_owned(), cid, None),
        None => ("Story and menus".to_owned(), cid, None),
    }
}

fn read32(m: &mut zakuro_core::memory::Memory, a: u32) -> u32 {
    if !m.is_readable(a, 4) {
        return 0;
    }
    let mut b = [0u8; 4];
    m.read_bytes(a, &mut b);
    u32::from_le_bytes(b)
}

/// the top screen's picture, made small.
fn thumbnail(system: &mut System) -> Option<(Vec<u8>, u32, u32)> {
    let (pixels, width, height) = system.read_screen_scaled(zakuro_common::Screen::Top);
    if width == 0 || height == 0 || pixels.len() < (width * height * 4) as usize {
        return None;
    }
    let image = image::RgbaImage::from_raw(width, height, pixels.to_vec())?;
    let thumb_h = (THUMB_W * height / width).max(1);
    let small = image::imageops::resize(&image, THUMB_W, thumb_h, image::imageops::FilterType::Triangle);
    Some((small.into_raw(), THUMB_W, thumb_h))
}

fn png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    let _ = image::ImageEncoder::write_image(encoder, rgba, width, height, image::ExtendedColorType::Rgba8);
    out
}

/// saves the console to a slot. Returns how long it took.
pub fn save(system: &mut System, slot: usize) -> Result<(Duration, u64), String> {
    if zakuro_core::netplay::battle() {
        return Err("Save states can't be used in an online battle.".into());
    }
    let path = path(slot).ok_or("There is no folder for save states.")?;
    let started = Instant::now();
    let thumb = thumbnail(system);
    let (place, cid, turn) = place(system);
    let bytes = system.save_state()?;
    let (thumb_png, thumb_w, thumb_h) = match &thumb {
        Some((rgba, w, h)) => (png(rgba, *w, *h), *w, *h),
        None => (Vec::new(), 0, 0),
    };
    let meta = Meta {
        format: FORMAT,
        program_id: program_id(system),
        game_build: game_build(system),
        haiku: with(|s| s.haiku.clone()),
        zakuro: env!("CARGO_PKG_VERSION").to_owned(),
        rando_rules: zakuro_core::randomizer::plan::VERSION_CURRENT,
        rando: zakuro_core::randomizer::run_now(),
        saved_unix: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        place,
        cid,
        turn,
        thumb_w,
        thumb_h,
        setup: crate::statecheck::current(), // statecheck
    };
    let header = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(bytes.len() + header.len() + thumb_png.len() + 32);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&(thumb_png.len() as u32).to_le_bytes());
    out.extend_from_slice(&thumb_png);
    out.extend_from_slice(&bytes);
    let parent = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // written beside it first, so a state cut short never takes a whole one's place
    let partial = path.with_extension("hstate.new");
    std::fs::write(&partial, &out).and_then(|()| std::fs::rename(&partial, &path)).map_err(|e| e.to_string())?;
    let took = started.elapsed();
    with(|s| s.read_at[slot] = None);
    if let Some(at) = zakuro_core::pcos::snd::wav::position() {
        log::info!("states: slot {} saved at console frame {}, wav sample {at}", slot + 1, system.frames); // statesnd: tests
    }
    Ok((took, out.len() as u64))
}

/// reads a slot's file: its header, thumbnail and the console's state.
fn parse(bytes: &[u8]) -> Result<(Meta, Option<(Vec<u8>, u32, u32)>, &[u8]), String> {
    let bad = || "This file is not a Haiku save state.".to_owned();
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err(bad());
    }
    let mut at = 8;
    let take_u32 = |at: &mut usize| -> Result<usize, String> {
        let v = bytes.get(*at..*at + 4).ok_or_else(bad)?;
        *at += 4;
        Ok(u32::from_le_bytes(v.try_into().unwrap()) as usize)
    };
    let header_len = take_u32(&mut at)?;
    let header = bytes.get(at..at + header_len).ok_or_else(bad)?;
    at += header_len;
    let meta: Meta = serde_json::from_slice(header).map_err(|_| bad())?;
    let thumb_len = take_u32(&mut at)?;
    let thumb_png = bytes.get(at..at + thumb_len).ok_or_else(bad)?;
    at += thumb_len;
    let thumb = (!thumb_png.is_empty())
        .then(|| image::load_from_memory_with_format(thumb_png, image::ImageFormat::Png).ok())
        .flatten()
        .map(|image| {
            let rgba = image.to_rgba8();
            let (w, h) = rgba.dimensions();
            (rgba.into_raw(), w, h)
        });
    Ok((meta, thumb, &bytes[at..]))
}

/// why a state can't be loaded by this build, if it can't.
fn problem(meta: &Meta, program: &str, build: &str) -> Option<String> {
    if meta.format != FORMAT {
        return Some(format!("This save state is in format {} and this Haiku reads format {FORMAT}.", meta.format));
    }
    if !program.is_empty() && meta.program_id != program {
        return Some("This save state is of another game.".into());
    }
    if !build.is_empty() && meta.game_build != build {
        return Some("This save state was made with another version of the game's files.".into());
    }
    if meta.zakuro != env!("CARGO_PKG_VERSION") {
        return Some(format!("This save state was made by another version of Haiku (engine {}, this is {}).", meta.zakuro, env!("CARGO_PKG_VERSION")));
    }
    if meta.rando.is_some_and(|(_, _, rules)| rules > zakuro_core::randomizer::plan::VERSION_CURRENT) {
        return Some("This save state's randomized run uses newer randomizer rules than this Haiku has.".into());
    }
    None
}

/// loads a slot into the console. Nothing changes when it can't be. Whether
/// it was made before Haiku checked the file layer comes back with the
/// time it took.
pub fn load(system: &mut System, slot: usize) -> Result<(Duration, bool), LoadError> {
    if zakuro_core::netplay::battle() {
        return Err("Save states can't be used in an online battle.".into());
    }
    let path = path(slot).ok_or("There is no folder for save states.")?;
    let started = Instant::now();
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("Slot {} is empty.", slot + 1),
        _ => format!("Slot {} could not be read: {e}", slot + 1),
    })?;
    let (meta, _, state) = parse(&bytes)?;
    if let Some(problem) = problem(&meta, &program_id(system), &game_build(system)) {
        return Err(problem.into());
    }
    // statecheck: the file layer it was made with against this one
    let verdict = match &meta.setup {
        Some(setup) => crate::statecheck::verdict(Some(setup)),
        None => crate::statecheck::verdict_of_state(state),
    };
    let unchecked = verdict == crate::statecheck::Verdict::Unchecked;
    match verdict {
        crate::statecheck::Verdict::Differs(changes) => {
            let text = crate::statecheck::message(&changes);
            if !crate::statecheck::warn_only() {
                log::warn!("statecheck: slot {} refused: {text}", slot + 1);
                return Err(LoadError::Setup(text));
            }
            log::warn!("statecheck: slot {} differs, loaded anyway (HAIKU_STATE_CHECK): {text}", slot + 1);
        }
        crate::statecheck::Verdict::Unchecked => log::info!("statecheck: slot {} was made before Haiku checked the file layer; loaded as before", slot + 1),
        crate::statecheck::Verdict::Same => {}
    }
    system.load_state(state).map_err(|e| format!("The state could not be loaded: {e}."))?;
    verify_loaded(system, state, &format!("slot {}", slot + 1)); // pulsefix: tests
    // the randomizer's run as the state's save has it
    zakuro_core::randomizer::state_loaded(system);
    crate::pulse::state_loaded();
    with(|s| s.loaded = true);
    if let Some(at) = zakuro_core::pcos::snd::wav::position() {
        log::info!("states: slot {} loaded, console frame {}, wav sample {at}", slot + 1, system.frames); // statesnd: tests
    }
    Ok((started.elapsed(), unchecked))
}

/// pulsefix, tests (FATES_STATE_VERIFY=1): right after a state was loaded,
/// everything of the console compared with the state's file: memory page by
/// page, the rest (CPU, kernel, services, files, GPU, memory map), the
/// sound, and the overrides' own state (zakuro-core state/check.rs).
pub fn verify_loaded(system: &mut System, bytes: &[u8], what: &str) {
    if !std::env::var_os("FATES_STATE_VERIFY").is_some_and(|v| v == "1") {
        return;
    }
    match system.check_loaded_state(bytes) {
        Ok(report) => println!("states: check {what}: {report}"),
        Err(error) => println!("states: check {what}: could not compare, {error}"),
    }
}

pub fn delete(slot: usize) -> Result<(), String> {
    let path = path(slot).ok_or("There is no folder for save states.")?;
    match std::fs::remove_file(&path) {
        Ok(()) => {
            with(|s| s.read_at[slot] = None);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// the slots as the menu shows them, read again when their files changed.
pub fn slots(program: &str, build: &str) -> Vec<Option<Slot>> {
    let paths: Vec<Option<PathBuf>> = (0..SLOTS).map(path).collect();
    for (slot, path) in paths.iter().enumerate() {
        let modified = path.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
        let stale = with(|s| s.read_at[slot] != modified || (modified.is_some() && s.slots[slot].is_none()));
        if !stale {
            continue;
        }
        let read = modified.and(path.as_ref()).and_then(|p| {
            let size = std::fs::metadata(p).map_or(0, |m| m.len());
            // only the header and thumbnail are wanted: the start of the file
            let bytes = read_start(p).ok()?;
            Some(match parse_head(&bytes) {
                Ok((meta, thumb)) => {
                    let problem = problem(&meta, program, build);
                    Slot { meta: Some(meta), thumb, modified, size, problem, mismatch: None, unchecked: false }
                }
                Err(problem) => Slot { meta: None, thumb: None, modified, size, problem: Some(problem), mismatch: None, unchecked: false },
            })
        });
        with(|s| {
            s.slots[slot] = read;
            s.read_at[slot] = modified;
        });
    }
    // statecheck: each against the layer running now (it changes with a
    // restart, the file doesn't)
    let mut slots = with(|s| s.slots.clone());
    for slot in slots.iter_mut().flatten() {
        let Some(meta) = &slot.meta else { continue };
        if slot.problem.is_some() {
            continue;
        }
        match crate::statecheck::verdict(meta.setup.as_ref()) {
            crate::statecheck::Verdict::Differs(changes) => {
                slot.problem = Some(crate::statecheck::message(&changes));
                slot.mismatch = Some(crate::statecheck::short(&changes));
            }
            crate::statecheck::Verdict::Unchecked => slot.unchecked = true,
            crate::statecheck::Verdict::Same => {}
        }
    }
    slots
}

fn read_start(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut head = vec![0u8; 12];
    file.read_exact(&mut head)?;
    let header_len = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    let mut rest = vec![0u8; header_len.min(1 << 20) + 4];
    file.read_exact(&mut rest)?;
    let thumb_len = u32::from_le_bytes(rest[rest.len() - 4..].try_into().unwrap()) as usize;
    let mut thumb = vec![0u8; thumb_len.min(8 << 20)];
    file.read_exact(&mut thumb)?;
    head.extend_from_slice(&rest);
    head.extend_from_slice(&thumb);
    Ok(head)
}

fn parse_head(bytes: &[u8]) -> Result<(Meta, Option<(Vec<u8>, u32, u32)>), String> {
    parse(bytes).map(|(meta, thumb, _)| (meta, thumb))
}

/// the game's identity, for the menu's checks.
pub fn identity(system: &System) -> (String, String) {
    (program_id(system), game_build(system))
}

/// a time as the menu shows it: "2026-11-16 21:30".
pub fn when(unix: u64) -> String {
    let local = chrono_like(unix);
    local
}

fn chrono_like(unix: u64) -> String {
    // the local time zone's offset, from the system
    let offset = local_offset_secs();
    let t = unix as i64 + offset;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
}

/// days since 1970-01-01 as a date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: [u16; 8],
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: [u16; 8],
        daylight_bias: i32,
    }
    extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
    }
    // SAFETY: a plain struct the call fills in
    unsafe {
        let mut info: TimeZoneInformation = std::mem::zeroed();
        let kind = GetTimeZoneInformation(&mut info);
        let bias = info.bias + if kind == 2 { info.daylight_bias } else { info.standard_bias };
        -(bias as i64) * 60
    }
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

impl crate::App {
    /// pulse: the save states' work between two frames: what the menu
    /// asked for, and the quick keys.
    pub(crate) fn states_poll(&mut self) {
        let on = self.settings.pc.save_states;
        let slot = self.settings.pc.save_state_slot.min(SLOTS as u32 - 1) as usize;
        // the keys are taken whatever happens, so a press never waits
        let quick_save = crate::input_api::take_action(crate::controls::PcAction::QuickSave);
        let quick_load = crate::input_api::take_action(crate::controls::PcAction::QuickLoad);
        let mut ops = take_requests();
        if on && quick_save {
            ops.push(Op::Save(slot));
        }
        if on && quick_load {
            ops.push(Op::Load(slot));
        }
        let Some(game) = &mut self.game else { return };
        crate::fates_ui::states::set_identity(identity(&game.system));
        let mut loaded = false;
        // pace: only a save, load or delete that ran moves the schedule; this
        // runs on every pass, and clamping the schedule each time took away
        // the game's catching up (it ran 2-8% slow, its sound stretched)
        let worked = !ops.is_empty();
        // statesnd, tests: where FATES_OUTPUT_WAV's recording was at each op
        let output_at = |audio: &Option<crate::audio::Audio>, what: &str| {
            if let Some(at) = audio.as_ref().and_then(|audio| audio.recorded()) {
                log::info!("states: {what} at output sample {at}");
            }
        };
        for op in ops {
            match op {
                Op::Save(_) => output_at(&self.audio, "save"),
                Op::Load(_) => output_at(&self.audio, "load"),
                Op::Delete(_) => {}
            }
            match op {
                Op::Save(slot) => match save(&mut game.system, slot) {
                    Ok((took, bytes)) => notice(format!("Saved to slot {} ({:.1} MB, {} ms)", slot + 1, bytes as f64 / 1e6, took.as_millis())),
                    Err(error) => notice(error),
                },
                Op::Load(slot) => match load(&mut game.system, slot) {
                    Ok((took, unchecked)) => {
                        game.frame = game.system.frames;
                        // statecheck: a state from before says so instead
                        if unchecked {
                            notice(crate::statecheck::UNCHECKED.to_owned());
                        } else {
                            notice(format!("Loaded slot {} ({} ms)", slot + 1, took.as_millis()));
                        }
                        loaded = true;
                    }
                    // statecheck: what changed, in the game's message box
                    // (or the Save States page's own while it is open)
                    Err(LoadError::Setup(text)) => {
                        println!("states: {text}");
                        if self.fates.options.open {
                            crate::fates_ui::states::show_info(text);
                        } else {
                            self.menus.message = Some(text);
                        }
                    }
                    Err(LoadError::Other(error)) => notice(error),
                },
                Op::Delete(slot) => match delete(slot) {
                    Ok(()) => notice(format!("Slot {} deleted", slot + 1)),
                    Err(error) => notice(error),
                },
            }
        }
        let pulse_loaded = take_loaded();
        if loaded || pulse_loaded {
            // as after any loaded state: the pictures' pacing and the sound
            // start over, the menus close on the game as it now is
            self.fates.options.hide();
            self.overlay_corner = None;
            self.hfr_apply();
            self.hfr.reset();
            // statesnd: the old moment's queued sound goes, the loaded
            // one's fades in (its voices came back with the state)
            if let Some(audio) = &self.audio {
                audio.state_loaded();
            }
        }
        // the time it took is not the game's to make up
        if worked || loaded || pulse_loaded {
            self.next_frame = std::time::Instant::now().max(self.next_frame);
        }
    }
}

/// statesnd, tests: where in the FATES_AUDIO_WAV recording a save or load
/// happened, for comparing the sound after it.
fn wav_at() -> String {
    zakuro_core::pcos::snd::wav::position().map_or_else(String::new, |at| format!(", wav sample {at}"))
}

/// tests: `FATES_STATES_AT="<frame>:save <n>;<frame>:load <n>;..."` on a
/// headless run (slots counted from 1), `FATES_STATES_DIR` for the folder.
pub fn headless_frame(system: &mut System, frame: u64) -> bool {
    static SCRIPT: Mutex<Option<Vec<(u64, String)>>> = Mutex::new(None);
    let due: Vec<String> = {
        let mut guard = SCRIPT.lock().unwrap_or_else(|e| e.into_inner());
        let list = guard.get_or_insert_with(|| {
            std::env::var("FATES_STATES_AT")
                .unwrap_or_default()
                .split(';')
                .filter_map(|part| {
                    let (at, command) = part.split_once(':')?;
                    Some((at.trim().parse().ok()?, command.trim().to_owned()))
                })
                .collect()
        });
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(list).into_iter().partition(|(at, _)| *at <= frame);
        *list = later;
        now.into_iter().map(|(_, c)| c).collect()
    };
    if due.is_empty() {
        return false;
    }
    if let Some(dir) = std::env::var_os("FATES_STATES_DIR") {
        with(|s| s.dir = Some(PathBuf::from(dir)));
    }
    let mut loaded = false;
    for command in due {
        let (verb, rest) = command.split_once(' ').unwrap_or((command.as_str(), "1"));
        let slot = rest.trim().parse::<usize>().unwrap_or(1).clamp(1, SLOTS) - 1;
        match verb {
            "save" => match save(system, slot) {
                Ok((took, bytes)) => println!("states: frame {frame} saved slot {} ({bytes} bytes) in {took:.1?}{}", slot + 1, wav_at()),
                Err(error) => println!("states: frame {frame} could not save: {error}"),
            },
            "load" => match load(system, slot) {
                Ok((took, unchecked)) => {
                    println!("states: frame {frame} loaded slot {} in {took:.1?}, at console frame {}{}", slot + 1, system.frames, wav_at());
                    if unchecked {
                        println!("states: notice: {}", crate::statecheck::UNCHECKED); // statecheck
                    }
                    loaded = true;
                }
                Err(error) => println!("states: frame {frame} could not load: {}{}", if matches!(error, LoadError::Setup(_)) { "refused: " } else { "" }, error.text()),
            },
            "delete" => println!("states: frame {frame} delete slot {}: {:?}", slot + 1, delete(slot)),
            "list" => {
                let (program, build) = identity(system);
                for (i, slot) in slots(&program, &build).iter().enumerate() {
                    match slot {
                        Some(slot) => println!(
                            "states: slot {}: {:?} {} bytes, problem {:?}, mismatch {:?}, unchecked {}, thumbnail {:?}",
                            i + 1,
                            slot.meta.as_ref().map(|m| (&m.place, m.turn, when(m.saved_unix), &m.haiku, &m.zakuro, m.rando)),
                            slot.size,
                            slot.problem,
                            slot.mismatch,
                            slot.unchecked,
                            slot.thumb.as_ref().map(|t| (t.1, t.2))
                        ),
                        None => println!("states: slot {}: empty", i + 1),
                    }
                }
            }
            other => println!("states: unknown command {other}"),
        }
    }
    with(|s| s.loaded = false);
    loaded
}
