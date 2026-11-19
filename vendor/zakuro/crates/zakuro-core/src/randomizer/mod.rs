//! randomizer: a randomized run, chosen when a new game starts and kept in
//! its saves.
//!
//! The game's data is never changed on disk. The randomizer works on the
//! game's memory, through the overrides in overrides/randomizer.c:
//!
//! - when a randomized run starts or a randomized save loads, the playable
//!   characters of GameData's person table (already in memory since boot)
//!   take their new look, class and stats; when the title screen comes back
//!   or an ordinary save loads, they are put back exactly as loaded;
//! - each chapter's own person file (enemy Xander, guest Hinoka, Garon in a
//!   cutscene) is changed the same way right after the game loads it;
//! - talk scenes: the portrait and name plate a line asks for ($Wm<name>)
//!   and its voice clip ($Svp<clip>) are those of whoever is shown in that
//!   character's place;
//! - units placed by a chapter (its dispos) get the class the plan gives
//!   them and weapons they can use.
//!
//! The seed and options live in four of the save's script variables that
//! the game never names (GameUserData's VariableManager, values 0 to 3;
//! the game names only the last four of its 48), so a save carries its
//! randomization, through Copy too, and an ordinary save has zeros there.

pub mod badge;
pub mod catalog;
// corrinswap: Corrin as a character
pub mod corrin;
// rando4: children's hair colours
pub mod hair;
// rando4: lines re-wrapped after names and words change; test helpers
pub mod audit;
pub mod wrap;
pub mod plan;
// rando3: the plan's round-3 rules, map sprites, names and pronouns in lines
pub mod plan2;
pub mod rom;
pub mod sprites;
pub mod text;
// lilith: a weapon for each unit of the run its class can use
pub mod weapons;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use recomp_abi::Context;
use zakuro_cpu::Bus;

use crate::memory::{Memory, MemoryRegion, MemoryState, Permission};
use crate::System;
use catalog::{Catalog, PERSON_SIZE};
use plan::Plan;

/// the calls overrides/randomizer.c makes
pub mod op {
    pub const TITLE: u32 = 1;
    pub const NEW_GAME: u32 = 2;
    pub const SAVE_LOADED: u32 = 3;
    pub const SAVE_WRITE: u32 = 4;
    pub const PERSON_LOADED: u32 = 5;
    pub const FID: u32 = 6;
    pub const VOICE: u32 = 7;
    pub const DISPOS_UNIT: u32 = 8;
    pub const TEXT: u32 = 9;
    pub const CAST: u32 = 10;
    /// a cutscene clip the shown model has no file for (a), played as b
    pub const CLIP: u32 = 11;
    /// rando3: a map sprite is set up for class a (JID text) and character b
    /// (AID text); answers the character to draw instead (0: as asked)
    pub const SPRITE: u32 = 12;
    /// rando3: a save slot of the save list is set up (a: its
    /// ChapterMenuItem); answers the mode badge's new text, or 0
    pub const BADGE: u32 = 13;
    /// rando3: a talk line before the game expands it (a: the expander, b:
    /// the line); answers the rewritten copy, or 0
    pub const TEXT_LINE: u32 = 14;
    /// rando3: a unit is made (a: its person, b: the class asked for);
    /// answers the class to use, or 0
    pub const UNIT_JOB: u32 = 15;
    /// rando3: a dispos entry's items for a unit that exists (a: the entry)
    pub const DISPOS_ITEMS: u32 = 16;
    /// corrinswap (corrin.rs): a function drawing a character from a unit
    /// (a: the unit); answers a copy to draw instead, or 0
    pub const UNIT_LOOK: u32 = 17;
    /// corrinswap: UnitIcon::SetIcon from a person and an edit (the context)
    pub const ICON_LOOK: u32 = 18;
    /// corrinswap: FaceInstance::LoadByUnitEdit (a: the edit); answers a FID
    /// to load instead, or 0
    pub const FACE_EDIT: u32 = 19;
    /// corrinswap: FaceData::GetMessName (a: the face data); answers the
    /// name, or 0
    pub const MESS_NAME: u32 = 20;
    /// corrinswap: Person::GetName (the context); 0, 1 (answered) or 2
    /// (ask the person in r0)
    pub const PERSON_NAME: u32 = 21;
    /// corrinswap: the avatar's unit for a talk window shown as Corrin
    pub const ALIAS: u32 = 22;
    /// corrinswap: the avatar's creator opens (a = 1) or closes (a = 0)
    pub const CREATOR: u32 = 23;
    /// rando4: FaceDataManager::GetHairColorFromPerson (a = the person)
    pub const HAIR: u32 = 24;
    /// rando4: after BattleUnitResName::PostProcess (a = the unit, b = the
    /// person)
    pub const HAIR_MODEL: u32 = 25;
    /// the New Game screens: see menu.rs
    pub const MENU_FIRST: u32 = 0x40;
    pub const MENU_LAST: u32 = 0x7F;
}

/// game addresses (USA Special Edition v2)
const PERSON_LIST: u32 = 0x006D_3B28; // Person's list of loaded tables
const GAME_USER_DATA: u32 = 0x006D_A248; // GameUserData*
const TALK_VOICE: u32 = 0x129E; // ProcTalkManager: the clip the next line plays, char[0x40]

/// where the strings of story characters' FIDs and models go in the guest:
/// a page the randomizer maps for itself when a run starts (never in an
/// ordinary run)
const POOL_ADDRESSES: [u32; 4] = [0x0E00_0000, 0x0E80_0000, 0x0F00_0000, 0x0C00_0000];
const POOL_SIZE: u32 = 0x10000;

const MAGIC: u32 = 0x4B48_5A52; // "RZHK"

pub mod menu;

/// what the player chose for a new game, or what a save asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub seed: u32,
    pub options: u32,
    /// rando3: the plan's rules (plan::VERSION_*), kept in the save
    pub version: u32,
}

struct Active {
    choice: Choice,
    plan: Plan,
    base: u32,
    table: u32,
    /// person entries in guest memory -> the slot (identity) they are
    pool: HashMap<u32, usize>,
    /// English names said in talk lines -> who says them now, longest first
    names: Vec<(Vec<u16>, Vec<u16>)>,
    /// rando3: who the talk lines know (names, genders, families), for the
    /// names and the gendered words said about a swapped character
    cast: text::Cast,
    /// rando3: Corrin's GameData entries (male, female) when Corrin's class
    /// is randomized
    corrin_at: Vec<(u32, bool)>,
    /// corrinswap: the Corrin swap the talk lines' cast was made for
    cast_pair: Option<(usize, usize)>,
    /// rando4: each child's hair colour (empty without plan::HAIR)
    hair: hair::Colours,
}

