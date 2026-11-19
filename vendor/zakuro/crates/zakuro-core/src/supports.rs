//! supports: Corrin can S-support anyone, and Kana's gender can be chosen.
//!
//! The host side of overrides/supports.c (see there for the game's side).
//!
//! **S with anyone.** The game's supports ("Reliance") are lists of 12-byte
//! entries per character in GameData's person section: +0 the partner's
//! person id, +4..+7 the points C, B, A and S need (0xFF: no such rank),
//! +9 (high) and +10 the pair's conversation index (C, B, A, then S). When
//! GameData is loaded (Person::Initialize), every pair of a Corrin that has
//! no S, where the other Corrin has an S with the same partner, gets that
//! S threshold, in Corrin's list and in the partner's. Marriage is any pair
//! at S (Unit::GetMarrigePid), so the ring, the children's parent slots
//! (Unit::CreateImpl: the fixed parent and their spouse), the paralogues and
//! My Castle follow. The S conversation is Haiku's own message, added to
//! the pair's file in the builtin RomFS layer (dev/supports/make_text.py),
//! under the game's own label for it.
//!
//! The pair's conversation index + 3 is another pair's C. A same-gender S
//! marks the other Corrin's S with that partner as seen instead (the same
//! conversation), and the support log (Extras) shows the game's own pairs:
//! the S thresholds are put back while it is open.
//!
//! **Kana's gender.** Kana's paralogue (X002) picks Kana by asking whether
//! the player is PID_プレイヤー女 (a female Corrin has a son). The answer is
//! the chosen Kana's (`kana_gender`): Ask shows the game's own three-choice
//! dialog when the paralogue starts. A Kana of Corrin's own gender gets
//! Corrin as the fixed parent (Person +0x2A), and the paralogue's lines
//! (labelled by Corrin's gender) are answered with Haiku's variants
//! (`<label>_HK`) written for that Kana.
//!
//! **Text by Corrin's gender.** Labels the game looks up without Corrin's
//! talk tone (the Live2D lines of a spouse) are answered with
//! `<label>_PCM1` / `<label>_PCF1` when Haiku's layer has one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

use recomp_abi::Context;
use zakuro_cpu::Bus;

use crate::memory::{Memory, MemoryRegion, MemoryState, Permission};
use crate::System;

/// the overrides' operations (overrides/supports.c, SP_*)
pub mod op {
    pub const PERSON_INIT: u32 = 1;
    pub const TALK: u32 = 2;
    pub const FLAG_SET: u32 = 3;
    pub const MUSEUM_ENTER: u32 = 4;
    pub const MUSEUM_EXIT: u32 = 5;
    pub const IDENT: u32 = 6;
    pub const IS_PID: u32 = 7;
    pub const CALL_C: u32 = 8;
    pub const ASK_TEXT: u32 = 9;
}

/// Kana's gender, as the Options say: asked, as in the game, or chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KanaGender {
    #[default]
    Ask,
    Vanilla,
    Male,
    Female,
    Random,
}

impl KanaGender {
    fn code(self) -> u32 {
        match self {
            KanaGender::Ask => 0,
            KanaGender::Vanilla => 1,
            KanaGender::Male => 2,
            KanaGender::Female => 3,
            KanaGender::Random => 4,
        }
    }
    fn from_code(code: u32) -> Self {
        match code {
            1 => KanaGender::Vanilla,
            2 => KanaGender::Male,
            3 => KanaGender::Female,
            4 => KanaGender::Random,
            _ => KanaGender::Ask,
        }
    }
}

static S_ANY: AtomicBool = AtomicBool::new(true);
static KANA: AtomicU32 = AtomicU32::new(0);
static TEXT_WORD: AtomicPtr<u32> = AtomicPtr::new(std::ptr::null_mut());

/// the PC settings: Corrin's S with anyone (read when GameData loads, so a
/// change takes effect at the next start) and Kana's gender.
pub fn set_settings(s_any: bool, kana: KanaGender) {
    let s_any = env_flag("FATES_SUPPORTS_S_ANY").unwrap_or(s_any);
    let kana = std::env::var("FATES_KANA_GENDER").ok().and_then(|v| parse_kana(&v)).unwrap_or(kana);
    S_ANY.store(s_any, Ordering::Relaxed);
    KANA.store(kana.code(), Ordering::Relaxed);
    update_text_word();
}

fn trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("FATES_SUPPORTS_TRACE").is_some())
}

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| v == "1")
}

