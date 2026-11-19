//! pulse: Divine Pulse, the modern Fire Emblem games' rewind, as an option
//! (Gameplay > Divine Pulse, off by default). See dev/reports/pulse.md and
//! dev/reports/pulse-research.md for the games it follows.
//!
//! - **Moments.** On a battle map a snapshot of the console is kept before
//!   each action (zakuro-core's `state::rewind`, in memory only): each time
//!   the player phase's free cursor comes back with the units changed (the
//!   first one of a phase is the turn's start), and before each enemy or
//!   other unit acts (each real decision of the game's AI). Three Houses
//!   keeps every action of the battle; Engage's list has the enemy's too.
//! - **Opening it.** The System Menu's Divine Pulse (overrides/qol.c and
//!   pulse.c), its rebindable key and button (Controls: "Divine Pulse"), and
//!   by itself when the battle is about to be lost (SetGameOver), as both
//!   games do; declining lets the Game Over go on.
//! - **The screen** (fates_ui/pulse.rs): the game stops, tinted, and the
//!   timeline of moments runs along the bottom. Moving through it puts the
//!   map back to that moment at once (the moment is loaded and two frames
//!   run without input, so the map shows it), with the turn, the phase and
//!   the charges left. Confirming turns time back there and costs a charge;
//!   cancelling puts back the moment it was opened at.
//! - **Charges.** Three Houses' story scaling (3 at first, up to 13, or 10
//!   on Lunatic, as Maddening's), a fixed number (Engage's 10 per map on
//!   Hard and Maddening) or unlimited (Engage's Normal). They refill at
//!   each new map, as both games' do.
//! - **Outcomes.** The whole console comes back, the game's random numbers
//!   with it: the same action taken again turns out the same, as in Three
//!   Houses (and as Engage is reported to); doing something else first
//!   changes what follows.
//! - **Never** in an online battle (netplay::battle()), off a battle map,
//!   or with the option off. Nothing is written to disk: the user's saves
//!   only change through the game's own save points.
//!
//! Headless tests: `FATES_PULSE_AT="<frame>:<command>[;...]"` (open, left,
//! right, back <n>, confirm, cancel, list) and `FATES_PULSE=1` to turn the
//! option on without the settings file.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use zakuro_core::memory::Memory;
use zakuro_core::state::rewind::{Cost, Rewind};
use zakuro_core::System;

use crate::settings::{PcSettings, PulseCharges};

/// indices into overrides/pulse.c's `pulse_words`.
pub mod word {
    pub const ON: usize = 0;
    pub const FREE_FRAME: usize = 1;
    pub const AI_CALLS: usize = 2;
    pub const GAME_OVER: usize = 3;
    pub const MENU_ITEM: usize = 4;
    pub const MENU_REQUEST: usize = 5;
    pub const GAME_OVER_TYPE: usize = 6;
    pub const AI_UNIT: usize = 7;
    pub const COUNT: usize = 64;
}

