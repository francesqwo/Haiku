//! rando4: children's hair colours. In the game a child's hair takes the
//! colour of their variable parent (the one the fixed parent married):
//! `FaceDataManager::GetHairColorFromPerson` (0x001E0180) finds the fixed
//! parent's unit, its spouse's unit, and answers the spouse's hair colour
//! (`Unit + 0x13C`, RGBA), or the child's own FaceData colour (+0x1C) when
//! there is no such unit. `Unit::Create` (and the castle, clerk and packet
//! units) keep the answer in the unit's +0x13C, which the save keeps
//! (`Unit::Serialize`), and everything that draws the child reads it there:
//! the portrait (`FaceInstance::LoadByUnit`; `LoadByFid` looks for the unit
//! first, then asks the same function), the map sprite
//! (`UnitIcon::SetIcon(const Unit*)`), the critical cut-in, and the 3D
//! model (`BattleUnitResName::PostProcess`, +0x724, for a unit with an edit
//! or a person with a parent).
//!
//! With the HAIR option (every new game that randomizes characters, never
//! classes-only, never a save made before it), each child gets a colour
//! drawn from the seed out of the colours the game itself can give
//! children: the hair colours (FaceData) of every variable parent of every
//! child, read from GameData's support table (an S rank with the fixed
//! parent). Corrin is left out as a parent (the avatar's colour is the
//! player's), and so is Kana, whose hair is always Corrin's. The colour is
//! the child's own, wherever the randomizer shows them: in a child's place
//! or in an adult's.
//!
//! - `GetHairColorFromPerson` answers the child's colour (so a unit made
//!   for them has it, and keeps it in the save);
//! - `BattleUnitResName::PostProcess` gets the colour too when the game
//!   would not ask (a child shown in an adult's place, whose person has no
//!   parent; a cutscene cast without a unit);
//! - when a run with the option starts or a chapter loads, the army's
//!   units of children get it (a test state made before; the same colour a
//!   new unit would get, so a save is never changed by it).

use std::collections::{BTreeMap, HashMap};

use super::catalog::Catalog;
use super::plan::Rng;
use super::{cstr, r16, r32, State};
use crate::System;

/// the bit of haiku_rando_active that says children's hair is randomized
pub const ACTIVE_BIT: u32 = 0x200;

/// UnitPool: the units (0xFA of 0x140 bytes)
const UNIT_POOL: u32 = 0x006D_9F40;
const UNIT_SIZE: u32 = 0x140;
const UNITS: u32 = 0xFA;
const UNIT_FLAGS: u32 = 0x8;
const UNIT_PERSON: u32 = 0x9C;
const UNIT_EDIT: u32 = 0xB0;
const UNIT_HAIR: u32 = 0x13C;
/// Unit::UpdateHairColor leaves a unit with this flag alone
const FLAG_NO_HAIR: u32 = 0x2000_0000;
/// Person: its FID, its parent (person index)
const PERSON_FID: u32 = 0xC;
const PERSON_PARENT: u32 = 0x2A;
/// the random stream of the colours (the plan uses 1 to 5)
const STREAM: u64 = 0x48;

/// identity -> colour (RGBA), for every child; empty without the option
pub type Colours = HashMap<usize, [u8; 4]>;