#[derive(Default)]
struct State {
    system: usize,
    /// the catalog of the run that is on (or the last one loaded)
    catalog: Option<Arc<Catalog>>,
    /// lilith: the catalogs read so far, by the pool they have (a run
    /// before rules version 3 has the first rounds' characters only)
    catalogs: HashMap<u32, Arc<Catalog>>,
    catalog_failed: bool,
    active: Option<Active>,
    /// the choice the New Game screens made, used when the game starts
    pending: Option<Choice>,
    /// the seed the New Game screens show
    menu_seed: u32,
    pool_at: u32,
    pool_used: u32,
    strings: HashMap<Vec<u8>, u32>,
    /// chapter person entries changed, and the FID pointer each was given
    patched: HashMap<u32, u32>,
    /// stand-in persons for cutscene casts named after a swapped character:
    /// (who, class) -> guest address
    stand_ins: HashMap<(usize, u16), u32>,
    casts_changed: u32,
    /// cutscene clips played as another one, logged once each
    clips_logged: std::collections::HashSet<(u32, u32)>,
    voices_changed: u32,
    faces_changed: u32,
    names_changed: u32,
    /// rando3: gendered words changed in talk lines
    words_changed: u32,
    /// rando3: where a rewritten talk line goes (in the randomizer's page)
    line_buffer: u32,
    /// rando3: save files read for the save list's mark: path -> (modified,
    /// the run it holds)
    badge_cache: HashMap<std::path::PathBuf, (Option<std::time::SystemTime>, Option<(u32, u32, u32)>)>,
    /// rando3: map sprites drawn with the class's own head, logged once each
    sprites_logged: std::collections::HashSet<(String, String)>,
    /// corrinswap: unit copies in the randomizer's page (corrin.rs)
    copies: corrin::Copies,
    /// corrinswap: the avatar's unit, once found
    avatar_at: u32,
    /// corrinswap: the avatar's creator is open
    creator: bool,
    /// corrinswap: what was logged once
    corrin_logged: std::collections::HashSet<String>,
    /// rando4: the talk font's widths, for re-wrapping lines (wrap.rs)
    font: Option<Arc<wrap::Font>>,
    font_failed: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// online2: the randomized run the game is in now (a randomized save is
/// loaded), if any. Online battles count it as a gameplay mod: both players
/// need the same one (the characters' table differs in one).
pub fn current() -> Option<Choice> {
    STATE.lock().ok()?.as_ref()?.active.as_ref().map(|active| active.choice)
}
static ACTIVE_WORD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SEED_SETTING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// the program gives the word overrides/randomizer.c reads to know whether
/// a run is randomized (so ordinary runs never call here for talk lines).
///
/// # Safety
///
/// word has to stay valid for the program's life.
pub unsafe fn set_active_word(word: *mut u32) {
    ACTIVE_WORD.store(word as usize, std::sync::atomic::Ordering::Relaxed);
    // tests: FATES_RANDO_STATE arms the hooks from the start, so a state
    // from an ordinary run is randomized from its first talk line or unit
    if std::env::var_os("FATES_RANDO_STATE").is_some() && forced().is_some() {
        ARMED.store(true, std::sync::atomic::Ordering::Relaxed);
        set_active(false);
    }
}

static ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// the seed the player set in the settings (randomizer_seed), used for new
/// runs instead of a random one; None for a random seed each time.
pub fn set_seed_setting(seed: Option<u32>) {
    SEED_SETTING.store(seed.map_or(0, |s| s as u64 | 1 << 32), std::sync::atomic::Ordering::Relaxed);
}

pub fn seed_setting() -> Option<u32> {
    let v = SEED_SETTING.load(std::sync::atomic::Ordering::Relaxed);
    (v >> 32 != 0).then_some(v as u32)
}

fn set_active(on: bool) {
    set_active_bits(on, false, false);
}

/// corrinswap: with Corrin swapped, the word also has corrin::ACTIVE_BIT
/// rando4: with children's hair randomized, also hair::ACTIVE_BIT
fn set_active_bits(on: bool, corrin: bool, hair_on: bool) {
    let word = ACTIVE_WORD.load(std::sync::atomic::Ordering::Relaxed) as *mut u32;
    if !word.is_null() {
        // SAFETY: see set_active_word
        let armed = ARMED.load(std::sync::atomic::Ordering::Relaxed);
        // tests: armed with a forced choice that swaps Corrin, the Corrin
        // hooks call too, so a state is randomized from its first name or face
        let armed_corrin = armed && forced().is_some_and(|c| c.options & plan::CORRIN != 0);
        let armed_hair = armed && forced().is_some_and(|c| c.options & plan::HAIR != 0);
        let value = if on {
            1 | if corrin { corrin::ACTIVE_BIT } else { 0 } | if hair_on { hair::ACTIVE_BIT } else { 0 }
        } else if armed {
            2 | if armed_corrin { corrin::ACTIVE_BIT } else { 0 } | if armed_hair { hair::ACTIVE_BIT } else { 0 }
        } else {
            0
        };
        unsafe { std::ptr::write_volatile(word, value) };
    }
}

/// the seed as the game shows it, 8 hex digits in two groups.
pub fn seed_text(seed: u32) -> String {
    format!("{:04X}-{:04X}", seed >> 16, seed & 0xFFFF)
}

pub fn parse_seed(text: &str) -> Option<u32> {
    let digits: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (!digits.is_empty() && digits.len() <= 8).then(|| u32::from_str_radix(&digits, 16).ok()).flatten()
}

/// FATES_RANDO=<seed>[:<options>] starts every new game randomized, for
/// tests that run without the New Game screens.
fn forced() -> Option<Choice> {
    let text = std::env::var("FATES_RANDO").ok()?;
    let (seed, options) = match text.split_once(':') {
        Some((s, o)) => (s, u32::from_str_radix(o.trim_start_matches("0x"), 16).ok()?),
        None => (text.as_str(), plan::DEFAULT_OPTIONS),
    };
    // FATES_RANDO_V=1: the first rounds' rules (an old save's run)
    let version = std::env::var("FATES_RANDO_V").ok().and_then(|v| v.parse().ok()).unwrap_or(plan::VERSION_CURRENT);
    Some(Choice { seed: parse_seed(seed)?, options, version })
}

// -- guest memory -----------------------------------------------------------

fn r8(m: &mut Memory, a: u32) -> u8 {
    m.read8(a)
}
fn r16(m: &mut Memory, a: u32) -> u16 {
    m.read16(a)
}
fn r32(m: &mut Memory, a: u32) -> u32 {
    m.read32(a)
}
fn w8(m: &mut Memory, a: u32, v: u8) {
    m.write8(a, v)
}
fn w16(m: &mut Memory, a: u32, v: u16) {
    m.write16(a, v)
}
fn w32(m: &mut Memory, a: u32, v: u32) {
    m.write32(a, v)
}

fn cstr(m: &mut Memory, a: u32, max: usize) -> Vec<u8> {
    if a == 0 || !m.is_readable(a, 1) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..max as u32 {
        let b = m.read8(a + i);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

fn write_cstr(m: &mut Memory, a: u32, text: &[u8], room: usize) {
    let n = text.len().min(room.saturating_sub(1));
    m.write_bytes(a, &text[..n]);
    m.write8(a + n as u32, 0);
}

// -- the hook ---------------------------------------------------------------

/// pulse: a save state (the Cheats tab's Save States) was loaded: the run
/// its save's marker names comes on, or the randomizer goes off for a state
/// of an ordinary run, as a save loaded from the title would have it. The
/// state's memory already has the run's tables; activating writes them
/// from the ROM's originals and the plan again, which gives the same bytes,
/// and gives the host the run's bookkeeping (the talk lines' cast, the
/// pool of names) that a new process would not have.
pub fn state_loaded(system: &mut System) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(State::default);
    let key = system as *mut System as usize;
    // everything the host kept about the memory before (its page of
    // strings, the chapter entries it changed) is of another memory now;
    // only the catalogs read from the ROM and the font stay
    let fresh = State {
        system: key,
        catalog: state.catalog.take(),
        catalogs: std::mem::take(&mut state.catalogs),
        catalog_failed: state.catalog_failed,
        font: state.font.take(),
        font_failed: state.font_failed,
        ..State::default()
    };
    *state = fresh;
    set_active(false);
    if let Some(choice) = user_data(system).and_then(|gud| read_marker(system, gud)) {
        activate(state, system, choice, "a loaded save state");
    } else {
        log::info!("randomizer: off (a loaded save state of an ordinary run)");
    }
}

/// pulse: the randomized run on now (seed, options, rules), for a save
/// state's header.
pub fn run_now() -> Option<(u32, u32, u32)> {
    current().map(|c| (c.seed, c.options, c.version))
}


/// the hook the overrides call (haiku_rando_hook).
///
/// # Safety
///
/// ctx has to be the context of a run of recompiled code, which the
/// overrides are only ever given.
pub unsafe extern "C" fn hook(ctx: *mut std::ffi::c_void, op: u32, a: u32, b: u32) -> u32 {
    // SAFETY: as above, the system is the one the code runs in
    let system = unsafe { &mut *crate::recompiled::system_of(ctx as *mut Context) };
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(State::default);
    let key = system as *mut System as usize;
    if state.system != key {
        // a new system (first call, or the game restarted in-process)
        *state = State { system: key, ..State::default() };
        set_active(false);
    }
    if state.active.is_none() && ARMED.load(std::sync::atomic::Ordering::Relaxed) && matches!(op, op::FID | op::VOICE | op::DISPOS_UNIT | op::TEXT | op::CAST | op::TEXT_LINE | op::UNIT_LOOK | op::ICON_LOOK | op::FACE_EDIT | op::MESS_NAME | op::PERSON_NAME | op::ALIAS | op::HAIR | op::HAIR_MODEL) {
        if let Some(choice) = forced() {
            activate(state, system, choice, "FATES_RANDO_STATE");
        }
    }
    match op {
        op::TITLE => {
            state.creator = false;
            deactivate(state, system, "the title screen");
            0
        }
        op::NEW_GAME => {
            let choice = state.pending.take().or_else(forced);
            // rando4: every new game that randomizes characters randomizes
            // children's hair too (not classes-only)
            // (tests: FATES_RANDO_NOHAIR=1 starts one as a game made before)
            let hair = std::env::var_os("FATES_RANDO_NOHAIR").is_none();
            let choice = choice.map(|c| if hair && c.options & plan::CHARACTERS != 0 { Choice { options: c.options | plan::HAIR, ..c } } else { c });
            match choice {
                Some(choice) if choice.options & (plan::CHARACTERS | plan::CLASSES) != 0 => {
                    activate(state, system, choice, "a new game");
                    if let Some(gud) = user_data(system) {
                        write_marker(state, system, gud);
                    }
                }
                _ => deactivate(state, system, "a new game without the randomizer"),
            }
            0
        }
        op::SAVE_LOADED => {
            match read_marker(system, a) {
                Some(choice) => activate(state, system, choice, "a randomized save"),
                None => deactivate(state, system, "an ordinary save"),
            }
            0
        }
        op::SAVE_WRITE => {
            write_marker(state, system, a);
            0
        }
        op::PERSON_LOADED => {
            // a state loaded in the middle of a randomized run: its save's
            // marker is in memory
            if let Some(gud) = user_data(system) {
                if let Some(choice) = read_marker(system, gud) {
                    if state.active.as_ref().map(|a| a.choice) != Some(choice) {
                        activate(state, system, choice, "the loaded state");
                    }
                } else if state.active.is_none() && std::env::var_os("FATES_RANDO_STATE").is_some() {
                    // tests: a state from an ordinary run, randomized from
                    // its next chapter file on (FATES_RANDO_STATE=1 with
                    // FATES_RANDO=<seed>)
                    if let Some(choice) = forced() {
                        activate(state, system, choice, "FATES_RANDO_STATE");
                    }
                }
            }
            if state.active.is_some() {
                patch_tables(state, system, false);
                hair::refresh_units(state, system);
                weapons::ensure(state, system, "a chapter file");
                if let Some(gud) = user_data(system) {
                    write_marker(state, system, gud);
                }
            }
            0
        }
        op::FID => {
            remap_fid(state, system, a, b);
            0
        }
        op::VOICE => {
            remap_voice(state, system, a + TALK_VOICE);
            0
        }
        op::DISPOS_UNIT => {
            dispos_unit(state, system, a);
            weapons::ensure(state, system, "a unit placed");
            0
        }
        op::CAST => {
            // SAFETY: the context the override was given, as above
            cast(state, system, unsafe { &mut *(ctx as *mut Context) });
            0
        }
        op::CLIP => {
            if state.clips_logged.len() < 200 && state.clips_logged.insert((a, b)) {
                log::debug!("randomizer: cutscene clip {a} has no file for the shown model; played clip {b}");
            }
            0
        }
        op::TEXT => {
            rename_in_text(state, system, a);
            0
        }
        op::SPRITE => {
            weapons::ensure(state, system, "a map sprite");
            sprite(state, system, a, b)
        }
        op::BADGE => save_badge(state, system, a),
        op::TEXT_LINE => text_line(state, system, b),
        op::UNIT_JOB => unit_job(state, system, a, b),
        op::DISPOS_ITEMS => {
            corrin_items(state, system, a);
            weapons::ensure(state, system, "a unit's items");
            0
        }
        op::UNIT_LOOK => {
            let copy = corrin::unit_look(state, system, a, if b == 0 { "a name" } else { "a portrait or sprite" });
            // tests: FATES_RANDO_CORRINLOG=1 logs every unit drawn and from where
            if std::env::var_os("FATES_RANDO_CORRINLOG").is_some() {
                // SAFETY: the context the override was given, as above
                let lr = unsafe { (*(ctx as *mut Context)).r[14] };
                log::info!("randomizer: unit look {a:#010x} ({b}) from {lr:#010x}: {copy:#010x}");
            }
            copy
        }
        op::ICON_LOOK => {
            // SAFETY: the context the override was given, as above
            corrin::icon_look(state, system, unsafe { &mut *(ctx as *mut Context) });
            0
        }
        op::FACE_EDIT => corrin::face_edit(state, system, a),
        op::MESS_NAME => corrin::mess_name(state, system, a),
        // SAFETY: the context the override was given, as above
        op::PERSON_NAME => corrin::person_name(state, system, unsafe { &mut *(ctx as *mut Context) }),
        op::ALIAS => corrin::alias(state, system),
        op::HAIR => hair::hair(state, system, a),
        op::HAIR_MODEL => hair::model(state, system, a, b),
        op::CREATOR => {
            state.creator = a != 0;
            log::debug!("randomizer: the avatar's creator {}", if a != 0 { "opens" } else { "closes" });
            0
        }
        menu::SHOWN => {
            menu::shown(a);
            0
        }
        menu::POINTER => menu::pick(),
        menu::STEP => {
            let shown = menu::step_shown();
            log::info!("randomizer: New Game, a mode chosen: {}", if shown { "the Normal / Randomizer step" } else { "Normal (a replay without the step)" });
            shown as u32
        }
        op::MENU_FIRST..=op::MENU_LAST => {
            if !map_pool(state, system) {
                return 0;
            }
            menu::call(state, system, op, a, b)
        }
        _ => 0,
    }
}

fn user_data(system: &mut System) -> Option<u32> {
    let gud = r32(&mut system.memory, crate::region::us(GAME_USER_DATA));
    (gud != 0 && system.memory.is_readable(gud, 0x40)).then_some(gud)
}

/// GameUserData's script values: (address of value 0, count).
fn variables(system: &mut System, gud: u32) -> Option<u32> {
    let m = &mut system.memory;
    let vm = r32(m, gud + 0x20);
    if vm == 0 || !m.is_readable(vm, 16) {
        return None;
    }
    let count = r32(m, vm + 8);
    let values = r32(m, vm + 0xC);
    (count >= 8 && values != 0 && m.is_writable(values, 16)).then_some(values)
}

fn read_marker(system: &mut System, gud: u32) -> Option<Choice> {
    let values = variables(system, gud)?;
    let m = &mut system.memory;
    let (magic, seed, options, check) = (r32(m, values), r32(m, values + 4), r32(m, values + 8), r32(m, values + 12));
    let version = options >> 16;
    (magic == MAGIC && check == (MAGIC ^ seed ^ options) && (plan::VERSION_FIRST..=plan::VERSION_CURRENT).contains(&version))
        .then_some(Choice { seed, options: options & 0xFFFF, version })
}

fn write_marker(state: &State, system: &mut System, gud: u32) {
    let Some(active) = &state.active else { return };
    let Some(values) = variables(system, gud) else { return };
    let options = active.choice.options | active.choice.version << 16;
    let seed = active.choice.seed;
    let m = &mut system.memory;
    w32(m, values, MAGIC);
    w32(m, values + 4, seed);
    w32(m, values + 8, options);
    w32(m, values + 12, MAGIC ^ seed ^ options);
}

// -- turning a run on and off -----------------------------------------------

/// the catalog for runs with the rules `version` (lilith: from version 3
/// on, everyone is in the pool; an older run's catalog is the one it was
/// planned with), which becomes the current one
fn load_catalog(state: &mut State, system: &mut System, version: u32) -> Option<Arc<Catalog>> {
    let pool = if version >= catalog::SINCE_DLC_STORY {
        catalog::SINCE_DLC_STORY
    } else if version >= catalog::SINCE_NO_LAYLA {
        catalog::SINCE_NO_LAYLA
    } else if version >= catalog::SINCE_ALL {
        catalog::SINCE_ALL
    } else {
        1
    };
    if let Some(cat) = state.catalogs.get(&pool) {
        state.catalog = Some(cat.clone());
        return Some(cat.clone());
    }
    if state.catalog_failed {
        return None;
    }
    let rom = system.title.as_ref().and_then(rom::Rom::of);
    // jpn2: the DLC's contents, for the Japanese Special Edition's Revelation
    // (read only when the game has no person files of its own for it)
    let dlc: Vec<rom::Rom> = if version >= catalog::SINCE_DLC_STORY {
        system.dlc.iter().flat_map(|d| d.contents.iter().map(|c| c.index).chain(d.folders.iter().map(|(i, _)| *i)).collect::<std::collections::BTreeSet<u16>>().into_iter().filter_map(move |i| rom::Rom::of_dlc(d, i))).collect()
    } else {
        Vec::new()
    };
    match rom.map(|rom| Catalog::load_with(&rom, &dlc, version)) {
        Some(Ok(cat)) => {
            let cat = Arc::new(cat);
            state.catalogs.insert(pool, cat.clone());
            state.catalog = Some(cat.clone());
            Some(cat)
        }
        Some(Err(error)) => {
            log::error!("randomizer: {error}; the run stays as it is");
            state.catalog_failed = true;
            None
        }
        None => {
            log::error!("randomizer: the game's files could not be read; the run stays as it is");
            state.catalog_failed = true;
            None
        }
    }
}

/// GameData's person table in guest memory, and where its data starts.
fn gamedata_in_memory(cat: &Catalog, system: &mut System) -> Option<(u32, u32)> {
    let m = &mut system.memory;
    let node = r32(m, crate::region::us(PERSON_LIST));
    if node == 0 || !m.is_readable(node, 12) {
        return None;
    }
    let table = r32(m, node);
    let base = table.wrapping_sub(cat.person_table);
    // the first character's PID pointer is where the file says it is
    let first = cat.persons.get(3)?;
    let expected = base.wrapping_add(cat.gamedata.u32(first.offset + 8));
    (m.is_readable(table, 0x10 + PERSON_SIZE * 4) && r32(m, base.wrapping_add(first.offset) + 8) == expected).then_some((base, table))
}

/// an entry of GameData's person table as the game loaded it.
fn original_entry(cat: &Catalog, base: u32, offset: u32) -> Vec<u8> {
    let mut bytes = cat.gamedata.bytes(offset, PERSON_SIZE as usize).to_vec();
    for k in (0..PERSON_SIZE).step_by(4) {
        if cat.gamedata.pointers.contains(&(offset + k)) {
            let v = cat.gamedata.u32(offset + k).wrapping_add(base);
            bytes[k as usize..k as usize + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    bytes
}

fn restore_globals(cat: &Catalog, system: &mut System, base: u32) {
    for id in &cat.identities {
        if let Some(index) = id.global {
            let offset = cat.persons.iter().find(|p| p.index == index).map(|p| p.offset).unwrap_or(0);
            let bytes = original_entry(cat, base, offset);
            system.memory.write_bytes(base.wrapping_add(offset), &bytes);
        }
    }
    // rando3: Corrin's entries (their class)
    for p in cat.persons.iter().filter(|p| p.index == 1 || p.index == 2) {
        let bytes = original_entry(cat, base, p.offset);
        system.memory.write_bytes(base.wrapping_add(p.offset), &bytes);
    }
}

fn deactivate(state: &mut State, system: &mut System, why: &str) {
    set_active(false);
    let Some(active) = state.active.take() else { return };
    if let Some(cat) = state.catalog.clone() {
        if gamedata_in_memory(&cat, system).is_some_and(|(base, _)| base == active.base) {
            restore_globals(&cat, system, active.base);
        }
    }
    log::info!("randomizer: off ({why}); the characters are back as the game has them");
}

/// the guest address of a string, in GameData when it is there, else in
/// the randomizer's own page.
fn string_at(state: &mut State, system: &mut System, text: &[u8]) -> Option<u32> {
    if let Some(&at) = state.strings.get(text) {
        return Some(at);
    }
    let need = text.len() as u32 + 1;
    if state.pool_at == 0 || state.pool_used + need > POOL_SIZE {
        return None;
    }
    let at = state.pool_at + state.pool_used;
    write_cstr(&mut system.memory, at, text, need as usize);
    state.pool_used = (state.pool_used + need + 3) & !3;
    state.strings.insert(text.to_vec(), at);
    Some(at)
}

/// a UTF-16 string for the game's menus, in the randomizer's page.
fn wstring_at(state: &mut State, system: &mut System, text: &str) -> Option<u32> {
    let mut bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    bytes.extend_from_slice(&[0, 0]);
    let mut key = vec![0xFFu8, b'w'];
    key.extend_from_slice(&bytes);
    if let Some(&at) = state.strings.get(&key) {
        return Some(at);
    }
    let need = bytes.len() as u32;
    if state.pool_at == 0 || state.pool_used + need > POOL_SIZE {
        return None;
    }
    let at = state.pool_at + state.pool_used;
    system.memory.write_bytes(at, &bytes);
    state.pool_used = (state.pool_used + need + 3) & !3;
    state.strings.insert(key, at);
    Some(at)
}

fn map_pool(state: &mut State, system: &mut System) -> bool {
    if state.pool_at != 0 && system.memory.mapping_at(state.pool_at).is_some() {
        return true;
    }
    state.pool_at = 0;
    state.pool_used = 0;
    state.strings.clear();
    state.line_buffer = 0;
    state.stand_ins.clear();
    state.copies.clear();
    let Some(&vaddr) = POOL_ADDRESSES.iter().find(|&&a| (0..POOL_SIZE).step_by(0x1000).all(|o| system.memory.mapping_at(a + o).is_none())) else {
        log::error!("randomizer: no free guest addresses for its strings");
        return false;
    };
    let block = [MemoryRegion::Base, MemoryRegion::System, MemoryRegion::Application]
        .into_iter()
        .find_map(|region| system.memory.phys.allocate_top(region, POOL_SIZE));
    let Some(block) = block else {
        log::error!("randomizer: no memory for its strings");
        return false;
    };
    system.memory.map(vaddr, block.addr, POOL_SIZE, Permission::READ | Permission::WRITE, MemoryState::Private);
    system.memory.write_bytes(vaddr, &vec![0u8; POOL_SIZE as usize]);
    state.pool_at = vaddr;
    true
}

/// guest addresses of an identity's FID, AID, MPID and help.
fn identity_strings(state: &mut State, system: &mut System, cat: &Catalog, base: u32, who: usize) -> (u32, u32, u32, u32) {
    let id = &cat.identities[who];
    if let Some(index) = id.global {
        if let Some(p) = cat.persons.iter().find(|p| p.index == index) {
            let field = |k: u32| if cat.gamedata.ptr(p.offset + k).is_some() { base.wrapping_add(cat.gamedata.u32(p.offset + k)) } else { 0 };
            return (field(0xC), field(0x10), field(0x14), field(0x18));
        }
    }
    let fid = string_at(state, system, &id.fid).unwrap_or(0);
    let aid = id.aid.clone().and_then(|aid| string_at(state, system, &aid)).unwrap_or(0);
    let mpid = string_at(state, system, &id.mpid).unwrap_or(0);
    let help = id.mpid_h.clone().and_then(|h| string_at(state, system, &h)).unwrap_or(0);
    (fid, aid, mpid, help)
}

fn activate(state: &mut State, system: &mut System, choice: Choice, why: &str) {
    if state.active.as_ref().is_some_and(|a| a.choice == choice) {
        return;
    }
    // codemods2: a code mod that changes the functions the randomizer hooks
    // (who a unit is, what it carries) has its own cast: its code wins
    if crate::codemods::replaces(0x001AE730) || crate::codemods::replaces(0x003D8C38) {
        log::warn!("randomizer: off, a code mod changes the functions it works through; the run stays as the mod makes it");
        return;
    }
    if state.active.is_some() {
        deactivate(state, system, "another randomized save");
    }
    let Some(cat) = load_catalog(state, system, choice.version) else { return };
    let Some((base, table)) = gamedata_in_memory(&cat, system) else {
        log::error!("randomizer: GameData isn't where it should be in memory; the run stays as it is");
        return;
    };
    if !map_pool(state, system) {
        return;
    }
    let choice = wanted_seed(&cat, choice);
    let plan = Plan::make_version(&cat, choice.seed, choice.options, choice.version);
    let mut pool = HashMap::new();
    for g in &plan.globals {
        let Some(p) = cat.persons.iter().find(|p| p.index == g.index) else { continue };
        let at = base.wrapping_add(p.offset);
        let original = original_entry(&cat, base, p.offset);
        system.memory.write_bytes(at, &original);
        let slot = cat.identities.iter().position(|id| id.global == Some(g.index)).unwrap_or(0);
        pool.insert(at, slot);
        let (fid, aid, mpid, help) = identity_strings(state, system, &cat, base, g.identity);
        let shown = &cat.identities[g.identity];
        let m = &mut system.memory;
        if plan.options & plan::CHARACTERS != 0 {
            w32(m, at + 0xC, fid);
            w32(m, at + 0x10, aid);
            w32(m, at + 0x14, mpid);
            if help != 0 {
                w32(m, at + 0x18, help);
            }
            let flags = r8(m, at);
            w8(m, at, (flags & !1) | shown.female as u8);
            w8(m, at + 0x87, shown.body);
            m.write_bytes(at + 0x38, &g.bases.map(|b| b as u8));
            m.write_bytes(at + 0x40, &g.growths);
            m.write_bytes(at + 0x48, &g.modifiers.map(|b| b as u8));
            m.write_bytes(at + 0x74, &g.pskills);
            m.write_bytes(at + 0x7A, &g.birthday);
        }
        w16(m, at + 0x2C, g.class);
        w16(m, at + 0x2E, g.class2);
        m.write_bytes(at + 0x60, &g.wexp);
        w16(m, at + 0x7C, g.reclass[0]);
        w16(m, at + 0x7E, g.reclass[1]);
    }
    // rando3: Corrin's class (both of Corrin's entries, one per gender)
    let mut corrin_at = Vec::new();
    if let Some(corrin) = plan.corrin {
        for p in cat.persons.iter().filter(|p| p.index == 1 || p.index == 2) {
            let at = base.wrapping_add(p.offset);
            let female = p.index == 2;
            let (class, promoted) = corrin[female as usize];
            system_write_class(&mut system.memory, &cat, at, class, promoted, &p.wexp);
            corrin_at.push((at, female));
        }
    }
    log::info!(
        "randomizer: on ({why}), seed {}, options {:#x}, rules v{}: {} characters shown in other places",
        seed_text(choice.seed),
        choice.options,
        choice.version,
        plan.shown.iter().filter(|(a, b)| a != b).count()
    );
    for line in &plan.spoiler {
        log::debug!("randomizer:   {line}");
    }
    // rando4: children's hair colours
    let hair = if choice.options & plan::HAIR != 0 { hair::colours(&cat, choice.seed) } else { hair::Colours::new() };
    let hair_lines = if hair.is_empty() { Vec::new() } else { hair::spoiler(&cat, &hair) };
    for line in &hair_lines {
        log::debug!("randomizer:   {line}");
    }
    write_spoiler(&plan, &hair_lines);
    let mut names: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
    if choice.options & plan::CHARACTERS != 0 {
        for (&slot, &who) in &plan.shown {
            let (from, to) = (&cat.identities[slot].name, &cat.identities[who].name);
            if slot != who && from.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && !names.iter().any(|(f, _)| *f == from.encode_utf16().collect::<Vec<_>>()) {
                names.push((from.encode_utf16().collect(), to.encode_utf16().collect()));
            }
        }
        names.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
    }
    let cast = if choice.options & plan::CHARACTERS != 0 { make_cast(&cat, &plan, None) } else { text::Cast::default() };
    if let Some(path) = std::env::var_os("FATES_RANDO_CASTDUMP") {
        audit::dump_cast(&cast, &path);
    }
    let corrin_on = plan.corrin_swap.is_some();
    let hair_on = !hair.is_empty();
    state.active = Some(Active { choice, plan, base, table, pool, names, cast, corrin_at, cast_pair: None, hair });
    state.patched.clear();
    state.stand_ins.clear();
    set_active_bits(true, corrin_on, hair_on);
    hair::refresh_units(state, system);
    // chapter files already in memory (a randomized save loaded in a
    // chapter, a test state)
    patch_tables(state, system, true);
}

/// tests: FATES_RANDO_WANT="Garon=Sakura,..." looks from the seed on for
/// the first seed that shows those characters in those places.
fn wanted_seed(cat: &Catalog, choice: Choice) -> Choice {
    let Ok(want) = std::env::var("FATES_RANDO_WANT") else { return choice };
    // corrinswap: "Corrin" names Corrin's place (Ryoma=Corrin: Ryoma's place
    // is shown as Corrin; Corrin=Sakura), for a male Corrin unless
    // FATES_RANDO_FEMALE is set
    const CORRIN: usize = usize::MAX;
    let female = std::env::var_os("FATES_RANDO_FEMALE").is_some();
    // lilith: a portrait key (カンナ女) names one of two of the same name
    let index = |name: &str| {
        if name.trim().eq_ignore_ascii_case("corrin") {
            Some(CORRIN)
        } else {
            cat.identities.iter().position(|i| i.key_text == name.trim()).or_else(|| cat.identities.iter().position(|i| i.name.eq_ignore_ascii_case(name.trim())))
        }
    };
    let pairs: Vec<(usize, usize)> = want.split(',').filter_map(|p| p.split_once('=')).filter_map(|(a, b)| Some((index(a)?, index(b)?))).collect();
    if pairs.is_empty() {
        return choice;
    }
    let fits = |plan: &Plan, slot: usize, who: usize| match (slot, who) {
        (CORRIN, w) => plan.corrin_pair(female).is_some_and(|(_, y)| y == w),
        (s, CORRIN) => plan.corrin_pair(female).is_some_and(|(p, _)| p == s),
        (s, w) => plan.shown(s) == w,
    };
    for n in 0..200_000u32 {
        let seed = choice.seed.wrapping_add(n);
        let plan = Plan::make_version(cat, seed, choice.options, choice.version);
        if pairs.iter().all(|&(slot, who)| fits(&plan, slot, who)) {
            log::info!("randomizer: seed {} has {want}", seed_text(seed));
            return Choice { seed, ..choice };
        }
    }
    log::warn!("randomizer: no seed has {want}");
    choice
}

/// userdata/haiku/randomizer/<seed>.txt: who is in whose place, for the
/// player who wants to know (or share) a run.
fn write_spoiler(plan: &Plan, extra: &[String]) {
    let Some(dir) = std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("haiku").join("randomizer")) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let mut text = format!(
        "Fire Emblem Fates randomizer, seed {} (options {:#x}, rules v{})\ncharacter  -> shown as   class\n",
        seed_text(plan.seed),
        plan.options,
        plan.version
    );
    for line in plan.spoiler.iter().chain(extra) {
        text.push_str(line);
        text.push('\n');
    }
    let _ = std::fs::write(dir.join(format!("{}.txt", seed_text(plan.seed))), text);
}

// -- chapters ---------------------------------------------------------------

/// the person tables Person's list holds, after GameData's own.
fn loaded_tables(system: &mut System) -> Vec<u32> {
    let mut tables = Vec::new();
    let mut node = r32(&mut system.memory, crate::region::us(PERSON_LIST));
    let mut guard = 0;
    while node != 0 && guard < 64 && system.memory.is_readable(node, 12) {
        if guard > 0 {
            tables.push(r32(&mut system.memory, node));
        }
        node = r32(&mut system.memory, node + 8);
        guard += 1;
    }
    tables
}

/// the chapter's person file Person::Load just appended (the last table),
/// or every loaded one when a run starts with chapter files in memory.
fn patch_tables(state: &mut State, system: &mut System, all: bool) {
    let tables = loaded_tables(system);
    let chosen: Vec<u32> = if all { tables } else { tables.last().copied().into_iter().collect() };
    for table in chosen {
        patch_table(state, system, table);
    }
}

fn patch_table(state: &mut State, system: &mut System, table: u32) {
    let Some(cat) = state.catalog.clone() else { return };
    let (base, global_table) = match &state.active {
        Some(a) => (a.base, a.table),
        None => return,
    };
    if table == global_table || table == 0 || !system.memory.is_readable(table, 0x10) {
        return;
    }
    let count = r16(&mut system.memory, table + 4) as u32;
    let mut changed = 0;
    for i in 0..count.min(512) {
        let at = table + 0x10 + i * PERSON_SIZE;
        let fid_ptr = r32(&mut system.memory, at + 0xC);
        if state.patched.get(&at) == Some(&fid_ptr) {
            // changed already (the same table, seen again)
            continue;
        }
        let fid = cstr(&mut system.memory, fid_ptr, 64);
        let slot = fid.strip_prefix(b"FID_".as_slice()).and_then(|k| slot_of_key(&cat, k));
        let Some(active) = state.active.as_mut() else { return };
        // lilith: a place the run leaves alone stays exactly as loaded
        let slot = slot.filter(|&s| active.plan.touches(s));
        let Some(slot) = slot else {
            active.pool.remove(&at);
            continue;
        };
        active.pool.insert(at, slot);
        let shown = active.plan.shown(slot);
        let options = active.plan.options;
        let class = r16(&mut system.memory, at + 0x2C);
        let class2 = r16(&mut system.memory, at + 0x2E);
        let new_class = active.plan.chapter_class(&cat, slot, class);
        let new_class2 = active.plan.chapter_class(&cat, slot, class2);
        if options & plan::CHARACTERS != 0 && shown != slot {
            let (fid, aid, mpid, help) = identity_strings(state, system, &cat, base, shown);
            let id = &cat.identities[shown];
            let m = &mut system.memory;
            w32(m, at + 0xC, fid);
            if aid != 0 {
                w32(m, at + 0x10, aid);
            }
            w32(m, at + 0x14, mpid);
            if help != 0 && r32(m, at + 0x18) != 0 {
                w32(m, at + 0x18, help);
            }
            let flags = r8(m, at);
            w8(m, at, (flags & !1) | id.female as u8);
            if r8(m, at + 0x87) != 0xFF {
                w8(m, at + 0x87, id.body);
            }
        }
        let m = &mut system.memory;
        let written = r32(m, at + 0xC);
        state.patched.insert(at, written);
        w16(m, at + 0x2C, new_class);
        w16(m, at + 0x2E, new_class2);
        if let Some(job) = cat.job(new_class) {
            let mut wexp = [0u8; 8];
            m.read_bytes(at + 0x60, &mut wexp);
            let best = *wexp.iter().max().unwrap_or(&0);
            let mut new = [0u8; 8];
            for k in 0..8 {
                if job.wexp[k] > 0 {
                    new[k] = best.max(1).min(job.wexp[k]);
                }
            }
            m.write_bytes(at + 0x60, &new);
        }
        changed += 1;
    }
    log::debug!("randomizer: a chapter's person table at {table:#010x}: {changed} of {count} characters changed");
}

/// a unit a chapter places: the class it is placed with and its weapons.
fn dispos_unit(state: &mut State, system: &mut System, entry: u32) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some(active) = state.active.as_ref() else { return };
    let m = &mut system.memory;
    let pid_ptr = r32(m, entry);
    let pid = cstr(m, pid_ptr, 64);
    if pid.is_empty() {
        return;
    }
    if std::env::var_os("FATES_RANDO_TEXTLOG").is_some() {
        log::info!("randomizer: dispos {}", sprites::sjis_text(&pid));
    }
    // the person: in the loaded tables, by PID
    let mut person = 0;
    let mut node = r32(m, crate::region::us(PERSON_LIST));
    let mut guard = 0;
    'tables: while node != 0 && guard < 64 {
        let table = r32(m, node);
        let count = r16(m, table + 4) as u32;
        for i in 0..count.min(512) {
            let at = table + 0x10 + i * PERSON_SIZE;
            let pid_at = r32(m, at + 8);
            if active.pool.contains_key(&at) && cstr(m, pid_at, 64) == pid {
                person = at;
                break 'tables;
            }
        }
        node = r32(m, node + 8);
        guard += 1;
    }
    // rando3: Corrin, when Corrin's class is randomized
    if person == 0 {
        if let Some(&(at, female)) = active.corrin_at.iter().find(|(at, _)| {
            let pid_at = r32(m, at + 8);
            cstr(m, pid_at, 64) == pid
        }) {
            let base = active.base;
            let class = r16(m, at + 0x2C);
            if let Some(job) = cat.job(class) {
                if r32(m, entry + 4) != 0 {
                    w32(m, entry + 4, base.wrapping_add(cat.gamedata.u32(job.offset + 8)));
                }
                let changed = fit_weapons(&cat, m, base, job, entry);
                log::debug!("randomizer: placed Corrin ({}) as {}, weapons {:?}", if female { "female" } else { "male" }, job.name, changed);
            }
            return;
        }
    }
    let Some(&slot) = active.pool.get(&person) else { return };
    let base = active.base;
    let jid_ptr = r32(m, entry + 4);
    let mut class = r16(m, person + 0x2C);
    if jid_ptr != 0 {
        let jid = cstr(m, jid_ptr, 64);
        if let Some(&original) = cat.jid_index.get(&jid) {
            let new = active.plan.chapter_class(&cat, slot, original);
            if let Some(job) = cat.job(new) {
                w32(m, entry + 4, base.wrapping_add(cat.gamedata.u32(job.offset + 8)));
                class = new;
            }
        }
    }
    let Some(job) = cat.job(class) else { return };
    let changed_items = fit_weapons(&cat, m, base, job, entry);
    log::debug!(
        "randomizer: placed {} as {} ({}){}",
        cat.identities[slot].name,
        cat.identities[active.plan.shown(slot)].name,
        job.name,
        if changed_items.is_empty() { String::new() } else { format!(", weapons {}", changed_items.join(", ")) }
    );
}

/// the weapons a dispos entry gives that the class can't use become ones it
/// can; answers what changed
fn fit_weapons(cat: &Catalog, m: &mut Memory, base: u32, job: &catalog::JobRec, entry: u32) -> Vec<String> {
    let mut changed_items = Vec::new();
    for i in 0..5 {
        let field = entry + 0x14 + 8 * i;
        let iid_at = r32(m, field);
        let iid = cstr(m, iid_at, 64);
        let Some(&item) = cat.iid_index.get(&iid) else { continue };
        let item = &cat.items[item];
        let Some(kind) = item.kind() else { continue };
        if job.uses(kind) {
            continue;
        }
        if let Some(new) = replacement_weapon(cat, job, item) {
            w32(m, field, base.wrapping_add(cat.gamedata.u32(cat.items[new].offset + 8)));
            changed_items.push(format!("{} -> {}", item.name, cat.items[new].name));
        }
    }
    changed_items
}

/// rando3: a class (and its promotion) in a GameData person entry, with
/// weapon ranks for it (the entry's best rank)
fn system_write_class(m: &mut Memory, cat: &Catalog, at: u32, class: u16, promoted: u16, wexp: &[u8; 8]) {
    w16(m, at + 0x2C, class);
    w16(m, at + 0x2E, promoted);
    if let Some(job) = cat.job(class) {
        let best = *wexp.iter().max().unwrap_or(&0);
        let mut new = [0u8; 8];
        for k in 0..8 {
            if job.wexp[k] > 0 {
                new[k] = best.max(1).min(job.wexp[k]);
            }
        }
        m.write_bytes(at + 0x60, &new);
    }
}

/// rando3: a character's place from a key the scripts use: their own
/// (レオン), or a costume of theirs with its own model (法衣裏返しレオン, Leo
/// with his robe on inside out in Chapter 1; フードスメラギ, Sumeragi hooded
/// in Chapter 11; 溶けガロン / 溶解ガロン, Garon melting in Birthright's
/// Chapter 27), or a chapter's event copy (PID_A012_OPイベント用ガロン)
pub fn slot_of_key(cat: &Catalog, key: &[u8]) -> Option<usize> {
    if let Some(&slot) = cat.by_key.get(key) {
        return Some(slot);
    }
    const COSTUMES: &[(&str, &str)] = &[("法衣裏返しレオン", "レオン"), ("フードスメラギ", "スメラギ"), ("溶けガロン", "ガロン"), ("溶解ガロン", "ガロン")];
    let text = sprites::sjis_text(key);
    let text = text.rsplit("イベント用").next().unwrap_or(&text).to_string();
    let own = COSTUMES.iter().find(|(c, _)| *c == text).map_or(text.as_str(), |(_, o)| o);
    cat.identities.iter().position(|i| i.key_text == own)
}

/// a weapon like the one given (its rank) that the class can use: the
/// plain one of that rank in the class's main weapon, of the class's
/// kingdom when there are two kinds (katana and sword).
fn replacement_weapon(cat: &Catalog, job: &catalog::JobRec, item: &catalog::ItemRec) -> Option<usize> {
    let kind = job.main_kind()?;
    let hoshido = (job.origin == 1) as u8;
    let categories: Vec<u8> = if kind == 7 {
        vec![15, 14] // beaststones for Wolfskin and Kitsune
    } else {
        vec![kind as u8 * 2 + hoshido, kind as u8 * 2 + 1 - hoshido]
    };
    let ranks = [251u8, 161, 96, 51, 21, 1];
    let wanted = ranks.iter().position(|&r| r <= item.rank.max(1)).unwrap_or(ranks.len() - 1);
    for &category in &categories {
        for &rank in &ranks[wanted..] {
            let best = cat
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.category == category && it.rank == rank && it.price > 0 && it.flags[2] & 0x80 == 0)
                .min_by_key(|(_, it)| (it.flags[2] as u32 + it.flags[3] as u32 + it.flags[4] as u32, it.price, it.id))
                .map(|(i, _)| i);
            if best.is_some() {
                return best;
            }
        }
    }
    None
}

// -- cutscene casts ---------------------------------------------------------

/// guest bytes in the randomizer's page, 4-aligned.
fn bytes_at(state: &mut State, system: &mut System, bytes: &[u8]) -> Option<u32> {
    let need = (bytes.len() as u32 + 3) & !3;
    if state.pool_at == 0 || state.pool_used + need > POOL_SIZE {
        return None;
    }
    let at = state.pool_at + state.pool_used;
    system.memory.write_bytes(at, bytes);
    state.pool_used += need;
    Some(at)
}

/// a person entry that looks like `who` in `class`, for a cutscene cast
/// named after a swapped character: a playable character's own entry as
/// GameData has it, or for a story character a playable one's entry with
/// their portrait, model, name and gender.
fn stand_in(state: &mut State, system: &mut System, cat: &Catalog, base: u32, who: usize, class: u16) -> Option<u32> {
    if let Some(&at) = state.stand_ins.get(&(who, class)) {
        return Some(at);
    }
    let id = &cat.identities[who];
    let own = id.global.and_then(|g| cat.persons.iter().find(|p| p.index == g));
    let template = own.or_else(|| cat.persons.get(3))?;
    let mut bytes = original_entry(cat, base, template.offset);
    if own.is_none() {
        let (fid, aid, mpid, help) = identity_strings(state, system, cat, base, who);
        bytes[0xC..0x10].copy_from_slice(&fid.to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&aid.to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&mpid.to_le_bytes());
        if help != 0 {
            bytes[0x18..0x1C].copy_from_slice(&help.to_le_bytes());
        }
        bytes[0] = (bytes[0] & !1) | id.female as u8;
        bytes[0x87] = id.body;
    }
    bytes[0x2C..0x2E].copy_from_slice(&class.to_le_bytes());
    bytes[0x2E..0x30].copy_from_slice(&0u16.to_le_bytes());
    let at = bytes_at(state, system, &bytes)?;
    state.stand_ins.insert((who, class), at);
    Some(at)
}

fn job_address(cat: &Catalog, base: u32, id: u16) -> Option<u32> {
    cat.job(id).map(|j| base.wrapping_add(j.offset))
}

/// BattleUnitResName::Construct(unit, person, name, job, item, flags), where
/// every 3D model of a character is put together (battles, cutscenes, talk
/// scenes, My Castle). A unit or a person brings its own look, which the
/// randomizer already changed. A cutscene cast named only after a
/// character (a bev script's 登場 "ガロン": no PID_ガロン person exists, so
/// the game would build AID_ガロン's own model) gets a stand-in person of
/// whoever is shown in that character's place, in the slot's class. A job
/// the script names becomes the slot's class, and a weapon in hand that the
/// class can't hold becomes one it can.
fn cast(state: &mut State, system: &mut System, ctx: &mut Context) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some(active) = state.active.as_ref() else { return };
    let (unit, person, name, sp) = (ctx.r[1], ctx.r[2], ctx.r[3], ctx.r[13]);
    if unit != 0 {
        // corrinswap: a model of the place shown as Corrin, or of Corrin's
        let _ = active;
        corrin::cast_unit(state, system, ctx);
        return;
    }
    let base = active.base;
    let m = &mut system.memory;
    let job_ptr = r32(m, sp);
    let item_ptr = r32(m, sp + 4);
    let slot = if person != 0 {
        active.pool.get(&person).copied()
    } else if name != 0 {
        let text = cstr(m, name, 64);
        let key = text.strip_prefix(b"PID_".as_slice()).or_else(|| text.strip_prefix(b"AID_".as_slice())).unwrap_or(&text).to_vec();
        slot_of_key(&cat, &key)
    } else {
        None
    };
    let Some(slot) = slot else { return };
    // lilith: a place the run leaves alone (version 3's characters in a
    // classes-only run) is cast as the game casts it
    if !active.plan.touches(slot) {
        return;
    }
    let shown = active.plan.shown(slot);
    let job_id = if job_ptr != 0 && m.is_readable(job_ptr, 0x20) { r16(m, job_ptr + 0x18) } else { 0 };
    let mut class = if person != 0 { r16(m, person + 0x2C) } else { 0 };
    if job_id != 0 {
        let new = active.plan.chapter_class(&cat, slot, job_id);
        if new != job_id {
            if let Some(at) = job_address(&cat, base, new) {
                w32(m, sp, at);
            }
        }
        class = new;
    }
    let own = cat.identities[slot].class;
    let wanted = if class != 0 { class } else { active.plan.chapter_class(&cat, slot, own) };
    if corrin::cast_as_corrin(state, system, ctx, slot, wanted) {
        // corrinswap: the place shown as Corrin, cast with no unit: the
        // avatar's own (a copy), in the place's class
        class = wanted;
    } else if person == 0 {
        let Some(fake) = stand_in(state, system, &cat, base, shown, wanted) else { return };
        ctx.r[2] = fake;
        ctx.r[3] = 0;
        class = wanted;
        state.casts_changed += 1;
        if state.casts_changed <= 60 {
            log::debug!(
                "randomizer: cutscene cast {} shown as {} ({})",
                cat.identities[slot].name,
                cat.identities[shown].name,
                cat.job(class).map_or("?", |j| j.name.as_str())
            );
        }
    }
    // the weapon in hand
    let Some(job) = cat.job(class) else { return };
    let m = &mut system.memory;
    if item_ptr != 0 {
        let iid = cstr(m, item_ptr, 64);
        if let Some(item) = cat.iid_index.get(&iid).map(|&i| &cat.items[i]) {
            if item.kind().is_some_and(|k| !job.uses(k)) {
                if let Some(new) = replacement_weapon(&cat, job, item) {
                    w32(m, sp + 4, base.wrapping_add(cat.gamedata.u32(cat.items[new].offset + 8)));
                }
            }
        }
    }
}