#[derive(Clone, Copy)]
pub struct Hooks {
    pub words: fn() -> *mut u32,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

pub fn set_hooks(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

fn read(index: usize) -> u32 {
    let Some(hooks) = HOOKS.get() else { return 0 };
    if index >= word::COUNT {
        return 0;
    }
    // SAFETY: a word of the game build's array; a u32 is never torn
    unsafe { std::ptr::read_volatile((hooks.words)().add(index)) }
}

fn write(index: usize, value: u32) {
    if let Some(hooks) = HOOKS.get() {
        if index < word::COUNT {
            // SAFETY: as above
            unsafe { std::ptr::write_volatile((hooks.words)().add(index), value) }
        }
    }
}

// ---- the game's memory (USA Special Edition v2) --------------------------------

const SITUATION: u32 = 0x006D_7ED0; // map::Situation*: +8 the phase's force, +0xC the turn
const UNIT_POOL: u32 = 0x006D_9F40;
const FORCES: u32 = 0x006D_9F44;
const UNIT_SIZE: u32 = 0x140;
const GAME_USER: u32 = 0x006D_A248; // +0xC the chapter, +0x28 mode bits, +0x2D difficulty

fn r8(m: &mut Memory, a: u32) -> u8 {
    if a == 0 || !m.is_readable(a, 1) {
        return 0;
    }
    let mut b = [0u8; 1];
    m.read_bytes(a, &mut b);
    b[0]
}
fn r16(m: &mut Memory, a: u32) -> u16 {
    if a == 0 || !m.is_readable(a, 2) {
        return 0;
    }
    let mut b = [0u8; 2];
    m.read_bytes(a, &mut b);
    u16::from_le_bytes(b)
}
fn r32(m: &mut Memory, a: u32) -> u32 {
    if a == 0 || !m.is_readable(a, 4) {
        return 0;
    }
    let mut b = [0u8; 4];
    m.read_bytes(a, &mut b);
    u32::from_le_bytes(b)
}
fn cstr(m: &mut Memory, a: u32) -> Vec<u8> {
    let mut out = Vec::new();
    if a == 0 {
        return out;
    }
    for i in 0..64 {
        let b = r8(m, a + i);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

/// the map's turn and the force whose phase it is, off a map none.
pub fn situation(m: &mut Memory) -> Option<(u16, u8)> {
    let sit = r32(m, zakuro_core::region::us(SITUATION));
    (sit != 0).then(|| (r16(m, sit + 0xC), r8(m, sit + 8)))
}

/// the chapter being played, its CID ("CID_B012"), from GameUserData.
pub fn chapter_cid(m: &mut Memory) -> String {
    let gud = r32(m, zakuro_core::region::us(GAME_USER));
    let chapter = r32(m, gud + 0xC);
    let at = r32(m, chapter);
    String::from_utf8_lossy(&cstr(m, at)).into_owned()
}

/// the difficulty (0 Normal, 1 Hard, 2 Lunatic) and whether it is Classic.
pub fn difficulty(m: &mut Memory) -> (u8, bool) {
    let gud = r32(m, zakuro_core::region::us(GAME_USER));
    let mode = r32(m, gud + 0x28);
    (r8(m, gud + 0x2D), mode & 0xC == 0)
}

/// a unit on the map, as a moment had it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitBrief {
    pub index: u16,
    pub force: u8,
    pub x: u8,
    pub y: u8,
    pub hp: u8,
    pub acted: bool,
    pub person: u32,
}

fn units(m: &mut Memory) -> (Vec<UnitBrief>, u64) {
    let pool = r32(m, zakuro_core::region::us(UNIT_POOL));
    let forces = r32(m, zakuro_core::region::us(FORCES));
    let mut out = Vec::new();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    if pool == 0 || forces == 0 {
        return (out, 0);
    }
    let mut bytes = vec![0u8; UNIT_SIZE as usize];
    for force in 0..4u32 {
        let mut unit = r32(m, forces + force * 12);
        let mut guard = 0;
        while unit != 0 && guard < 256 && m.is_readable(unit, UNIT_SIZE) {
            m.read_bytes(unit, &mut bytes);
            // everything of the unit but the transient display state
            for (i, b) in bytes.iter().enumerate() {
                if (0x14..0x18).contains(&i) {
                    continue;
                }
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            hash ^= force as u64;
            if force < 3 {
                out.push(UnitBrief {
                    index: (unit.wrapping_sub(pool) / UNIT_SIZE + 1) as u16,
                    force: force as u8,
                    x: bytes[0xF4],
                    y: bytes[0xF5],
                    hp: bytes[0xF3],
                    acted: bytes[8] & 1 != 0,
                    person: u32::from_le_bytes(bytes[0x9C..0xA0].try_into().unwrap()),
                });
            }
            unit = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap());
            guard += 1;
        }
    }
    (out, hash)
}

/// a moment of the map the player can go back to.
#[derive(Clone, Debug)]
pub struct Moment {
    pub id: u64,
    pub turn: u16,
    /// the force whose phase it was: 0 the player's, 1 the enemy's, 2 other
    pub phase: u8,
    /// who acts next from here: a name, and their force
    pub actor: Option<(String, u8)>,
    /// the first moment of its phase
    pub phase_start: bool,
    /// pulse2: the frame it was kept at
    pub frame: u64,
    units: Vec<UnitBrief>,
    hash: u64,
    /// the game's random numbers then (xorshift128 at 0x752B8C)
    rng: [u32; 4],
    /// tests (FATES_PULSE_VERIFY=1): a hash of all of memory then
    memory: Option<u64>,
}

const RNG: u32 = 0x0075_2B8C;

fn rng(m: &mut Memory) -> [u32; 4] {
    [r32(m, zakuro_core::region::us(RNG)), r32(m, zakuro_core::region::us(RNG) + 4), r32(m, zakuro_core::region::us(RNG) + 8), r32(m, zakuro_core::region::us(RNG) + 12)]
}

/// pulsefix: the store, with the check on for tests (FATES_PULSE_VERIFY=1:
/// each moment keeps a picture of the console, and each restore compares
/// everything with it, state/check.rs). FATES_PULSE_HOSTKEEP=0 leaves the
/// overrides' own state as it is at a restore, as before pulsefix, so the
/// check can show what that missed.
fn new_rewind() -> Rewind {
    let mut rewind = Rewind::default();
    rewind.verify = std::env::var_os("FATES_PULSE_VERIFY").is_some_and(|v| v == "1");
    rewind.keep_host = !std::env::var_os("FATES_PULSE_HOSTKEEP").is_some_and(|v| v == "0");
    rewind
}

/// pulsefix: what the check found at the last restore, for the log.
fn check_line(rewind: &mut Rewind, what: &str) {
    match rewind.last_check.take() {
        Some(Ok(report)) => println!("pulse: check {what}: {report}"),
        Some(Err(error)) => println!("pulse: check {what}: could not compare, {error}"),
        None => {}
    }
}

/// tests: a hash of the four memories, to show a rewind put back every byte.
fn memory_hash(system: &System) -> Option<u64> {
    if !std::env::var_os("FATES_PULSE_VERIFY").is_some_and(|v| v == "1") {
        return None;
    }
    let phys = &system.memory.phys;
    let mut hash: u64 = 0x9e37_79b9_7f4a_7c15;
    for memory in [&phys.fcram[..], &phys.vram[..], &phys.dsp_ram[..], &phys.axi_wram[..]] {
        for chunk in memory.chunks_exact(8) {
            hash = (hash ^ u64::from_le_bytes(chunk.try_into().unwrap())).wrapping_mul(0x0100_0000_01b3).rotate_left(5);
        }
    }
    Some(hash)
}

/// what the rewind screen shows and is doing.
pub struct Screen {
    /// the moment chosen, an index into `moments`; `moments.len()` is now
    pub selected: usize,
    /// the moment the console shows now (an index as above)
    shown: usize,
    /// the console as it was when the screen opened
    now_id: u64,
    /// opened by the battle about to be lost
    pub game_over: bool,
    /// a question up: true for Yes chosen
    pub asking: Option<bool>,
    /// frames still to run without input after loading a moment
    frames_left: u32,
    pub opened: Instant,
    /// a line for the screen (no charges left...)
    pub notice: Option<(String, Instant)>,
}

#[derive(Default)]
pub struct Stats {
    pub snapshots: u64,
    pub snap_time: Duration,
    pub snap_max: Duration,
    pub snap_bytes: u64,
    pub previews: u64,
    pub preview_time: Duration,
    pub preview_max: Duration,
    pub rewinds: u64,
    pub rewind_time: Duration,
    pub rewind_max: Duration,
}

pub struct Pulse {
    pub on: bool,
    pub charges: PulseCharges,
    pub fixed: u32,
    rewind: Option<Rewind>,
    pub moments: Vec<Moment>,
    /// the map the moments are of: the Situation's address and the chapter
    map: Option<(u32, String)>,
    pub used: u32,
    /// the most this map allows, worked out when it began
    pub limit: Option<u32>,
    last_free: u32,
    was_free: bool,
    /// frames since the free cursor last ticked
    free_idle: u32,
    last_ai: u32,
    last_game_over: u32,
    last_menu: u32,
    pub screen: Option<Screen>,
    /// the app should start the pacing and sound over (a moment was loaded)
    loaded: bool,
    /// a line for the toast
    pub toast: Option<String>,
    pub stats: Stats,
    /// the names of persons, from the game's English messages
    names: std::collections::HashMap<Vec<u8>, String>,
    names_loaded: bool,
    /// the newest main chapter played this session, for paralogues
    story: u32,
    script: Vec<(u64, String)>,
    script_loaded: bool,
    frame: u64,
    /// what the screen asked for, done before the next frame
    pending: Option<Pending>,
    /// the turn and phase of the map now
    now_turn: (u16, u8),
    /// a UI script asked for the screen, as the key does
    open_asked: bool,
    /// pulse2: the System Menu's Divine Pulse was chosen: the frames left
    /// for the menu to finish closing (no input reaches the game) before
    /// the screen opens, so the menu is not part of the stopped picture
    menu_closing: u32,
}

impl Default for Pulse {
    fn default() -> Self {
        Pulse {
            on: false,
            charges: PulseCharges::Story,
            fixed: 10,
            rewind: None,
            moments: Vec::new(),
            map: None,
            used: 0,
            limit: None,
            last_free: 0,
            was_free: false,
            free_idle: 1000,
            last_ai: 0,
            last_game_over: 0,
            last_menu: 0,
            screen: None,
            loaded: false,
            toast: None,
            stats: Stats::default(),
            names: Default::default(),
            names_loaded: false,
            story: 0,
            script: Vec::new(),
            script_loaded: false,
            frame: 0,
            pending: None,
            now_turn: (0, 0),
            open_asked: false,
            menu_closing: 0,
        }
    }
}

static PULSE: Mutex<Option<Pulse>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Pulse) -> R) -> R {
    let mut guard = PULSE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Pulse::default))
}

