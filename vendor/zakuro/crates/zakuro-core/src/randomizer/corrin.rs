//! corrinswap: Corrin as a character. With the CORRIN option, the player's
//! own Corrin appears in another character's place in the story, and the
//! character that place would have shown appears in Corrin's.
//!
//! The plan (plan2.rs) names, for each gender of Corrin, a place S and the
//! one Y it showed; Y is of Corrin's gender. Once the avatar's gender is
//! known (after the creator) the pair for it applies:
//!
//! - **S is shown as Corrin**: the avatar's own look (the creator's face,
//!   hair, colour, accessory, body and voice-less talk portrait), the
//!   player's name, in S's class.
//! - **Corrin's place is shown as Y**: Y's portrait, name, model and map
//!   sprite, in Corrin's class.
//!
//! The game draws the avatar from the unit's own edit (`Unit + 0xB0`, a
//! `unit::Edit`) and its person `PID_プレイヤー男/女`, and every other
//! character from its person's FID and AID. So each place that draws a
//! character from a unit (its name, face, map sprite, battle and cutscene
//! models, critical cut-in) is handed a copy of the unit for the duration
//! of the call: S's unit with the avatar's person, edit and hair colour, or
//! the avatar's unit with Y's person, no edit and Y's hair colour. The
//! units themselves never change, so nothing about a unit's stats, saves or
//! story logic does. The places that draw from a person and an edit (map
//! sprites in menus, support lists, castle cards) get the same swap in their
//! arguments. Talk lines swap the window's FID (`FID_username` both ways)
//! and the names said (S's name becomes `$Nu`, the player's name; `$Nu`
//! becomes Y's name). Gendered words need nothing: Y and Corrin are of the
//! same gender.

use super::catalog::Catalog;
use super::{bytes_at, cstr, r16, r32, r8, stand_in, w32, State, POOL_SIZE};
use crate::memory::Memory;
use crate::System;
use recomp_abi::Context;

/// UnitPool: the units (0xFA of 0x140 bytes)
const UNIT_POOL: u32 = 0x006D_9F40;
const UNIT_SIZE: u32 = 0x140;
const UNITS: u32 = 0xFA;
/// Unit: its person, class, edit and hair colour
const UNIT_PERSON: u32 = 0x9C;
const UNIT_JOB: u32 = 0xA0;
const UNIT_EDIT: u32 = 0xB0;
const UNIT_HAIR: u32 = 0x13C;
/// unit::Edit: the gender (1 female); the name is at +0
const EDIT_GENDER: u32 = 0x20;
/// "PID_プレイヤー" in Shift JIS: the avatar's persons (male, female)
const PLAYER_PID: &[u8] = b"PID_\x83\x76\x83\x8C\x83\x43\x83\x84\x81\x5B";
/// how many unit copies are kept (round robin; each is read during one call)
const COPIES: usize = 24;

/// the bit of haiku_rando_active that says Corrin is swapped
pub const ACTIVE_BIT: u32 = 0x100;

#[derive(Default)]
pub(super) struct Copies {
    /// (key, guest address) of each copy buffer
    slots: Vec<(u32, u32)>,
    next: usize,
}

impl Copies {
    pub fn clear(&mut self) {
        self.slots.clear();
        self.next = 0;
    }
}

fn in_pool_page(state: &State, a: u32) -> bool {
    state.pool_at != 0 && a >= state.pool_at && a < state.pool_at + POOL_SIZE
}

fn is_avatar(m: &mut Memory, u: u32) -> bool {
    if u == 0 || !m.is_readable(u, UNIT_SIZE) {
        return false;
    }
    let edit = r32(m, u + UNIT_EDIT);
    let person = r32(m, u + UNIT_PERSON);
    if edit == 0 || person == 0 || !m.is_readable(person, 0x10) {
        return false;
    }
    let pid = r32(m, person + 8);
    cstr(m, pid, 32).starts_with(PLAYER_PID)
}