// -- talk scenes ------------------------------------------------------------

/// the English names said in a talk line, each made the name of whoever is
/// shown in that character's place ("Sakura, stay back!" in the prologue
/// says the name of the one in Sakura's place). Whole words only, outside
/// the line's $ codes, in one pass so two swapped names don't undo each
/// other.
fn rename_in_text(state: &mut State, system: &mut System, expander: u32) {
    let Some(active) = state.active.as_ref() else { return };
    if active.plan.options & plan::CHARACTERS == 0 || !active.cast.any_change() {
        return;
    }
    let m = &mut system.memory;
    let buffer = r32(m, expander + 4);
    if buffer == 0 || !m.is_writable(buffer, 0x4000) {
        return;
    }
    let mut raw = vec![0u8; 0x4000];
    m.read_bytes(buffer, &mut raw);
    let text: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    // rando3: names and the gendered words about a swapped character, in
    // one pass (text.rs); the first rounds' name pass is text::rewrite's too
    if std::env::var_os("FATES_RANDO_TEXTLOG").is_some() {
        log::info!("randomizer: talk text: {:?}", String::from_utf16_lossy(&text));
    }
    let mut changes = Vec::new();
    let Some(new) = text::rewrite(&active.cast, &text, &mut changes) else { return };
    let mut bytes: Vec<u8> = new.iter().take(0x1FFF).flat_map(|u| u.to_le_bytes()).collect();
    bytes.extend_from_slice(&[0, 0]);
    m.write_bytes(buffer, &bytes);
    state.names_changed += 1;
    for c in &changes {
        state.words_changed += 1;
        if state.words_changed <= 400 || state.words_changed % 100 == 0 {
            log::debug!("randomizer: talk word {} -> {}: {} ({} so far)", c.from, c.to, c.why, state.words_changed);
        }
    }
}