/// the settings, as the options set them.
pub fn apply(pc: &PcSettings) {
    let forced = std::env::var_os("FATES_PULSE").is_some_and(|v| v == "1");
    with(|p| {
        p.on = pc.divine_pulse || forced;
        p.charges = pc.divine_pulse_charges;
        p.fixed = pc.divine_pulse_fixed.clamp(1, 99);
        if !p.on {
            p.forget();
        }
        // the limit follows a change of the setting at once
        p.limit = None;
    });
}

/// whether the game should not run this frame: the rewind screen is up
/// and the map shows the moment chosen.
pub fn holds() -> bool {
    // a frame runs (quietly) when there is work for it: a moment to show,
    // the frames after showing one, or the screen's answer
    with(|p| p.pending.is_none() && p.screen.as_ref().is_some_and(|s| s.frames_left == 0 && s.selected == s.shown))
}

pub fn screen_open() -> bool {
    with(|p| p.screen.is_some())
}

/// a moment was loaded: the app starts the pictures' pacing and the sound
/// over, as after loading a saved state.
pub fn take_loaded() -> bool {
    with(|p| std::mem::take(&mut p.loaded))
}

pub fn take_toast() -> Option<String> {
    with(|p| p.toast.take())
}

/// Three Houses' charges for how far the story is, mapped onto Fates'
/// chapters (see pulse-research.md): 3 at first; one each at chapters 8,
/// 14, 20 and 25 (the four Saint Statues, all difficulties); 3 (2 on
/// Lunatic) from chapter 11 (Three Houses' Chapter 10 clear); 3 (1 on
/// Lunatic) from chapter 17 (Tales of the Red Canyon). 13 at most, 10 on
/// Lunatic, as Normal/Hard and Maddening.
pub fn story_charges(chapter: u32, lunatic: bool) -> u32 {
    let mut charges = 3;
    for statue in [8, 14, 20, 25] {
        if chapter >= statue {
            charges += 1;
        }
    }
    if chapter >= 11 {
        charges += if lunatic { 2 } else { 3 };
    }
    if chapter >= 17 {
        charges += if lunatic { 1 } else { 3 };
    }
    charges
}

/// a main chapter's number from its CID: CID_A001..A006 the common
/// chapters, B/C/D007.. the routes'; none for paralogues and the rest.
fn chapter_number(cid: &str) -> Option<u32> {
    let rest = cid.strip_prefix("CID_")?;
    let mut chars = rest.chars();
    let route = chars.next()?;
    if !matches!(route, 'A' | 'B' | 'C' | 'D') {
        return None;
    }
    chars.as_str().parse().ok()
}