/// the avatar's unit (the army's, in UnitPool), found once and checked after
fn avatar_unit(state: &mut State, system: &mut System) -> Option<u32> {
    let m = &mut system.memory;
    if state.avatar_at != 0 && is_avatar(m, state.avatar_at) {
        return Some(state.avatar_at);
    }
    state.avatar_at = 0;
    let units = r32(m, crate::region::us(UNIT_POOL));
    if units == 0 || !m.is_readable(units, UNIT_SIZE) {
        return None;
    }
    for i in 0..UNITS {
        let u = units + i * UNIT_SIZE;
        if is_avatar(m, u) {
            state.avatar_at = u;
            return Some(u);
        }
    }
    None
}

/// the swap that applies now: (S, Y, the avatar's unit, the avatar is
/// female). None without the option, without an avatar yet, or while the
/// avatar's creator is open (its preview is the avatar as edited).
pub(super) fn pair(state: &mut State, system: &mut System) -> Option<(usize, usize, u32, bool)> {
    if state.creator {
        return None;
    }
    state.active.as_ref()?.plan.corrin_swap?;
    let avatar = avatar_unit(state, system)?;
    let m = &mut system.memory;
    let edit = r32(m, avatar + UNIT_EDIT);
    let female = r8(m, edit + EDIT_GENDER) == 1;
    let (s, y) = state.active.as_ref()?.plan.corrin_pair(female)?;
    if state.corrin_logged.insert(format!("pair {s} {y} {female}")) {
        let cat = state.catalog.clone()?;
        log::info!(
            "randomizer: Corrin ({}) swapped: {} is shown as Corrin, Corrin as {}",
            if female { "female" } else { "male" },
            cat.identities[s].name,
            cat.identities[y].name
        );
    }
    Some((s, y, avatar, female))
}

/// a copy of a unit, in the randomizer's page, for one call
fn unit_copy(state: &mut State, system: &mut System, key: u32, from: u32) -> Option<u32> {
    let at = if let Some(&(_, at)) = state.copies.slots.iter().find(|(k, _)| *k == key) {
        at
    } else if state.copies.slots.len() < COPIES {
        let at = bytes_at(state, system, &[0u8; UNIT_SIZE as usize])?;
        state.copies.slots.push((key, at));
        at
    } else {
        let i = state.copies.next % COPIES;
        state.copies.next += 1;
        state.copies.slots[i].0 = key;
        state.copies.slots[i].1
    };
    let mut bytes = vec![0u8; UNIT_SIZE as usize];
    system.memory.read_bytes(from, &mut bytes);
    system.memory.write_bytes(at, &bytes);
    Some(at)
}

/// Y's look as a person: Y's own GameData entry as the game has it (or a
/// story character's stand-in), in the class given
fn y_person(state: &mut State, system: &mut System, cat: &Catalog, y: usize, job: u32) -> Option<u32> {
    let base = state.active.as_ref()?.base;
    let class = if job != 0 && system.memory.is_readable(job, 0x20) { r16(&mut system.memory, job + 0x18) } else { cat.identities[y].class };
    stand_in(state, system, cat, base, y, class)
}

fn y_hair(state: &State, cat: &Catalog, y: usize) -> [u8; 4] {
    // rando4: a child shown in Corrin's place has their random colour
    if let Some(c) = super::hair::identity_colour(state, y) {
        return c;
    }
    cat.face_hair.get(&cat.identities[y].key).copied().unwrap_or([0x80, 0x80, 0x80, 0xFF])
}

fn note(state: &mut State, what: String) {
    if state.corrin_logged.len() < 400 && state.corrin_logged.insert(what.clone()) {
        log::debug!("randomizer: {what}");
    }
}