fn parse_kana(v: &str) -> Option<KanaGender> {
    Some(match v.to_ascii_lowercase().as_str() {
        "ask" => KanaGender::Ask,
        "vanilla" => KanaGender::Vanilla,
        "male" => KanaGender::Male,
        "female" => KanaGender::Female,
        "random" => KanaGender::Random,
        _ => return None,
    })
}

/// the override's word that turns the message hook on (haiku_supports_text).
///
/// # Safety
///
/// the pointer has to stay valid for the program's life.
pub unsafe fn set_text_word(word: *mut u32) {
    TEXT_WORD.store(word, Ordering::Relaxed);
    update_text_word();
}

fn update_text_word() {
    let word = TEXT_WORD.load(Ordering::Relaxed);
    if word.is_null() {
        return;
    }
    let s_any = S_ANY.load(Ordering::Relaxed);
    let text = s_any || KanaGender::from_code(KANA.load(Ordering::Relaxed)) != KanaGender::Vanilla;
    // SAFETY: set_text_word's contract; a plain word the code only reads.
    // bit 0: the message hook; bit 1: S with anyone (child paralogues)
    unsafe { std::ptr::write_volatile(word, text as u32 | (s_any as u32) << 1) };
}

// -- the game's memory --------------------------------------------------------

const PERSON_LIST: u32 = 0x006D_3B28; // Person's list of loaded tables; GameData's first
const PERSON_SIZE: u32 = 0x98;
const GAME_USER_DATA: u32 = 0x006D_A248; // GameUserData*: +0xC the chapter, +0x20 the script values
const UNIT_POOL: u32 = 0x006D_9F40;
const FORCES: u32 = 0x006D_9F44; // 12 bytes a force: the first unit, units linked at +0x14
const UNIT_SIZE: u32 = 0x140;
const MESS_HASH: u32 = 0x006D_CB44;
/// the script value the event dialogs leave their choice in (S_イベント選択肢)
const EVENT_CHOICE: u32 = 47;
/// guest pages Haiku may map for the ask's labels (after the randomizer's)
const PAGE_ADDRESSES: [u32; 3] = [0x0D00_0000, 0x0D80_0000, 0x0C80_0000];
const PAGE_SIZE: u32 = 0x1000;

/// the PIDs, Shift JIS as in the game's data
const PID_MALE: &[u8] = b"PID_\x83\x76\x83\x8c\x83\x43\x83\x84\x81\x5b\x92\x6a"; // PID_プレイヤー男
const PID_FEMALE: &[u8] = b"PID_\x83\x76\x83\x8c\x83\x43\x83\x84\x81\x5b\x8f\x97"; // PID_プレイヤー女
const PID_KANA_M: &[u8] = b"PID_\x83\x4a\x83\x93\x83\x69\x92\x6a"; // PID_カンナ男
/// カンナ, in a label
const KANA_LABEL: &[u8] = b"\x83\x4a\x83\x93\x83\x69";
const PID_NILES: &[u8] = b"PID_\x83\x5b\x83\x8d"; // PID_ゼロ
const PID_RHAJAT: &[u8] = b"PID_\x83\x56\x83\x83\x83\x89"; // PID_シャラ
const PID_KANA_F: &[u8] = b"PID_\x83\x4a\x83\x93\x83\x69\x8f\x97"; // PID_カンナ女

fn r8(m: &mut Memory, a: u32) -> u8 {
    m.read8(a)
}
fn r16(m: &mut Memory, a: u32) -> u16 {
    m.read16(a)
}
fn r32(m: &mut Memory, a: u32) -> u32 {
    m.read32(a)
}

fn word(m: &mut Memory, a: u32) -> u32 {
    if a == 0 || !m.is_readable(a, 4) {
        return 0;
    }
    m.read32(a)
}