impl Pulse {
    fn forget(&mut self) {
        self.menu_closing = 0;
        if self.screen.is_some() {
            crate::pulse_fx::reset(); // pulse2
        }
        if let Some(rewind) = &mut self.rewind {
            rewind.clear();
        }
        self.moments.clear();
        self.map = None;
        self.screen = None;
        self.used = 0;
    }

    /// the charges this map allows (None: unlimited).
    fn limit(&mut self, m: &mut Memory) -> Option<u32> {
        match self.charges {
            PulseCharges::Unlimited => None,
            PulseCharges::Fixed => Some(self.fixed),
            PulseCharges::Story => {
                let (difficulty, _) = difficulty(m);
                // paralogues and the rest: the newest main chapter played
                // this session, else the middle of the story
                let chapter = chapter_number(&chapter_cid(m)).unwrap_or(if self.story > 0 { self.story } else { 10 });
                Some(story_charges(chapter, difficulty >= 2))
            }
        }
    }

    pub fn left(&self) -> Option<u32> {
        self.limit.map(|limit| limit.saturating_sub(self.used))
    }

    fn load_names(&mut self, system: &mut System) {
        if !self.names_loaded {
            self.names_loaded = true;
            if let Some(rom) = system.title.as_ref().and_then(zakuro_core::randomizer::rom::Rom::of) {
                for path in ["m/@E/GameData.bin.lz", "m/@E/System.bin.lz", "m/@U/GameData.bin.lz", "m/@U/System.bin.lz", "m/GameData.bin.lz", "m/System.bin.lz"] { // eur: @U, the EUR English; jpprep: m/, the Japanese
                    if let Some(file) = rom.read(path).and_then(|f| zakuro_core::randomizer::rom::lz11(&f)) {
                        for (k, v) in zakuro_core::randomizer::rom::messages(&file) {
                            self.names.entry(k).or_insert(v);
                        }
                    }
                }
            }
        }
    }

    fn name_of(&mut self, system: &mut System, unit: &UnitBrief) -> String {
        self.load_names(system);
        let m = &mut system.memory;
        let pool = r32(m, zakuro_core::region::us(UNIT_POOL));
        let addr = pool + (unit.index as u32 - 1) * UNIT_SIZE;
        // the avatar's own name (unit::Edit +0, wchar_t)
        let edit = r32(m, addr + 0xB0);
        if edit != 0 {
            let mut chars = Vec::new();
            for i in 0..16 {
                let c = r16(m, edit + 2 * i);
                if c == 0 {
                    break;
                }
                chars.push(c);
            }
            let own = String::from_utf16_lossy(&chars);
            if !own.trim().is_empty() && own.chars().all(|c| !c.is_control()) {
                return own;
            }
        }
        let at = r32(m, unit.person + 0x14);
        let key = cstr(m, at);
        if let Some(name) = self.names.get(&key) {
            return name.clone();
        }
        // a generic soldier: the class's name
        let job = r32(m, addr + 0xA0);
        let at = r32(m, job + 0x10);
        let key = cstr(m, at);
        self.names.get(&key).cloned().unwrap_or_else(|| String::from_utf8_lossy(&key).trim_start_matches("MJID_").to_owned())
    }

    /// keeps a moment of the console as it is now.
    fn keep(&mut self, system: &mut System, turn: u16, phase: u8, actor: Option<(String, u8)>, units: Vec<UnitBrief>, hash: u64) {
        let snapshot = self.rewind.get_or_insert_with(new_rewind).snapshot(system);
        match snapshot {
            Ok((id, cost)) => {
                self.count_snapshot(cost);
                let phase_start = self.moments.last().is_none_or(|last| last.turn != turn || last.phase != phase);
                // who acted from the moment before: the one whose place or
                // state changed, of the force whose phase it was
                if let Some(last) = self.moments.last().cloned() {
                    if last.actor.is_none() {
                        let mover = units.iter().find(|u| {
                            u.force == last.phase && last.units.iter().find(|w| w.index == u.index).is_some_and(|w| (w.x, w.y) != (u.x, u.y) || (!w.acted && u.acted))
                        });
                        let mover = mover.or_else(|| last.units.iter().find(|w| w.force == last.phase && !units.iter().any(|u| u.index == w.index)));
                        if let Some(mover) = mover.cloned() {
                            let name = self.name_of(system, &mover);
                            if let Some(entry) = self.moments.last_mut() {
                                entry.actor = Some((name, mover.force));
                            }
                        }
                    }
                }
                let rng = rng(&mut system.memory);
                let memory = memory_hash(system);
                self.moments.push(Moment { id, turn, phase, actor, phase_start, frame: self.frame, units, hash, rng, memory });
                // the moments the store let go of for room
                let ids = self.rewind.as_ref().map(Rewind::ids).unwrap_or_default();
                self.moments.retain(|moment| ids.contains(&moment.id));
                // Three Houses goes back 50 turns at most
                self.moments.retain(|moment| moment.turn + 50 > turn);
                log::info!(
                    "pulse: moment {id} kept (turn {turn}, phase {phase}) in {:.1?} (the rest {:.1?}, scan {:.1?}, pages {:.1?}): {} pages, {} KiB",
                    cost.total,
                    cost.rest,
                    cost.scan,
                    cost.pages_time,
                    cost.pages,
                    cost.bytes / 1024
                );
            }
            Err(error) => log::warn!("pulse: no moment kept, {error}"),
        }
    }