/// op::UNIT_LOOK: a function that draws a character from a unit (its name,
/// face, map sprite, critical cut-in, model) is given this copy instead, or
/// 0 for the unit itself.
pub(super) fn unit_look(state: &mut State, system: &mut System, unit: u32, why: &str) -> u32 {
    if unit == 0 || in_pool_page(state, unit) {
        return 0;
    }
    let Some(cat) = state.catalog.clone() else { return 0 };
    let Some((s, y, avatar, _)) = pair(state, system) else { return 0 };
    let m = &mut system.memory;
    if unit == avatar || is_avatar(m, unit) {
        // Corrin's place, shown as Y
        let job = r32(m, unit + UNIT_JOB);
        let Some(person) = y_person(state, system, &cat, y, job) else { return 0 };
        let Some(copy) = unit_copy(state, system, unit, unit) else { return 0 };
        let m = &mut system.memory;
        w32(m, copy + UNIT_PERSON, person);
        w32(m, copy + UNIT_EDIT, 0);
        let hair = y_hair(state, &cat, y);
        let m = &mut system.memory;
        m.write_bytes(copy + UNIT_HAIR, &hair);
        note(state, format!("{why}: Corrin's unit drawn as {}", cat.identities[y].name));
        return copy;
    }
    if !m.is_readable(unit, UNIT_SIZE) {
        return 0;
    }
    let person = r32(m, unit + UNIT_PERSON);
    if state.active.as_ref().and_then(|a| a.pool.get(&person)) != Some(&s) {
        return 0;
    }
    // S's place, shown as Corrin
    let Some(copy) = unit_copy(state, system, unit, unit) else { return 0 };
    let m = &mut system.memory;
    let (ap, ae) = (r32(m, avatar + UNIT_PERSON), r32(m, avatar + UNIT_EDIT));
    let mut hair = [0u8; 4];
    m.read_bytes(avatar + UNIT_HAIR, &mut hair);
    w32(m, copy + UNIT_PERSON, ap);
    w32(m, copy + UNIT_EDIT, ae);
    m.write_bytes(copy + UNIT_HAIR, &hair);
    note(state, format!("{why}: {}'s unit drawn as Corrin", cat.identities[s].name));
    copy
}

/// op::ICON_LOOK: UnitIcon::SetIcon(job, person, force, edit, bool) from a
/// person and an edit (menus, support lists, castle): r2 the person, the
/// edit on the stack.
pub(super) fn icon_look(state: &mut State, system: &mut System, ctx: &mut Context) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some((s, y, avatar, _)) = pair(state, system) else { return };
    let (job, person, sp) = (ctx.r[1], ctx.r[2], ctx.r[13]);
    let m = &mut system.memory;
    let edit = r32(m, sp);
    let avatar_edit = r32(m, avatar + UNIT_EDIT);
    if person != 0 && edit != 0 && edit == avatar_edit {
        let Some(p) = y_person(state, system, &cat, y, job) else { return };
        ctx.r[2] = p;
        w32(&mut system.memory, sp, 0);
        note(state, format!("map sprite from a person: Corrin as {}", cat.identities[y].name));
    } else if person != 0 && state.active.as_ref().and_then(|a| a.pool.get(&person)) == Some(&s) {
        ctx.r[2] = r32(m, avatar + UNIT_PERSON);
        w32(m, sp, avatar_edit);
        note(state, format!("map sprite from a person: {} as Corrin", cat.identities[s].name));
    }
}

/// op::FACE_EDIT: FaceInstance::LoadByUnitEdit for the avatar's edit
/// (castle cards, support lists): Y's FID to load instead, or 0
pub(super) fn face_edit(state: &mut State, system: &mut System, edit: u32) -> u32 {
    let Some(cat) = state.catalog.clone() else { return 0 };
    let Some((_, y, avatar, _)) = pair(state, system) else { return 0 };
    if edit == 0 || edit != r32(&mut system.memory, avatar + UNIT_EDIT) {
        return 0;
    }
    note(state, format!("a portrait from Corrin's edit: {}", cat.identities[y].name));
    super::string_at(state, system, &cat.identities[y].fid).unwrap_or(0)
}