fn cstr(m: &mut Memory, a: u32, max: usize) -> Vec<u8> {
    if a == 0 || !m.is_readable(a, 1) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..max as u32 {
        if !m.is_readable(a + i, 1) {
            break;
        }
        let b = m.read8(a + i);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

/// a person in GameData's section
#[derive(Clone, Debug)]
struct PersonAt {
    at: u32,
    id: u16,
    rid: i16,
    pid: Vec<u8>,
    female: bool,
}

fn persons(m: &mut Memory) -> Vec<PersonAt> {
    let node = word(m, crate::region::us(PERSON_LIST));
    let section = word(m, node);
    if section == 0 || !m.is_readable(section, 0x10) {
        return Vec::new();
    }
    let count = r16(m, section + 4) as u32;
    let mut out = Vec::new();
    for i in 0..count.min(1024) {
        let at = section + 0x10 + i * PERSON_SIZE;
        if !m.is_readable(at, PERSON_SIZE) {
            break;
        }
        let name = r32(m, at + 8);
        let pid = cstr(m, name, 64);
        let female = r8(m, at) & 1 != 0;
        out.push(PersonAt { at, id: r16(m, at + 0x24), rid: r16(m, at + 0x30) as i16, pid, female });
    }
    out
}

/// GameData's reliance lists: (address, reliance id, count)
fn lists(m: &mut Memory) -> Vec<(u32, u16, u16)> {
    let node = word(m, crate::region::us(PERSON_LIST));
    let section = word(m, node);
    let table = word(m, section + 8);
    if table == 0 {
        return Vec::new();
    }
    let n = r32(m, table).min(1024);
    let mut out = Vec::new();
    for i in 0..n {
        let list = word(m, table + 4 + i * 4);
        if list == 0 || !m.is_readable(list, 4) {
            continue;
        }
        let rid = r16(m, list);
        let count = r16(m, list + 2);
        if m.is_readable(list, 4 + count as u32 * 12) {
            out.push((list, rid, count));
        }
    }
    out
}

fn entry_for(m: &mut Memory, lists: &[(u32, u16, u16)], rid: i16, partner: u16) -> Option<u32> {
    if rid < 0 {
        return None;
    }
    for &(list, r, count) in lists {
        if r != rid as u16 {
            continue;
        }
        for k in 0..count as u32 {
            let e = list + 4 + k * 12;
            if r16(m, e) == partner {
                return Some(e);
            }
        }
    }
    None
}

fn conversation(m: &mut Memory, e: u32) -> u32 {
    ((r8(m, e + 9) as u32) << 16) | r16(m, e + 10) as u32
}

/// the units in the forces' lists: (unit, its person's PID)
fn units(m: &mut Memory) -> Vec<(u32, Vec<u8>)> {
    let forces = word(m, crate::region::us(FORCES));
    let mut out = Vec::new();
    if forces == 0 {
        return out;
    }
    for force in 0..8u32 {
        let mut unit = word(m, forces + force * 12);
        let mut seen = 0;
        while unit != 0 && seen < 256 {
            seen += 1;
            let person = word(m, unit + 0x9C);
            if person != 0 {
                let name = word(m, person + 8);
                out.push((unit, cstr(m, name, 32)));
            }
            unit = word(m, unit + 0x14);
        }
    }
    out
}

/// tones: Corrin's unit::Edit (looks, voice, speaking style), which the
/// unit in the army points at (+0xB0, as ProcTalkManager::Initialize reads it)
pub(crate) fn corrin_edit(m: &mut Memory) -> Option<u32> {
    let (unit, _) = units(m).into_iter().find(|(_, pid)| pid == PID_MALE || pid == PID_FEMALE)?;
    let edit = word(m, unit + 0xB0);
    (edit != 0 && m.is_readable(edit, 0x30)).then_some(edit)
}

/// whether Corrin (the unit in the army) is female
fn corrin_female(m: &mut Memory) -> Option<bool> {
    units(m).into_iter().find_map(|(_, pid)| {
        if pid == PID_FEMALE {
            Some(true)
        } else if pid == PID_MALE {
            Some(false)
        } else {
            None
        }
    })
}

/// Corrin's spouse, as Unit::GetMarrigePid finds it: the partner of the
/// first support in Corrin's list at S (the points, Unit::GetReliance, are
/// in the unit that owns the entry: Corrin's when +8 is set, else the
/// partner's, at the entry's index +2). Returns (the spouse, Corrin female).
fn spouse(m: &mut Memory) -> Option<(PersonAt, bool)> {
    let army = units(m);
    let &(unit, ref pid) = army.iter().find(|(_, pid)| pid == PID_MALE || pid == PID_FEMALE)?;
    let female = pid == PID_FEMALE;
    let person = word(m, unit + 0x9C);
    let rid = r16(m, person + 0x30) as i16;
    let all = lists(m);
    let &(list, _, count) = all.iter().find(|&&(_, r, _)| r as i16 == rid)?;
    for k in 0..count as u32 {
        let e = list + 4 + k * 12;
        let s = r8(m, e + 7);
        if s == 0xFF {
            continue;
        }
        let partner = r16(m, e);
        let owner = if r8(m, e + 8) != 0 {
            Some(unit)
        } else {
            army.iter().map(|&(u, _)| u).find(|&u| {
                let p = word(m, u + 0x9C);
                p != 0 && r16(m, p + 0x24) == partner
            })
        };
        let Some(owner) = owner else { continue };
        let points = word(m, owner + 0x11C);
        if points == 0 {
            continue;
        }
        let index = r16(m, e + 2) as u32;
        if r8(m, points + index) < s {
            continue;
        }
        return persons(m).into_iter().find(|p| p.id == partner).map(|p| (p, female));
    }
    None
}

/// the child whose other parent is Corrin by a same-gender S: the person
/// whose fixed parent (+0x2A) is Corrin's spouse of Corrin's gender (not
/// Kana, Corrin's own child): its PID
fn co_child(m: &mut Memory) -> Option<Vec<u8>> {
    let (sp, female) = spouse(m)?;
    if sp.female != female {
        return None;
    }
    let people = persons(m);
    people.into_iter().find(|p| p.pid != PID_KANA_M && p.pid != PID_KANA_F && r16(m, p.at + 0x2A) == sp.id).map(|p| p.pid)
}

/// Kana in the army or on the map: whether Kana is male
fn kana_in_army(m: &mut Memory) -> Option<bool> {
    units(m).into_iter().find_map(|(_, pid)| {
        if pid == PID_KANA_M {
            Some(true)
        } else if pid == PID_KANA_F {
            Some(false)
        } else {
            None
        }
    })
}

fn chapter(m: &mut Memory) -> Vec<u8> {
    let gud = word(m, crate::region::us(GAME_USER_DATA));
    let chapter = word(m, gud + 0xC);
    let name = word(m, chapter);
    cstr(m, name, 32)
}

/// IdentHash::GetIdent, read here: the node of a label, or 0.
fn ident(m: &mut Memory, hash: u32, label: &[u8]) -> u32 {
    if hash == 0 || label.is_empty() || !m.is_readable(hash, 0x14) {
        return 0;
    }
    let (mut h1, mut h2) = (0u32, 0u32);
    for &b in label {
        let c = b as i8 as i32 as u32;
        h1 = h1.wrapping_mul(37).wrapping_add(c);
        h2 = h2.wrapping_mul(31).wrapping_add(c);
    }
    let size = r32(m, hash + 4);
    let table = r32(m, hash + 0x10);
    if size == 0 || table == 0 {
        return 0;
    }
    let mut node = word(m, table + (h1 % size) * 4);
    let mut guard = 0;
    while node != 0 && guard < 4096 {
        guard += 1;
        if word(m, node + 4) == h2 {
            return if word(m, node + 0xC) != 0 { node } else { 0 };
        }
        node = word(m, node);
    }
    0
}

// -- the state ----------------------------------------------------------------

#[derive(Default)]
struct State {
    system: usize,
    /// (address, the game's byte, Haiku's byte) of every S threshold filled in
    patches: Vec<(u32, u8, u8)>,
    applied: bool,
    /// same-gender S conversation index -> the other Corrin's S with the partner
    pairs: HashMap<u32, u32>,
    /// (Corrin's person id, the partner's) of the pairs given an S
    pair_ids: HashMap<(u16, u16), (u32, u32)>,
    /// the S flag the current conversation would mark, and Haiku's
    pending: Option<(u32, u32)>,
    corrin_ids: (u16, u16),
    /// Kana's gender decided for this run of the paralogue (true: male)
    kana: Option<bool>,
    asked: bool,
    page: u32,
    /// GameData's lists have been read, from this table
    read: bool,
    table: u32,
    kana_parent_patched: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// the hook the overrides call (haiku_supports_hook).
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
        *state = State { system: key, ..State::default() };
    }
    // GameData read once per system (a saved state loaded holds GameData
    // already: Person::Initialize ran before it was saved), and the S ranks
    // checked again where memory may have been replaced by a state
    if op != op::PERSON_INIT {
        let table = reliance_table(&mut system.memory);
        if table != 0 && (!state.read || table != state.table) {
            person_init(state, system);
        } else if state.applied && matches!(op, op::TALK | op::IDENT) {
            if let Some(&(at, game, _)) = state.patches.first() {
                if r8(&mut system.memory, at) == game {
                    state.applied = false;
                    apply(state, &mut system.memory);
                }
            }
        }
    }
    if trace() && matches!(op, op::IS_PID | op::CALL_C | op::ASK_TEXT) {
        log::info!("supports: op {op} {a:#x} {b:#x}");
    }
    match op {
        op::PERSON_INIT => {
            person_init(state, system);
            0
        }
        op::TALK => talk(state, system, a),
        op::FLAG_SET => match state.pending.take() {
            Some((from, to)) if from == a => {
                log::info!("supports: a same-gender S marks the other Corrin's S ({to:#x}) as seen, not {from:#x}");
                to
            }
            _ => a,
        },
        op::MUSEUM_ENTER => {
            unapply(state, &mut system.memory);
            0
        }
        op::MUSEUM_EXIT => {
            if S_ANY.load(Ordering::Relaxed) {
                apply(state, &mut system.memory);
            }
            0
        }
        op::IDENT => {
            let node = ident_hook(state, &mut system.memory, a, b);
            // fanpatch: HAIKU_MSG_TRACE, the lines the game finds no message for
            if crate::msgtrace::enabled() && a == word(&mut system.memory, crate::region::us(MESS_HASH)) {
                let label = cstr(&mut system.memory, b, 128);
                let found = if node != 0 { node } else { ident(&mut system.memory, a, &label) };
                // SAFETY: as above, ctx is the running code's context
                let lr = unsafe { (*(ctx as *mut Context)).r[14] };
                crate::msgtrace::lookup(&label, lr, found);
            }
            node
        }
        op::IS_PID => is_pid(state, &mut system.memory, a, b),
        op::CALL_C => call_c(state, system, a, b),
        op::ASK_TEXT => ask_text(state, system, a),
        _ => 0,
    }
}

/// GameData's reliance table (its address tells a GameData loaded anew)
fn reliance_table(m: &mut Memory) -> u32 {
    let node = word(m, crate::region::us(PERSON_LIST));
    let section = word(m, node);
    word(m, section + 8)
}

fn person_init(state: &mut State, system: &mut System) {
    let m = &mut system.memory;
    state.table = reliance_table(m);
    state.read = state.table != 0;
    let people = persons(m);
    let all = lists(m);
    let find = |pid: &[u8]| people.iter().find(|p| p.pid == pid).cloned();
    let (Some(male), Some(female)) = (find(PID_MALE), find(PID_FEMALE)) else {
        log::warn!("supports: Corrin is not in GameData's person table; supports stay as they are");
        return;
    };
    state.corrin_ids = (male.id, female.id);
    let by_id: HashMap<u16, PersonAt> = people.iter().map(|p| (p.id, p.clone())).collect();
    state.patches.clear();
    state.pairs.clear();
    state.pair_ids.clear();
    let mut given = 0;
    for (corrin, other) in [(&male, &female), (&female, &male)] {
        let Some(&(list, _, count)) = all.iter().find(|&&(_, rid, _)| rid as i16 == corrin.rid) else { continue };
        for k in 0..count as u32 {
            let e = list + 4 + k * 12;
            let partner = r16(m, e);
            // the game gives Corrin an S with the other gender only (and
            // Niles and Rhajat with both); a same-gender pair is filled in.
            // Decided by gender, not by the byte, which may be Haiku's
            // already (a saved state from a run with it)
            let Some(p) = by_id.get(&partner) else { continue };
            if p.female != corrin.female || p.pid == PID_NILES || p.pid == PID_RHAJAT {
                continue;
            }
            let Some(eo) = entry_for(m, &all, other.rid, partner) else { continue };
            let s = r8(m, eo + 7);
            if s == 0xFF {
                continue;
            }
            state.patches.push((e + 7, 0xFF, s));
            if let Some(ep) = entry_for(m, &all, p.rid, corrin.id) {
                state.patches.push((ep + 7, 0xFF, s));
            }
            let from = conversation(m, e) + 3;
            let to = conversation(m, eo) + 3;
            state.pairs.insert(from, to);
            state.pair_ids.insert((corrin.id, partner), (from, to));
            given += 1;
        }
    }
    log::info!("supports: {given} pairs of Corrin's can reach S ({} thresholds)", state.patches.len());
    state.applied = false;
    if S_ANY.load(Ordering::Relaxed) {
        apply(state, m);
    }
}

fn apply(state: &mut State, m: &mut Memory) {
    if state.applied {
        return;
    }
    for &(at, game, ours) in &state.patches {
        if r8(m, at) == game {
            m.write8(at, ours);
        }
    }
    state.applied = !state.patches.is_empty();
}

fn unapply(state: &mut State, m: &mut Memory) {
    if !state.applied {
        return;
    }
    for &(at, game, ours) in &state.patches {
        if r8(m, at) == ours {
            m.write8(at, game);
        }
    }
    state.applied = false;
}

fn talk(state: &mut State, system: &mut System, proc_: u32) -> u32 {
    state.pending = None;
    if trace() {
        let m = &mut system.memory;
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let sp = spouse(m).map(|(p, _)| hex(&p.pid));
        let child = co_child(m).map(|c| hex(&c));
        log::info!("supports: a support conversation; Corrin's spouse {sp:?}, the child Corrin co-parents by a same-gender S {child:?} (Shift JIS PIDs)");
    }
    if !state.applied {
        return 0;
    }
    let m = &mut system.memory;
    let ids: Vec<u16> = [0x34u32, 0x38]
        .iter()
        .map(|&o| {
            let unit = word(m, proc_ + o);
            let person = word(m, unit + 0x9C);
            if person != 0 && m.is_readable(person, 0x28) {
                r16(m, person + 0x24)
            } else {
                0
            }
        })
        .collect();
    let (a, b) = (ids[0], ids[1]);
    let (cm, cf) = state.corrin_ids;
    let key = if a == cm || a == cf {
        (a, b)
    } else if b == cm || b == cf {
        (b, a)
    } else {
        return 0;
    };
    match state.pair_ids.get(&key) {
        Some(&pair) => {
            state.pending = Some(pair);
            1
        }
        None => 0,
    }
}

/// a label of Haiku's for one the game looks up
fn ident_hook(state: &mut State, m: &mut Memory, hash: u32, label_at: u32) -> u32 {
    if hash != word(m, crate::region::us(MESS_HASH)) {
        return 0;
    }
    let label = cstr(m, label_at, 128);
    if trace() {
        let node = ident(m, hash, &label);
        log::info!("supports: lookup {} -> {node:#x}", String::from_utf8_lossy(&label));
        // the same label at rank C (full-width S -> C in Shift JIS: 82 72 -> 82 62)
        if let Some(i) = label.windows(2).position(|w| w == [0x82, 0x72]) {
            let mut c = label.clone();
            c[i + 1] = 0x62;
            let node = ident(m, hash, &c);
            log::info!("supports: lookup (rank C) {} -> {node:#x}", String::from_utf8_lossy(&c));
        }
    }
    // jpn: the Japanese game looks labels up in any of Corrin's three talk
    // tones (_PCM1.._PCM3, _PCF1.._PCF3); Haiku's Japanese layer has the
    // _HK variants of all three (dev/eur/text/make_text.py, x002)
    let ends_tone = label.len() >= 5 && {
        let t = &label[label.len() - 5..];
        (t.starts_with(b"_PCM") || t.starts_with(b"_PCF")) && (t[4] == b'1' || (crate::region::is_jpn() && (t[4] == b'2' || t[4] == b'3')))
    };
    // Kana's paralogue, for a Kana of Corrin's gender
    if label.starts_with(b"MID_X002_") && ends_tone {
        if kana_flipped(state, m) == Some(true) {
            let mut alt = label.clone();
            alt.extend_from_slice(b"_HK");
            return ident(m, hash, &alt);
        }
        return 0;
    }
    // lines that name a parent or Kana by the game's pairing: Haiku's layer
    // marks them with an empty <label>_HKC / _HKS / _HKK and has the
    // variants beside them (dev/supports/make_text.py), so a label without
    // a marker costs one lookup each
    if !ends_tone {
        let with = |suffix: &[u8]| [label.as_slice(), suffix].concat();
        // a child whose other parent is Corrin, of the fixed parent's gender
        if ident(m, hash, &with(b"_HKC")) != 0 {
            if let Some(child) = co_child(m) {
                let node = ident(m, hash, &with(&[b"_HKC_".as_slice(), child.get(4..).unwrap_or(&[])].concat()));
                if node != 0 {
                    return node;
                }
            }
        }
        // Kana to the other parent: by the spouse's gender
        if ident(m, hash, &with(b"_HKS")) != 0 {
            if let Some((sp, _)) = spouse(m) {
                let node = ident(m, hash, &with(if sp.female { b"_HKS_F" } else { b"_HKS_M" }));
                if node != 0 {
                    return node;
                }
            }
        }
        // a parent to Kana: by Kana's gender
        if ident(m, hash, &with(b"_HKK")) != 0 {
            if let Some(male) = kana_in_army(m) {
                let node = ident(m, hash, &with(if male { b"_HKK_M" } else { b"_HKK_F" }));
                if node != 0 {
                    return node;
                }
            }
        }
    }
    // lines the game labels without Corrin's talk tone but that name
    // Corrin's gender: a spouse's Live2D lines (for a spouse of Corrin's
    // gender), Kana's lines (for a Kana of Corrin's gender), and the
    // paralogue's own ones. Haiku's layer has <label>_PCM1 / _PCF1 where
    // they differ; Corrin's talk tone picks it
    let kana = label.windows(KANA_LABEL.len()).any(|w| w == KANA_LABEL);
    if !ends_tone && (label.starts_with(b"MKID_LIVE2D_VOICE_") || label.starts_with(b"MID_X002_") || kana) {
        if let Some(female) = corrin_female(m) {
            let mut alt = label.clone();
            alt.extend_from_slice(if female { b"_PCF1" } else { b"_PCM1" });
            return ident(m, hash, &alt);
        }
    }
    0
}

/// whether Kana's gender is Corrin's (not the game's own pairing)
fn kana_flipped(state: &mut State, m: &mut Memory) -> Option<bool> {
    let female = corrin_female(m)?;
    let male_kana = kana_choice(state, m, female)?;
    Some(male_kana != female)
}

/// Kana's gender: the Kana in the army, else the one decided for this run
/// of the paralogue (decided here when the setting allows: not before the
/// question, when it is asked)
fn kana_choice(state: &mut State, m: &mut Memory, corrin_female: bool) -> Option<bool> {
    if let Some(k) = kana_in_army(m) {
        return Some(k);
    }
    if let Some(k) = state.kana {
        return Some(k);
    }
    if chapter(m) != b"CID_X002" {
        return None;
    }
    if KanaGender::from_code(KANA.load(Ordering::Relaxed)) == KanaGender::Ask && !state.asked {
        return None;
    }
    let k = decide(state, m, corrin_female);
    state.kana = Some(k);
    log::info!("supports: Kana's paralogue: Kana is {} (Corrin is {})", if k { "male" } else { "female" }, if corrin_female { "female" } else { "male" });
    if k != corrin_female {
        // a Kana of Corrin's gender: Corrin is the fixed parent
        patch_kana_parent(state, m, corrin_female);
    }
    Some(k)
}

fn is_pid(state: &mut State, m: &mut Memory, unit: u32, pid_at: u32) -> u32 {
    const ORIGINAL: u32 = 2;
    if KanaGender::from_code(KANA.load(Ordering::Relaxed)) == KanaGender::Vanilla {
        return ORIGINAL;
    }
    let pid = cstr(m, pid_at, 32);
    if trace() {
        let c = chapter(m);
        log::info!("supports: UnitIsPID({unit}, {}) in {}", String::from_utf8_lossy(&pid), String::from_utf8_lossy(&c));
    }
    let asks_female = pid == PID_FEMALE;
    if !asks_female && pid != PID_MALE {
        return ORIGINAL;
    }
    if chapter(m) != b"CID_X002" {
        state.kana = None;
        return ORIGINAL;
    }
    // the unit asked about is Corrin
    if unit == 0 || unit > 0xFA {
        return ORIGINAL;
    }
    let pool = word(m, crate::region::us(UNIT_POOL));
    let at = pool + (unit - 1) * UNIT_SIZE;
    let person = word(m, at + 0x9C);
    let name = word(m, person + 8);
    let unit_pid = cstr(m, name, 32);
    let female = if unit_pid == PID_FEMALE {
        true
    } else if unit_pid == PID_MALE {
        false
    } else {
        return ORIGINAL;
    };
    let Some(male_kana) = kana_choice(state, m, female) else { return ORIGINAL };
    if male_kana == female {
        // the game's own pairing (a female Corrin has a son)
        return ORIGINAL;
    }
    // as if Corrin were the parent this Kana has in the game
    let virtual_female = male_kana;
    (asks_female == virtual_female) as u32
}

fn decide(state: &mut State, m: &mut Memory, corrin_female: bool) -> bool {
    let random = || {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        nanos >> 7 & 1 == 1
    };
    match KanaGender::from_code(KANA.load(Ordering::Relaxed)) {
        KanaGender::Vanilla => corrin_female,
        KanaGender::Male => true,
        KanaGender::Female => false,
        KanaGender::Random => random(),
        KanaGender::Ask => {
            if !state.asked {
                return corrin_female;
            }
            state.asked = false;
            let gud = word(m, crate::region::us(GAME_USER_DATA));
            let vm = word(m, gud + 0x20);
            let values = word(m, vm + 0xC);
            let count = word(m, vm + 8);
            let choice = if values != 0 && count > EVENT_CHOICE { word(m, values + EVENT_CHOICE * 4) } else { u32::MAX };
            log::info!("supports: Kana's gender was asked: choice {choice}");
            match choice {
                0 => true,
                1 => false,
                2 => random(),
                _ => corrin_female,
            }
        }
    }
}

/// Kana's fixed parent (Person +0x2A) becomes the Corrin playing, so the
/// child's family (Unit::CreateImpl) has Corrin and Corrin's spouse.
fn patch_kana_parent(state: &mut State, m: &mut Memory, corrin_female: bool) {
    if state.kana_parent_patched {
        return;
    }
    let people = persons(m);
    let corrin = if corrin_female { PID_FEMALE } else { PID_MALE };
    let Some(id) = people.iter().find(|p| p.pid == corrin).map(|p| p.id) else { return };
    for kana in [PID_KANA_M, PID_KANA_F] {
        if let Some(p) = people.iter().find(|p| p.pid == kana) {
            m.write16(p.at + 0x2A, id);
        }
    }
    state.kana_parent_patched = true;
    log::info!("supports: Kana's parent is Corrin ({id:#x})");
}

const ASK_LABELS: [&str; 4] = ["MID_HAIKU_KANA_ASK", "MID_HAIKU_KANA_MALE", "MID_HAIKU_KANA_FEMALE", "MID_HAIKU_KANA_RANDOM"];

/// ev::CutScene's function (BeginScene) and ev::Talk
const CUT_SCENE: u32 = 0x0020_97B8;
const EV_TALK: u32 = 0x003A_F36C;

/// a native the script calls: hold back Kana's first line for the question
fn call_c(state: &mut State, system: &mut System, function: u32, first: u32) -> u32 {
    let m = &mut system.memory;
    let function = crate::region::to_us(function); // eur: the USA one, to compare
    if function != CUT_SCENE && function != EV_TALK {
        return 0;
    }
    if chapter(m) != b"CID_X002" {
        // another chapter: the next run of the paralogue decides again
        state.kana = None;
        state.asked = false;
        state.kana_parent_patched = false;
        return 0;
    }
    if function != EV_TALK || cstr(m, first, 20) != b"MID_X002_OP1_1" || state.asked || state.kana.is_some() {
        return 0;
    }
    if KanaGender::from_code(KANA.load(Ordering::Relaxed)) != KanaGender::Ask || kana_in_army(m).is_some() {
        return 0;
    }
    let hash = word(m, crate::region::us(MESS_HASH));
    if ident(m, hash, ASK_LABELS[0].as_bytes()) == 0 {
        log::warn!("supports: the Kana question's text is not loaded; Kana is the game's own");
        return 0;
    }
    if !map_page(state, system) {
        return 0;
    }
    state.asked = true;
    log::info!("supports: Kana's paralogue: asking Kana's gender");
    1
}

fn ask_text(state: &mut State, system: &mut System, index: u32) -> u32 {
    if !map_page(state, system) {
        return 0;
    }
    let at = state.page + index.min(3) * 0x40;
    let label = ASK_LABELS[index.min(3) as usize].as_bytes();
    system.memory.write_bytes(at, label);
    system.memory.write8(at + label.len() as u32, 0);
    at
}

fn map_page(state: &mut State, system: &mut System) -> bool {
    if state.page != 0 && system.memory.mapping_at(state.page).is_some() {
        return true;
    }
    state.page = 0;
    let Some(&vaddr) = PAGE_ADDRESSES.iter().find(|&&a| system.memory.mapping_at(a).is_none()) else {
        log::error!("supports: no free guest addresses for the Kana question");
        return false;
    };
    let block = [MemoryRegion::Base, MemoryRegion::System, MemoryRegion::Application]
        .into_iter()
        .find_map(|region| system.memory.phys.allocate_top(region, PAGE_SIZE));
    let Some(block) = block else {
        log::error!("supports: no memory for the Kana question");
        return false;
    };
    system.memory.map(vaddr, block.addr, PAGE_SIZE, Permission::READ | Permission::WRITE, MemoryState::Private);
    system.memory.write_bytes(vaddr, &[0u8; PAGE_SIZE as usize]);
    state.page = vaddr;
    true
}