    fn count_snapshot(&mut self, cost: Cost) {
        self.stats.snapshots += 1;
        self.stats.snap_time += cost.total;
        self.stats.snap_max = self.stats.snap_max.max(cost.total);
        self.stats.snap_bytes += cost.bytes as u64;
    }

    /// opens the rewind screen, keeping the console as it is now first.
    fn open(&mut self, system: &mut System, game_over: bool) -> bool {
        if self.screen.is_some() {
            return true;
        }
        if self.moments.is_empty() {
            self.toast = Some("Divine Pulse: nothing to turn back yet".into());
            return false;
        }
        let rewind = self.rewind.get_or_insert_with(new_rewind);
        let now_id = match rewind.snapshot(system) {
            Ok((id, cost)) => {
                self.stats.snapshots += 1;
                self.stats.snap_time += cost.total;
                id
            }
            Err(error) => {
                log::warn!("pulse: could not open, {error}");
                return false;
            }
        };
        let now = self.moments.len();
        // the moment before the newest action, as Three Houses opens on it
        let selected = now.saturating_sub(1);
        self.screen = Some(Screen { selected: now, shown: now, now_id, game_over, asking: None, frames_left: 0, opened: Instant::now(), notice: None });
        crate::pulse_fx::opened(); // pulse2: the activation effect
        self.select(selected);
        println!("pulse: frame {}{} the rewind opened{} with {} moments, {} charges left", self.frame, wav_at(), if game_over { " for a lost battle" } else { "" }, self.moments.len(), self.left().map_or("unlimited".into(), |n| n.to_string()));
        true
    }

    /// moves the screen's choice to `index` (moments.len() is now).
    pub fn select(&mut self, index: usize) {
        let count = self.moments.len();
        if let Some(screen) = &mut self.screen {
            screen.selected = index.min(count);
        }
    }

    /// the id of the moment at `index` (now's at moments.len()).
    fn id_at(&self, index: usize) -> Option<u64> {
        let screen = self.screen.as_ref()?;
        if index >= self.moments.len() {
            Some(screen.now_id)
        } else {
            Some(self.moments[index].id)
        }
    }

    /// turns time back to the moment chosen, at the cost of a charge.
    pub fn confirm(&mut self) -> Result<(), String> {
        let Some(screen) = &self.screen else { return Ok(()) };
        let index = screen.selected;
        if index >= self.moments.len() {
            return Ok(());
        }
        if self.left() == Some(0) {
            return Err("No charges of Divine Pulse are left on this map.".into());
        }
        self.pending = Some(Pending::Rewind(index));
        Ok(())
    }

    /// closes the screen, putting back the moment it was opened at.
    pub fn cancel(&mut self) {
        if self.screen.is_some() {
            self.pending = Some(Pending::Cancel);
        }
    }
}

/// what the screen asked for, done between two frames.
#[derive(Clone, Copy, Debug)]
pub enum Pending {
    Rewind(usize),
    Cancel,
}

/// pulse2: frames the System Menu gets to close (its own animation) before
/// the rewind screen opens over the map
const MENU_CLOSING: u32 = 10;

/// frames run without input after a moment is loaded for the screen, so the
/// map shows it
const PREVIEW_FRAMES: u32 = 2;

/// how the frame about to run goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Run {
    Normal,
    /// no input reaches the game and its sound is dropped
    Quiet,
    /// pulse2: the frame after a rewind made or cancelled: no input reaches
    /// the game, but its sound is heard (the music going on from the
    /// moment, its voices put back with it)
    Resumed,
}

/// before each frame of the game, its input already set: the screen's work
/// (a moment loaded for it, a rewind made, the screen closed). While the
/// screen is up the game gets no input.
pub fn before_frame(system: &mut System) -> Run {
    if !with(|p| p.screen.is_some()) {
        // pulse2: the System Menu closing before the screen opens
        if with(|p| p.menu_closing > 0) {
            system.set_input(zakuro_core::services::hid::InputState::default());
        }
        return Run::Normal;
    }
    system.set_input(zakuro_core::services::hid::InputState::default());
    with(|p| p.before(system))
}