/// rando3: Corrin's unit, made by the avatar creator with Corrin's own class
/// named (Nohr Prince or Princess, or the Hoshido and Nohr nobles), gets
/// the randomized class of its entry instead.
fn unit_job(state: &mut State, system: &mut System, person: u32, job: u32) -> u32 {
    let Some(cat) = state.catalog.clone() else { return 0 };
    let Some(active) = state.active.as_ref() else { return 0 };
    if !active.corrin_at.iter().any(|&(at, _)| at == person) {
        return 0;
    }
    let m = &mut system.memory;
    let asked = r16(m, job + 0x18);
    let class = r16(m, person + 0x2C);
    // only Corrin's own classes (the lord's line: ids 1 to 6)
    if asked == class || !(1..=6).contains(&asked) {
        return 0;
    }
    let Some(new) = cat.job(class) else { return 0 };
    log::info!("randomizer: Corrin's unit is made as {} (asked {})", new.name, cat.job(asked).map_or("?", |j| j.name.as_str()));
    active.base.wrapping_add(new.offset)
}

/// rando3: the items a chapter gives Corrin's existing unit: weapons
/// Corrin's random class can't use become ones it can.
fn corrin_items(state: &mut State, system: &mut System, entry: u32) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some(active) = state.active.as_ref() else { return };
    let m = &mut system.memory;
    let pid_at = r32(m, entry);
    let pid = cstr(m, pid_at, 64);
    if pid.is_empty() {
        return;
    }
    if std::env::var_os("FATES_RANDO_TEXTLOG").is_some() {
        log::info!("randomizer: dispos items for {}", sprites::sjis_text(&pid));
    }
    let Some(&(at, _)) = active.corrin_at.iter().find(|(at, _)| {
        let p = r32(m, at + 8);
        cstr(m, p, 64) == pid
    }) else {
        return;
    };
    let class = r16(m, at + 0x2C);
    let base = active.base;
    if let Some(job) = cat.job(class) {
        let changed = fit_weapons(&cat, m, base, job, entry);
        if !changed.is_empty() {
            log::info!("randomizer: Corrin's items for {}: {}", job.name, changed.join(", "));
        }
    }
}