/// op::MESS_NAME: FaceData::GetMessName for MPID_username (the avatar's
/// portraits: a talk window of the place shown as Corrin) is the player's
/// name, the avatar's own (its edit), whatever Corrin's unit is shown as
pub(super) fn mess_name(state: &mut State, system: &mut System, face: u32) -> u32 {
    let m = &mut system.memory;
    if face == 0 || !m.is_readable(face, 8) {
        return 0;
    }
    let mpid = r32(m, face + 4);
    if cstr(m, mpid, 32) != b"MPID_username" {
        return 0;
    }
    let Some(avatar) = avatar_unit(state, system) else { return 0 };
    r32(&mut system.memory, avatar + UNIT_EDIT)
}

/// op::PERSON_NAME: Person::GetName (r0 the person). Answers 1 with r0 the
/// name (S, shown as Corrin: the player's), 2 with r0 another person to ask
/// (Corrin's person: Y's), else 0.
pub(super) fn person_name(state: &mut State, system: &mut System, ctx: &mut Context) -> u32 {
    let Some(cat) = state.catalog.clone() else { return 0 };
    let person = ctx.r[0];
    if person == 0 || in_pool_page(state, person) {
        return 0;
    }
    let Some((s, y, avatar, _)) = pair(state, system) else { return 0 };
    let m = &mut system.memory;
    if person == r32(m, avatar + UNIT_PERSON) {
        let job = r32(m, avatar + UNIT_JOB);
        let Some(p) = y_person(state, system, &cat, y, job) else { return 0 };
        ctx.r[0] = p;
        return 2;
    }
    if state.active.as_ref().and_then(|a| a.pool.get(&person)) == Some(&s) {
        ctx.r[0] = r32(m, avatar + UNIT_EDIT);
        return 1;
    }
    0
}

/// op::ALIAS: the avatar's unit as it is (a copy, which is never swapped),
/// for a talk window of the place shown as Corrin (FID_username)
pub(super) fn alias(state: &mut State, system: &mut System) -> u32 {
    if pair(state, system).is_none() {
        return 0;
    }
    let Some(avatar) = avatar_unit(state, system) else { return 0 };
    unit_copy(state, system, avatar | 1, avatar).unwrap_or(0)
}

/// a cutscene cast of the place shown as Corrin with no unit of its own
/// (a person or a name only): the avatar's unit, in the class given
pub(super) fn cast_as_corrin(state: &mut State, system: &mut System, ctx: &mut Context, slot: usize, class: u16) -> bool {
    let Some(cat) = state.catalog.clone() else { return false };
    let Some((s, _, avatar, _)) = pair(state, system) else { return false };
    if slot != s {
        return false;
    }
    let Some(base) = state.active.as_ref().map(|a| a.base) else { return false };
    let Some(copy) = unit_copy(state, system, avatar | 2, avatar) else { return false };
    if let Some(job) = cat.job(class) {
        w32(&mut system.memory, copy + UNIT_JOB, base.wrapping_add(job.offset));
    }
    ctx.r[1] = copy;
    ctx.r[2] = 0;
    ctx.r[3] = 0;
    note(state, format!("cutscene cast {} shown as Corrin ({})", cat.identities[s].name, cat.job(class).map_or("?", |j| j.name.as_str())));
    true
}

/// a model built from a unit (battles, cutscenes with the army, My Castle):
/// the unit's copy, and the person argument with it
pub(super) fn cast_unit(state: &mut State, system: &mut System, ctx: &mut Context) {
    let unit = ctx.r[1];
    let copy = unit_look(state, system, unit, "a model");
    if copy == 0 {
        return;
    }
    let m = &mut system.memory;
    if ctx.r[2] == r32(m, unit + UNIT_PERSON) {
        ctx.r[2] = r32(m, copy + UNIT_PERSON);
    }
    ctx.r[1] = copy;
}