impl Pulse {
    fn before(&mut self, system: &mut System) -> Run {
        if let Some(pending) = self.pending.take() {
            let now_id = self.screen.as_ref().map_or(0, |s| s.now_id);
            crate::pulse_fx::closed(matches!(pending, Pending::Rewind(_))); // pulse2: the rewind's transition
            let Some(rewind) = self.rewind.as_mut() else {
                self.screen = None;
                return Run::Normal;
            };
            match pending {
                Pending::Rewind(index) => {
                    let id = self.moments[index].id;
                    match rewind.restore(system, id, true) {
                        Ok(cost) => {
                            check_line(rewind, &format!("rewind to moment {id}"));
                            self.stats.rewinds += 1;
                            self.stats.rewind_time += cost.total;
                            self.stats.rewind_max = self.stats.rewind_max.max(cost.total);
                            let moment = &self.moments[index];
                            println!(
                                "pulse: frame {}{} turned back to moment {id} (turn {}, {} phase, kept at frame {}) in {:.1?} ({} pages written, the rest {:.1?}); {} left",
                                self.frame,
                                wav_at(),
                                moment.turn,
                                phase_name(moment.phase),
                                moment.frame,
                                cost.total,
                                cost.pages,
                                cost.rest,
                                self.limit.map_or("unlimited".into(), |l| l.saturating_sub(self.used + 1).to_string())
                            );
                            // the console as the moment had it: its units, its
                            // random numbers, and (tests) every byte of memory
                            let (units_now, _) = units(&mut system.memory);
                            let rng_now = rng(&mut system.memory);
                            let memory_now = memory_hash(system);
                            let moment = &self.moments[index];
                            println!(
                                "pulse: after the rewind: turn {:?} (moment {}), units match {}, random numbers match {} ({:08X?}), memory match {}",
                                situation(&mut system.memory),
                                moment.turn,
                                units_now == moment.units,
                                rng_now == moment.rng,
                                rng_now,
                                match (memory_now, moment.memory) {
                                    (Some(a), Some(b)) => (a == b).to_string(),
                                    _ => "not checked".to_owned(),
                                }
                            );
                            self.moments.truncate(index + 1);
                            if let Some(last) = self.moments.last_mut() {
                                // who acts next is up to the player again
                                if last.phase == 0 {
                                    last.actor = None;
                                }
                            }
                            self.used += 1;
                            self.toast = Some(match self.left() {
                                Some(left) => format!("Divine Pulse: {left} {} left", if left == 1 { "charge" } else { "charges" }),
                                None => "Divine Pulse".to_owned(),
                            });
                        }
                        Err(error) => {
                            log::error!("pulse: the rewind failed, {error}");
                            println!("pulse: frame {} the rewind failed, {error}", self.frame);
                            // the console as it was opened, if that comes back
                            let _ = rewind.restore(system, now_id, true);
                        }
                    }
                }
                Pending::Cancel => match rewind.restore(system, now_id, true) {
                    Ok(cost) => {
                        println!("pulse: frame {}{} back to the present in {:.1?}", self.frame, wav_at(), cost.total);
                        check_line(rewind, "back to the present");
                    }
                    Err(error) => log::error!("pulse: the present could not be put back, {error}"),
                },
            }
            self.screen = None;
            self.loaded = true;
            // the counters as the console has them now, so nothing reads as new
            self.last_free = read(word::FREE_FRAME);
            self.was_free = false;
            self.last_ai = read(word::AI_CALLS);
            self.last_game_over = read(word::GAME_OVER);
            self.last_menu = read(word::MENU_REQUEST);
            return Run::Resumed;
        }
        let Some(screen) = self.screen.as_ref() else { return Run::Normal };
        if screen.selected != screen.shown {
            let index = screen.selected;
            let Some(id) = self.id_at(index) else { return Run::Quiet };
            let Some(rewind) = self.rewind.as_mut() else { return Run::Quiet };
            crate::pulse_fx::shown(index < screen.shown); // pulse2: the picture's cross-fade
            match rewind.restore(system, id, false) {
                Ok(cost) => {
                    self.stats.previews += 1;
                    self.stats.preview_time += cost.total;
                    self.stats.preview_max = self.stats.preview_max.max(cost.total);
                    log::info!("pulse: moment {id} shown in {:.1?} ({} pages)", cost.total, cost.pages);
                    check_line(rewind, &format!("moment {id} shown"));
                }
                Err(error) => log::warn!("pulse: moment {id} could not be shown, {error}"),
            }
            self.loaded = true;
            if let Some(screen) = self.screen.as_mut() {
                screen.shown = index;
                screen.frames_left = PREVIEW_FRAMES;
            }
        }
        if let Some(screen) = self.screen.as_mut() {
            screen.frames_left = screen.frames_left.saturating_sub(1);
        }
        Run::Quiet
    }