/// the longest talk line (TalkExpander's buffer: 0x2000 characters)
const LINE_CHARS: u32 = 0x2000;

/// rando3: a talk line, before TalkExpander expands it: its names and the
/// gendered words about swapped characters made to fit who is shown
/// (text.rs). The copy goes in the randomizer's page; the game expands it
/// as it would have the line. Answers its address, or 0 for no change.
fn text_line(state: &mut State, system: &mut System, line: u32) -> u32 {
    // corrinswap: the cast follows the Corrin swap of the avatar's gender
    // (known after the creator)
    let pair = corrin::pair(state, system).map(|(s, y, _, _)| (s, y));
    if let (Some(cat), Some(active)) = (state.catalog.clone(), state.active.as_mut()) {
        if active.plan.corrin_swap.is_some() && active.cast_pair != pair {
            active.cast = make_cast(&cat, &active.plan, pair);
            active.cast_pair = pair;
        }
    }
    let Some(active) = state.active.as_ref() else { return 0 };
    if (active.plan.options & plan::CHARACTERS == 0 && active.cast_pair.is_none()) || !active.cast.any_change() {
        return 0;
    }
    let m = &mut system.memory;
    if !m.is_readable(line, 2) {
        return 0;
    }
    let mut text: Vec<u16> = Vec::new();
    for i in 0..LINE_CHARS {
        let at = line + i * 2;
        if i % 0x800 == 0 && !m.is_readable(at, 2) {
            break;
        }
        let u = m.read16(at);
        if u == 0 {
            break;
        }
        text.push(u);
    }
    if std::env::var_os("FATES_RANDO_TEXTLOG").is_some() {
        log::info!("randomizer: talk line: {:?}", String::from_utf16_lossy(&text));
    }
    let mut changes = Vec::new();
    let Some(new) = text::rewrite(&active.cast, &text, &mut changes) else { return 0 };
    // rando4: a page made too wide for its window is wrapped again
    let new = match talk_font(state, system) {
        Some(font) => match wrap::rewrap(&font, &text, &new) {
            Some(wrapped) => {
                log::debug!("randomizer: line wrapped again: {:?}", String::from_utf16_lossy(&wrapped));
                wrapped
            }
            None => new,
        },
        None => new,
    };
    if state.line_buffer == 0 {
        state.line_buffer = bytes_at(state, system, &vec![0u8; (LINE_CHARS as usize + 1) * 2]).unwrap_or(0);
        if state.line_buffer == 0 {
            return 0;
        }
    }
    let mut bytes: Vec<u8> = new.iter().take(LINE_CHARS as usize - 1).flat_map(|u| u.to_le_bytes()).collect();
    bytes.extend_from_slice(&[0, 0]);
    system.memory.write_bytes(state.line_buffer, &bytes);
    state.names_changed += 1;
    for c in &changes {
        state.words_changed += 1;
        if state.words_changed <= 400 || state.words_changed % 100 == 0 {
            log::debug!("randomizer: talk word {} -> {}: {} ({} so far)", c.from, c.to, c.why, state.words_changed);
        }
    }
    state.line_buffer
}