/// the colours the game can give a child, each with the parents who have
/// it: the FaceData hair colours of every variable parent (every S-rank
/// partner of a child's fixed parent; not Corrin, not a child), for every
/// child but Kana. Sorted, so a seed draws the same on every PC.
pub fn palette(cat: &Catalog) -> Vec<([u8; 4], Vec<String>)> {
    let g = &cat.gamedata;
    // Person::Initialize: the person table's header has the support
    // tables at +8 (a count, then a pointer to each). A table: u16 the
    // owner's support index (Person + 0x30), u16 the count, then 12-byte
    // entries: u16 the partner's id (Person + 0x24), then the points each
    // rank needs (C, B, A, S at +4..+7; 99 for none: Reliance::IsRomance).
    let Some(list) = g.ptr(cat.person_table + 8) else { return Vec::new() };
    let support_index = |offset: u32| g.u16(offset + 0x30) as i16;
    let id = |offset: u32| g.u16(offset + 0x24);
    let mut partners: HashMap<i16, Vec<u16>> = HashMap::new();
    for t in 0..g.u32(list) {
        let Some(table) = g.ptr(list + 4 + 4 * t) else { continue };
        let owner = g.u16(table) as i16;
        for e in 0..g.u16(table + 2) as u32 {
            let at = table + 4 + e * 12;
            if g.u8(at + 7) < 99 {
                partners.entry(owner).or_default().push(g.u16(at));
            }
        }
    }
    let is_child = |p: &super::catalog::PersonRec| p.parent != 0 && p.child_id != 0xFFFF;
    let kana = super::catalog::sjis("PID_カンナ");
    let mut colours: BTreeMap<[u8; 4], Vec<String>> = BTreeMap::new();
    for child in cat.persons.iter().filter(|p| is_child(p) && !p.pid.starts_with(&kana)) {
        let Some(fixed) = cat.persons.iter().find(|p| p.index == child.parent as usize) else { continue };
        let Some(ids) = partners.get(&support_index(fixed.offset)) else { continue };
        for &pid in ids {
            let Some(parent) = cat.persons.iter().find(|p| id(p.offset) == pid) else { continue };
            // Corrin's two persons (the avatar's colour is the player's)
            if parent.index < 3 || is_child(parent) {
                continue;
            }
            let Some(key) = parent.fid.as_ref().and_then(|f| f.strip_prefix(b"FID_".as_slice())) else { continue };
            let Some(&hair) = cat.face_hair.get(key) else { continue };
            let name = cat.identities.iter().find(|i| i.global == Some(parent.index)).map_or_else(|| super::sprites::sjis_text(key), |i| i.name.clone());
            let names = colours.entry(hair).or_default();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    colours.into_iter().collect()
}

/// each child's colour, drawn from the seed
pub fn colours(cat: &Catalog, seed: u32) -> Colours {
    let palette = palette(cat);
    let mut out = Colours::new();
    if palette.is_empty() {
        return out;
    }
    let mut rng = Rng::new(seed, STREAM);
    for (i, id) in cat.identities.iter().enumerate() {
        if id.child {
            out.insert(i, palette[rng.below(palette.len())].0);
        }
    }
    out
}

/// the spoiler's lines: each child's colour, and whose it is
pub fn spoiler(cat: &Catalog, colours: &Colours) -> Vec<String> {
    let owners: HashMap<[u8; 4], Vec<String>> = palette(cat).into_iter().collect();
    let mut lines = vec![format!("children's hair colours ({} colours, the parents'):", owners.len())];
    let mut children: Vec<(&usize, &[u8; 4])> = colours.iter().collect();
    children.sort();
    for (&i, c) in children {
        let whose = owners.get(c).map(|o| o.join(", ")).unwrap_or_default();
        lines.push(format!("  {:<10} #{:02X}{:02X}{:02X} ({whose}'s)", cat.identities[i].name, c[0], c[1], c[2]));
    }
    lines
}

/// the colour of a person (in guest memory), when the option changes it:
/// a child's own colour; an adult the randomizer shows in a child's place
/// has their own FaceData colour (not the slot's parents').
fn person_colour(state: &mut State, system: &mut System, person: u32, model: bool) -> Option<[u8; 4]> {
    let active = state.active.as_ref()?;
    if active.hair.is_empty() || person == 0 || !system.memory.is_readable(person, 0x30) {
        return None;
    }
    let cat = state.catalog.as_ref()?;
    let m = &mut system.memory;
    let fid_at = r32(m, person + PERSON_FID);
    let fid = cstr(m, fid_at, 64);
    let key = fid.strip_prefix(b"FID_".as_slice())?;
    let &who = cat.by_key.get(key)?;
    if let Some(&c) = active.hair.get(&who) {
        return Some(c);
    }
    // an adult in a child's place: the game would give them the slot's
    // other parent's colour; the model only takes the unit's colour there
    if !model && r16(m, person + PERSON_PARENT) != 0 {
        return cat.face_hair.get(key).copied();
    }
    None
}

/// op::HAIR: FaceDataManager::GetHairColorFromPerson(out, person): the
/// colour (RGBA as a little-endian word), or 0 for the game's own answer
pub(super) fn hair(state: &mut State, system: &mut System, person: u32) -> u32 {
    let Some(c) = person_colour(state, system, person, false) else { return 0 };
    note(state, system, person, c, "a unit or portrait");
    u32::from_le_bytes(c)
}

/// op::HAIR_MODEL: after BattleUnitResName::PostProcess(unit, person): a
/// child's model colour (0x80RRGGBB, for +0x724 as 0xRRGGBB), or 0 to keep
/// the game's
pub(super) fn model(state: &mut State, system: &mut System, unit: u32, person: u32) -> u32 {
    let m = &mut system.memory;
    let unit_ok = unit != 0 && m.is_readable(unit, UNIT_SIZE);
    // the avatar (a unit with an edit) has the player's colour
    if unit_ok && r32(m, unit + UNIT_EDIT) != 0 {
        return 0;
    }
    let person = if person != 0 { person } else if unit_ok { r32(m, unit + UNIT_PERSON) } else { 0 };
    let Some(c) = person_colour(state, system, person, true) else { return 0 };
    note(state, system, person, c, "a model");
    0x8000_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// the army's units of children take their colour (see the module's notes)
pub(super) fn refresh_units(state: &mut State, system: &mut System) {
    if state.active.as_ref().map_or(true, |a| a.hair.is_empty()) {
        return;
    }
    let units = r32(&mut system.memory, crate::region::us(UNIT_POOL));
    if units == 0 || !system.memory.is_readable(units, UNIT_SIZE * UNITS) {
        return;
    }
    let mut changed = 0;
    let mut seen: Vec<String> = Vec::new();
    for i in 0..UNITS {
        let u = units + i * UNIT_SIZE;
        let m = &mut system.memory;
        let person = r32(m, u + UNIT_PERSON);
        if person == 0 || r32(m, u + UNIT_EDIT) != 0 || r32(m, u + UNIT_FLAGS) & FLAG_NO_HAIR != 0 {
            continue;
        }
        let Some(c) = person_colour(state, system, person, false) else { continue };
        let m = &mut system.memory;
        let mut old = [0u8; 4];
        m.read_bytes(u + UNIT_HAIR, &mut old);
        let fid_at = r32(m, person + PERSON_FID);
        let fid = String::from_utf8_lossy(&cstr(m, fid_at, 64)).into_owned();
        seen.push(format!("{fid} #{:02X}{:02X}{:02X}{}", old[0], old[1], old[2], if old != c { " (given its colour)" } else { "" }));
        if old != c {
            m.write_bytes(u + UNIT_HAIR, &c);
            changed += 1;
        }
    }
    if changed > 0 {
        log::info!("randomizer: {changed} units of children given their hair colour");
    }
    if !seen.is_empty() {
        log::debug!("randomizer: the army's units with a child's look: {}", seen.join(", "));
    }
}

/// a child's colour for a copy of a unit (corrin.rs: a child shown in
/// Corrin's place), when the option is on
pub(super) fn identity_colour(state: &State, who: usize) -> Option<[u8; 4]> {
    state.active.as_ref()?.hair.get(&who).copied()
}

fn note(state: &mut State, system: &mut System, person: u32, c: [u8; 4], what: &str) {
    let m = &mut system.memory;
    let fid_at = r32(m, person + PERSON_FID);
    let fid = cstr(m, fid_at, 64);
    let key = format!("hair {} {what}", String::from_utf8_lossy(&fid));
    if state.corrin_logged.len() < 400 && state.corrin_logged.insert(key) {
        let name = fid
            .strip_prefix(b"FID_".as_slice())
            .and_then(|k| state.catalog.as_ref().and_then(|cat| cat.by_key.get(k).map(|&i| cat.identities[i].name.clone())))
            .unwrap_or_default();
        log::info!("randomizer: hair colour of {name}, {what}: #{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    }
}