    fn after(&mut self, system: &mut System, frame: u64) {
        self.frame = frame;
        let active = self.on && HOOKS.get().is_some() && !zakuro_core::netplay::battle();
        write(word::ON, active as u32);
        if !active {
            if self.map.is_some() || self.screen.is_some() {
                self.forget();
            }
            write(word::MENU_ITEM, 0);
            return;
        }
        if self.screen.is_some() {
            self.open_asked = false;
            // the frames the screen runs keep nothing
            self.last_free = read(word::FREE_FRAME);
            self.last_ai = read(word::AI_CALLS);
            self.last_game_over = read(word::GAME_OVER);
            self.last_menu = read(word::MENU_REQUEST);
            return;
        }
        let Some((turn, phase)) = situation(&mut system.memory) else {
            if self.map.is_some() {
                println!("pulse: frame {frame} the map ended: {} moments let go", self.moments.len());
                self.forget();
            }
            write(word::MENU_ITEM, 0);
            return;
        };
        self.now_turn = (turn, phase);
        let sit = r32(&mut system.memory, zakuro_core::region::us(SITUATION));
        let cid = chapter_cid(&mut system.memory);
        if self.map.as_ref().is_none_or(|(at, was)| *at != sit || *was != cid) {
            // a new map: no moments, the charges full again
            self.forget();
            if let Some(number) = chapter_number(&cid) {
                self.story = self.story.max(number);
            }
            self.map = Some((sit, cid.clone()));
            self.limit = self.limit(&mut system.memory);
            self.last_free = read(word::FREE_FRAME);
            self.was_free = false;
            self.last_ai = read(word::AI_CALLS);
            self.last_game_over = read(word::GAME_OVER);
            self.last_menu = read(word::MENU_REQUEST);
            let (difficulty, classic) = difficulty(&mut system.memory);
            println!(
                "pulse: frame {frame} map {cid} (difficulty {difficulty}, {}): {} charges",
                if classic { "Classic" } else { "Casual/Phoenix" },
                self.limit.map_or("unlimited".into(), |l| l.to_string())
            );
        }
        if self.limit.is_none() && self.charges != PulseCharges::Unlimited {
            self.limit = self.limit(&mut system.memory);
        }
        write(word::MENU_ITEM, 1);

        // the free cursor back: a moment before the player's next action
        let free = read(word::FREE_FRAME);
        let ticked = free != self.last_free;
        self.last_free = free;
        self.free_idle = if ticked { 0 } else { self.free_idle.saturating_add(1) };
        let rising = ticked && !self.was_free;
        self.was_free = ticked;
        if rising && phase == 0 {
            let (list, hash) = units(&mut system.memory);
            if self.moments.last().is_none_or(|last| last.hash != hash || last.turn != turn || last.phase != phase) {
                self.keep(system, turn, phase, None, list, hash);
            }
        }
        // a unit of the enemy or another force decided: a moment before it acts
        let ai = read(word::AI_CALLS);
        if ai != self.last_ai {
            self.last_ai = ai;
            if phase != 0 {
                let (list, hash) = units(&mut system.memory);
                let index = read(word::AI_UNIT) as u16;
                let actor = list.iter().find(|u| u.index == index).cloned();
                let actor = Some(actor.map_or_else(|| (phase_name(phase).to_owned(), phase), |u| (self.name_of(system, &u), u.force)));
                self.keep(system, turn, phase, actor, list, hash);
            }
        }
        // the battle is about to be lost: the rewind offers itself
        let lost = read(word::GAME_OVER);
        if lost != self.last_game_over {
            self.last_game_over = lost;
            println!("pulse: frame {frame} the battle is lost (SetGameOver {})", read(word::GAME_OVER_TYPE));
            if self.left() != Some(0) && !self.moments.is_empty() {
                self.open(system, true);
            }
        }
        // the System Menu's Divine Pulse (pulse2: once the menu has closed)
        let menu = read(word::MENU_REQUEST);
        if menu != self.last_menu {
            self.last_menu = menu;
            self.menu_closing = MENU_CLOSING;
        } else if self.menu_closing > 0 {
            self.menu_closing -= 1;
            if self.menu_closing == 0 {
                self.open(system, false);
            }
        }
        // its key or button (or a UI script's `pulse`)
        if crate::input_api::take_action(crate::controls::PcAction::DivinePulse) || std::mem::take(&mut self.open_asked) {
            if phase == 0 && self.free_idle < 4 {
                self.open(system, false);
            } else {
                self.toast = Some("Divine Pulse can be used while choosing a unit on your turn".into());
            }
        }
    }

    fn script_due(&mut self, frame: u64) -> Vec<String> {
        if !self.script_loaded {
            self.script_loaded = true;
            if let Ok(text) = std::env::var("FATES_PULSE_AT") {
                self.script = text
                    .split(';')
                    .filter_map(|part| {
                        let (at, command) = part.split_once(':')?;
                        Some((at.trim().parse().ok()?, command.trim().to_owned()))
                    })
                    .collect();
            }
        }
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.script).into_iter().partition(|(at, _)| *at <= frame);
        self.script = later;
        now.into_iter().map(|(_, command)| command).collect()
    }

    fn command(&mut self, system: &mut System, command: &str) {
        let (verb, rest) = command.split_once(' ').unwrap_or((command, ""));
        let count = self.moments.len();
        let selected = self.screen.as_ref().map_or(count, |s| s.selected);
        match verb {
            "open" => {
                self.open(system, false);
            }
            "left" => self.select(selected.saturating_sub(rest.parse().unwrap_or(1))),
            "right" => self.select(selected + rest.parse().unwrap_or(1)),
            // back n: the moment n before now
            "back" => self.select(count.saturating_sub(rest.parse().unwrap_or(1))),
            // to n: moment n, counted from the map's first
            "to" => self.select(rest.parse().unwrap_or(0)),
            "confirm" => {
                if let Err(error) = self.confirm() {
                    println!("pulse: {error}");
                }
            }
            "cancel" => self.cancel(),
            "list" => self.list(),
            // tests: the battle lost, as SetGameOver tells it (the hook's
            // count goes up), for the screen's own way in
            "lose" => write(word::GAME_OVER, read(word::GAME_OVER).wrapping_add(1)),
            // pulsefix, tests: Enemy Targeting Lines turned on or off in the
            // middle of a map (as Options does), so the lines' buffer is made
            // after the map's first moment
            "lines" => crate::info_ui::write(crate::info_ui::word::MODE, rest.parse().unwrap_or(1)),
            // tests: Yes on the question up (Accept Defeat's, or a rewind's)
            "yes" => {
                let accept = self.screen.as_ref().is_some_and(|s| s.selected >= self.moments.len());
                if accept {
                    self.cancel();
                } else if let Err(error) = self.confirm() {
                    println!("pulse: {error}");
                }
            }
            other => println!("pulse: unknown command {other}"),
        }
        println!("pulse: frame {} command {command}: selected {:?} of {}", self.frame, self.screen.as_ref().map(|s| s.selected), self.moments.len());
    }

    fn list(&self) {
        for (i, moment) in self.moments.iter().enumerate() {
            let actor = moment.actor.as_ref().map_or("?".to_owned(), |(name, force)| format!("{name} ({})", phase_name(*force)));
            println!("pulse: moment {i} id {} frame {} turn {} {} phase{}: next {actor}, random numbers {:08X?}", moment.id, moment.frame, moment.turn, phase_name(moment.phase), if moment.phase_start { " (start)" } else { "" }, moment.rng);
            for u in &moment.units {
                println!("pulse:   unit {} force {} at {},{} hp {} acted {}", u.index, u.force, u.x, u.y, u.hp, u.acted);
            }
        }
        if let Some(rewind) = &self.rewind {
            let (shadow, base, snaps) = rewind.bytes();
            println!("pulse: store: shadow {} MiB, base {} KiB, moments {} KiB, folded {}", shadow >> 20, base >> 10, snaps >> 10, rewind.folded);
        }
    }
}