/// rando4: the talk font's widths, read once
fn talk_font(state: &mut State, system: &mut System) -> Option<Arc<wrap::Font>> {
    if state.font.is_none() && !state.font_failed {
        state.font = system.title.as_ref().and_then(rom::Rom::of).and_then(|rom| wrap::Font::load(&rom)).map(Arc::new);
        state.font_failed = state.font.is_none();
        if state.font_failed {
            log::warn!("randomizer: the talk font could not be read; lines are not wrapped again");
        }
    }
    state.font.clone()
}

/// rando3: who the talk lines know, for a run: every character of the
/// catalog with their name, key, gender and family, who is shown in their
/// place, Corrin (either gender: the game genders Corrin's words itself),
/// and the game's other named characters for he and she.
fn make_cast(cat: &Catalog, plan: &Plan, corrin_pair: Option<(usize, usize)>) -> text::Cast {
    let mut cast = text::Cast::default();
    let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    for (slot, id) in cat.identities.iter().enumerate() {
        let shown = &cat.identities[plan.shown(slot)];
        // corrinswap: the place shown as Corrin says the player's name ($Nu,
        // which the game expands), of Corrin's gender (Y's)
        let as_corrin = corrin_pair.is_some_and(|(s, _)| s == slot);
        cast.add(text::TextPerson {
            name: w(&id.name),
            key: w(&id.key_text),
            female: id.female,
            child: id.child,
            shown_name: if as_corrin { w("$Nu") } else { w(&shown.name) },
            shown_female: shown.female,
            ..Default::default()
        });
    }
    let corrin_shown = corrin_pair.map_or("Corrin".to_string(), |(_, y)| cat.identities[y].name.clone());
    let corrin = cast.add(text::TextPerson { name: w("Corrin"), key: w("username"), either: true, shown_name: w(&corrin_shown), ..Default::default() });
    cast.corrin = Some(corrin);
    // corrinswap: Corrin's own name ($Nu) is Y's
    cast.corrin_name = corrin_pair.map(|(_, y)| w(&cat.identities[y].name));
    let find = |k: &str| cat.identities.iter().position(|i| i.key_text == k);
    // the families (curated): the royal siblings with Corrin, the twins
    let families: &[&[&str]] = &[
        &["マークス", "カミラ", "レオン", "エリーゼ"],
        &["リョウマ", "ヒノカ", "タクミ", "サクラ"],
        &["フェリシア", "フローラ"],
        &["スズカゼ", "サイゾウ"],
    ];
    for fam in families {
        let members: Vec<usize> = fam.iter().filter_map(|k| find(k)).collect();
        for &a in &members {
            let mut sib: Vec<usize> = members.iter().copied().filter(|&b| b != a).collect();
            if fam.len() == 4 {
                sib.push(corrin);
                cast.people[corrin].siblings.push(a);
            }
            cast.people[a].siblings.extend(sib);
        }
    }
    // parents: Garon of Nohr's royals, Sumeragi and Mikoto of Hoshido's,
    // Arete of Azura; Corrin's three; each child's own parent from GameData
    let parents: &[(&str, &[&str])] = &[
        ("ガロン", &["マークス", "カミラ", "レオン", "エリーゼ"]),
        ("スメラギ", &["リョウマ", "ヒノカ", "タクミ", "サクラ"]),
        ("ミコト", &["リョウマ", "ヒノカ", "タクミ", "サクラ"]),
        ("シェンメイ", &["アクア"]),
    ];
    for (parent, kids) in parents {
        let Some(p) = find(parent) else { continue };
        for k in kids.iter().filter_map(|k| find(k)) {
            cast.people[k].parents.push(p);
            cast.people[p].children.push(k);
        }
    }
    for k in ["ミコト", "スメラギ", "ガロン"] {
        if let Some(p) = find(k) {
            cast.people[corrin].parents.push(p);
            cast.people[p].children.push(corrin);
        }
    }
    for (slot, id) in cat.identities.iter().enumerate() {
        let Some(parent) = id.parent else { continue };
        if let Some(p) = cat.identities.iter().position(|i| i.global == Some(parent)) {
            cast.people[slot].parents.push(p);
            cast.people[p].children.push(slot);
        }
    }
    for (name, g) in &cat.extra_names {
        cast.extra.insert(w(name), *g);
    }
    cast.extra.insert(w("Kana"), None);
    cast
}

/// GameBackup::GetFileName's table of the chapter saves' names (Type 3)
const CHAPTER_FILE_NAMES: u32 = 0x0017_8F94;

/// rando3: the save list's mark. A slot whose save is randomized shows it
/// on its mode badge: "Casual" becomes "Randomizer", in the badge's
/// own frame and font. Read from the save file (badge.rs), whether a run is
/// on or not.
fn save_badge(state: &mut State, system: &mut System, item: u32) -> u32 {
    let m = &mut system.memory;
    let kind = r8(m, item + 0x3D);
    let slot = r32(m, item + 0x40);
    if kind != 3 || slot >= 9 {
        return 0;
    }
    let table = r32(m, crate::region::us(CHAPTER_FILE_NAMES));
    let name_at = r32(m, table.wrapping_add(slot * 4));
    if name_at == 0 || !m.is_readable(name_at, 2) {
        return 0;
    }
    let mut name = String::new();
    for i in 0..40 {
        let u = m.read16(name_at + i * 2);
        if u == 0 {
            break;
        }
        name.push(char::from_u32(u as u32).unwrap_or('?'));
    }
    let files = badge::save_files(&system.services.fs.user_dir, system.kernel.program_id, &name);
    let Some(path) = files.first() else {
        log::debug!("randomizer: save slot {slot} ({name}): no file");
        return 0;
    };
    // the save list draws each slot every frame: a file is read once (again
    // when it changes)
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let run = match state.badge_cache.get(path) {
        Some((when, run)) if *when == modified => *run,
        _ => {
            let run = std::fs::read(path).ok().and_then(|raw| badge::marker_in(&raw, MAGIC));
            if let Some((seed, options, version)) = run {
                log::info!("randomizer: save slot {slot} ({name}) is randomized: seed {}, options {options:#x}, rules v{version}", seed_text(seed));
            }
            state.badge_cache.insert(path.clone(), (modified, run));
            run
        }
    };
    if run.is_none() {
        return 0;
    }
    let m = &mut system.memory;
    let mut mode = String::new();
    for i in 0..0x20u32 {
        let u = m.read16(item + 0x1F8 + i * 2);
        if u == 0 {
            break;
        }
        mode.push(char::from_u32(u as u32).unwrap_or('?'));
    }
    // the badge's green, pink or orange frame still says the mode; its
    // text, about 11 letters wide (the mode with "Random" after it does not
    // fit), says the run is randomized
    if mode == "Randomizer" || !map_pool(state, system) {
        return 0;
    }
    let text = "Randomizer".to_string();
    wstring_at(state, system, &text).unwrap_or(0)
}