/// pulse2, tests: where in FATES_AUDIO_WAV's recording this is.
fn wav_at() -> String {
    zakuro_core::pcos::snd::wav::position().map_or(String::new(), |n| format!(" (wav sample {n})"))
}

pub fn phase_name(force: u8) -> &'static str {
    match force {
        0 => "Player",
        1 => "Enemy",
        _ => "Other",
    }
}

/// after each frame of the game: the moments worth keeping, and what opens
/// the screen.
pub fn after_frame(system: &mut System, frame: u64) {
    with(|p| p.after(system, frame));
}

/// a headless frame, before it runs: the test commands due.
pub fn headless_frame(system: &mut System, frame: u64) {
    with(|p| {
        p.frame = frame;
        for command in p.script_due(frame) {
            p.command(system, &command);
        }
    });
}

/// what a headless run prints at its end.
pub fn summary() -> Option<String> {
    with(|p| {
        if !p.on {
            return None;
        }
        let s = &p.stats;
        let mean = |total: Duration, n: u64| if n == 0 { Duration::ZERO } else { total / n as u32 };
        let store = p.rewind.as_ref().map(|r| r.bytes()).unwrap_or_default();
        Some(format!(
            "pulse: {} moments kept (mean {:.1?}, max {:.1?}, {} KiB each on average), {} shown (mean {:.1?}, max {:.1?}), {} rewinds (mean {:.1?}, max {:.1?}); store: shadow {} MiB, base {} MiB, moments {} MiB",
            s.snapshots,
            mean(s.snap_time, s.snapshots),
            s.snap_max,
            if s.snapshots == 0 { 0 } else { s.snap_bytes / s.snapshots / 1024 },
            s.previews,
            mean(s.preview_time, s.previews),
            s.preview_max,
            s.rewinds,
            mean(s.rewind_time, s.rewinds),
            s.rewind_max,
            store.0 >> 20,
            store.1 >> 20,
            store.2 >> 20
        ))
    })
}

/// what the screen draws: a copy of what it needs.
pub struct View {
    /// each moment: turn, phase, the first of its phase, who acts next
    pub moments: Vec<(u16, u8, bool, Option<(String, u8)>)>,
    pub selected: usize,
    pub left: Option<u32>,
    pub limit: Option<u32>,
    pub game_over: bool,
    pub asking: Option<bool>,
    pub opened: Instant,
    pub notice: Option<String>,
    /// the turn and phase the screen was opened in
    pub turn_now: (u16, u8),
}

pub fn view() -> Option<View> {
    with(|p| {
        let screen = p.screen.as_ref()?;
        Some(View {
            moments: p.moments.iter().map(|m| (m.turn, m.phase, m.phase_start, m.actor.clone())).collect(),
            selected: screen.selected,
            left: p.left(),
            limit: p.limit,
            game_over: screen.game_over,
            asking: screen.asking,
            opened: screen.opened,
            notice: screen.notice.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(3)).map(|(text, _)| text.clone()),
            turn_now: p.now_turn,
        })
    })
}

/// the screen's question: Some(yes chosen) while it is up.
pub fn set_asking(asking: Option<bool>) {
    with(|p| {
        if let Some(screen) = &mut p.screen {
            screen.asking = asking;
        }
    });
}

/// a line the screen shows for a moment.
pub fn notice(text: &str) {
    with(|p| {
        if let Some(screen) = &mut p.screen {
            screen.notice = Some((text.to_owned(), Instant::now()));
        }
    });
}

/// statecheck: how many moments are kept now (the log, at a new start).
pub fn moments_kept() -> usize {
    with(|p| p.moments.len())
}

/// a save state was loaded: the moments are of another console now.
pub fn state_loaded() {
    with(|p| {
        p.forget();
        p.limit = None;
    });
}

/// a chapter's name as the game writes it ("Chapter 5: Lost in the Ice"),
/// from its CID.
pub fn chapter_name(system: &mut System, cid: &str) -> String {
    with(|p| {
        p.load_names(system);
        let Some(rest) = cid.strip_prefix("CID_") else { return String::new() };
        let key = format!("MCID_{rest}").into_bytes();
        let name = p.names.get(&key).cloned().unwrap_or_default();
        let mut prefix_key = key.clone();
        prefix_key.extend_from_slice(b"_PREFIX");
        match p.names.get(&prefix_key) {
            Some(prefix) if !prefix.is_empty() && !name.is_empty() => format!("{prefix}: {name}"),
            _ => name,
        }
    })
}

/// after a rewind, whether a replay goes back to the moment's frame (its
/// same inputs played again: the headless tests' redo) or on from where it
/// was (a window's: a recorded demo's next inputs are the turn played
/// differently). FATES_PULSE_FRAME=reset|keep chooses either way.
pub fn replays_again(headless: bool) -> bool {
    match std::env::var("FATES_PULSE_FRAME").as_deref() {
        Ok("reset") => true,
        Ok("keep") => false,
        _ => headless,
    }
}

/// a UI script's `pulse`: the screen opens as its key opens it.
pub fn request_open() {
    with(|p| p.open_asked = p.screen.is_none());
}