/// rando3: a map sprite set up for class `job` and character `chara`
/// (UnitIcon::SetIcon(const char*, const char*, UnitColor)): when the
/// character's head sheet has no head for that class's body (Kotaro or
/// Hans on a horse, Garon in anything), the sprite is drawn with the
/// class's own generic head, the one its soldiers wear, instead of none.
/// Answers the character name to use (the class's), or 0.
fn sprite(state: &mut State, system: &mut System, job: u32, chara: u32) -> u32 {
    let Some(cat) = state.catalog.clone() else { return 0 };
    // tests: FATES_RANDO_NOSPRITE=1 draws the sprites as the game would (the before picture)
    if state.active.is_none() || job == 0 || chara == 0 || std::env::var_os("FATES_RANDO_NOSPRITE").is_some() {
        return 0;
    }
    let m = &mut system.memory;
    let class = sprites::sjis_text(&cstr(m, job, 48));
    let who = sprites::sjis_text(&cstr(m, chara, 48));
    if class == who || cat.sprites.fits(&class, &who) {
        return 0;
    }
    // the avatar's heads (プレイヤー男1_01...) have every column
    if who.starts_with("プレイヤー") || !cat.sprites.bodies.contains(&class) {
        return 0;
    }
    if state.sprites_logged.len() < 200 && state.sprites_logged.insert((class.clone(), who.clone())) {
        log::info!("randomizer: map sprite: {who}'s head has no frames for the {class} body; drawn with the class's own head");
    }
    job
}

fn is_word(u: u16) -> bool {
    char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric())
}

/// text with the names replaced, None when none was said.
pub fn rename(names: &[(Vec<u16>, Vec<u16>)], text: &[u16]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(text.len() + 16);
    let mut changed = false;
    let mut i = 0;
    while i < text.len() {
        // a $ code, copied as it is: $k $p $a, $Nu, $t0, $Wa (window codes
        // without a name), or up to its | ($Wm<name>|, $Svp<clip>|, $E..|)
        if text[i] == b'$' as u16 {
            let c1 = text.get(i + 1).copied().unwrap_or(0);
            let c2 = text.get(i + 2).copied().unwrap_or(0);
            let to_bar = || text[i..].iter().position(|&u| u == b'|' as u16).map_or(text.len(), |p| i + p + 1);
            let stop = match c1 {
                0x6B | 0x70 | 0x61 => i + 2,                      // $k $p $a
                0x4E | 0x74 => i + 3,                             // $Nu $t0
                0x57 if c2 != 0x6D && c2 != 0x73 => i + 3,        // $Wa $Wd ...
                _ => to_bar(),
            }
            .min(text.len());
            out.extend_from_slice(&text[i..stop]);
            i = stop;
            continue;
        }
        if i == 0 || !is_word(text[i - 1]) {
            if let Some((from, to)) = names.iter().find(|(from, _)| {
                text[i..].starts_with(from) && !text.get(i + from.len()).is_some_and(|&u| is_word(u))
            }) {
                out.extend_from_slice(to);
                i += from.len();
                changed = true;
                continue;
            }
        }
        out.push(text[i]);
        i += 1;
    }
    changed.then_some(out)
}

/// the FID a talk line's window asked for (TalkUtil::GetFidForWindow's
/// output), made the FID of whoever is shown in that character's place.
fn remap_fid(state: &mut State, system: &mut System, out: u32, room: u32) {
    let Some(cat) = state.catalog.clone() else { return };
    // corrinswap: Corrin's window (FID_username) shows Y, and the window of
    // the place shown as Corrin shows the avatar (FID_username)
    if let Some((s, y, _, _)) = corrin::pair(state, system) {
        let m = &mut system.memory;
        let fid = cstr(m, out, room as usize);
        if let Some(key) = fid.strip_prefix(b"FID_".as_slice()) {
            let to: Option<Vec<u8>> = if key == b"username" {
                Some(cat.identities[y].fid.clone())
            } else if slot_of_key(&cat, key) == Some(s) {
                Some(b"FID_username".to_vec())
            } else {
                None
            };
            if let Some(to) = to {
                write_cstr(m, out, &to, room as usize);
                state.faces_changed += 1;
                if state.faces_changed <= 40 || state.faces_changed % 500 == 0 {
                    log::debug!("randomizer: talk face {} -> {} ({} so far)", sprites::sjis_text(&fid), sprites::sjis_text(&to), state.faces_changed);
                }
                return;
            }
        }
    }
    let Some(active) = state.active.as_ref() else { return };
    if active.plan.options & plan::CHARACTERS == 0 {
        return;
    }
    let m = &mut system.memory;
    let fid = cstr(m, out, room as usize);
    let Some(key) = fid.strip_prefix(b"FID_".as_slice()) else { return };
    let Some(slot) = slot_of_key(&cat, key) else { return };
    let shown = active.plan.shown(slot);
    if shown != slot {
        write_cstr(m, out, &cat.identities[shown].fid, room as usize);
        state.faces_changed += 1;
        if state.faces_changed <= 40 || state.faces_changed % 500 == 0 {
            log::debug!("randomizer: talk face {} -> {} ({} so far)", cat.identities[slot].name, cat.identities[shown].name, state.faces_changed);
        }
    }
}

/// the clip a talk line plays ($Svp), made the same kind of clip of whoever
/// is shown in its speaker's place: the clip with the same name if they
/// have it, else one of the same category (another "hmm", another battle
/// cry), else one of a related category, else none.
fn remap_voice(state: &mut State, system: &mut System, at: u32) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some(active) = state.active.as_ref() else { return };
    if active.plan.options & plan::CHARACTERS == 0 && active.plan.corrin_swap.is_none() {
        return;
    }
    let m = &mut system.memory;
    let label = String::from_utf8_lossy(&cstr(m, at, 0x40)).into_owned();
    // corrinswap: the place shown as Corrin has no voice set of its own
    // (the avatar's voice is chosen in the creator): its talk clips are
    // silent
    let as_corrin = corrin_pair_now(state, system).is_some_and(|(s, _)| voice_owner(&cat, &label) == Some(s));
    let Some(active) = state.active.as_ref() else { return };
    let m = &mut system.memory;
    let new = if as_corrin { Some(String::new()) } else { voice_for(&cat, &active.plan, &label) };
    if let Some(new) = new {
        state.voices_changed += 1;
        if state.voices_changed <= 40 || state.voices_changed % 500 == 0 {
            log::debug!("randomizer: talk voice {label} -> {} ({} so far)", if new.is_empty() { "(none)" } else { &new }, state.voices_changed);
        }
        write_cstr(m, at, new.as_bytes(), 0x40);
    }
}

/// corrinswap: the Corrin swap now, if any (S, Y)
fn corrin_pair_now(state: &mut State, system: &mut System) -> Option<(usize, usize)> {
    corrin::pair(state, system).map(|(s, y, _, _)| (s, y))
}

/// the identity whose voice set a clip is in
fn voice_owner(cat: &Catalog, label: &str) -> Option<usize> {
    let rest = label.strip_prefix("VOICE_")?;
    let set = cat.voice_sets.iter().find(|s| rest.starts_with(s.as_str()) && rest.as_bytes().get(s.len()) == Some(&b'_'))?;
    cat.voice_owner.get(set).copied()
}

const VOICE_FAMILIES: &[&[&str]] = &[
    &["EVT", "EVT_S", "B_SUPPORT", "MYROOM", "ETC", "B_WIN", "WIN"],
    &["B_ATTACK", "ATTACK_L", "ATTACK_H", "CUT_IN", "B_SUPPORT"],
    &["B_GUARD", "DAMAGE", "DEAD", "B_SUPPORT"],
];

pub fn voice_for(cat: &Catalog, plan: &Plan, label: &str) -> Option<String> {
    let rest = label.strip_prefix("VOICE_")?;
    let set = cat.voice_sets.iter().find(|s| rest.starts_with(s.as_str()) && rest.as_bytes().get(s.len()) == Some(&b'_'))?;
    let owner = *cat.voice_owner.get(set)?;
    let shown = plan.shown(owner);
    if shown == owner {
        return None;
    }
    let Some(target) = cat.identities[shown].voice.as_ref() else { return Some(String::new()) };
    let tail = &rest[set.len() + 1..];
    let exact = format!("VOICE_{target}_{tail}");
    if cat.voices.contains(&exact) {
        return Some(exact);
    }
    let category = catalog::category(tail);
    let sets = cat.voice_categories.get(target)?;
    let pick = |clips: &Vec<String>| clips[(tail.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize) % clips.len()].clone();
    if let Some(clips) = sets.get(&category).filter(|c| !c.is_empty()) {
        return Some(pick(clips));
    }
    for family in VOICE_FAMILIES.iter().filter(|f| f.contains(&category.as_str())) {
        for other in family.iter() {
            if let Some(clips) = sets.get(*other).filter(|c| !c.is_empty()) {
                return Some(pick(clips));
            }
        }
    }
    Some(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_read_back() {
        assert_eq!(parse_seed(&seed_text(0x1A2B_3C4D)), Some(0x1A2B_3C4D));
        assert_eq!(parse_seed("ff"), Some(0xFF));
        assert_eq!(parse_seed("not a seed"), None);
    }

    #[test]
    fn names_in_lines() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let names = vec![(w("Sakura"), w("Shura")), (w("Leo"), w("Sakura"))];
        let line = w("$Wmサクラ|3$w0|$Wsサクラ|$Wa$SvpVOICE_SAKURA_EVT_01|Leo, Leon and Sakura's$k");
        let out = String::from_utf16(&rename(&names, &line).unwrap()).unwrap();
        assert_eq!(out, "$Wmサクラ|3$w0|$Wsサクラ|$Wa$SvpVOICE_SAKURA_EVT_01|Sakura, Leon and Shura's$k");
    }

    #[test]
    fn categories() {
        assert_eq!(catalog::category("B_ATTACK_02"), "B_ATTACK");
        assert_eq!(catalog::category("EVT_S_00"), "EVT_S");
        assert_eq!(catalog::category("MYROOM_SP_A"), "MYROOM_SP_A");
    }
}
