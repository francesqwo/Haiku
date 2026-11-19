//! lilith: every randomized unit of the army can fight with its new class.
//!
//! A chapter's units get weapons their class can use when they are placed
//! (mod.rs, `fit_weapons` on the dispos entry). Corrin's first weapons are
//! not placed that way: the Prologue and Chapter 1 give Corrin a Bronze
//! Sword, Chapters 2 and 3 Ganglari, outside the dispos hooks. With rules
//! version 3 Corrin's class need not use swords, so the army's units are
//! looked over at the hooks that come with a chapter (its person file, its
//! units being placed, their items, their map sprites) and a unit of the run
//! that holds no weapon its class can use is handed one: the plain weapon of
//! its class's main kind at the rank of the first weapon it holds. It goes
//! into a free slot, so nothing the unit holds is taken away (Yato and the
//! story's swords stay).
//!
//! Layouts (the game's code): Unit (0x140): +0x9C person, +0xA0 class (Job),
//! +0x108 items, 5 of `unit::Item` (u16 item id, u16 uses and flags: zero
//! for a weapon, `unit::Item::New`, 0x00419820).

use super::{r16, r32, w16, State};
use crate::System;

/// UnitPool: the units (0xFA of 0x140 bytes), as hair.rs
const UNIT_POOL: u32 = 0x006D_9F40;
const UNIT_SIZE: u32 = 0x140;
const UNITS: u32 = 0xFA;
const UNIT_PERSON: u32 = 0x9C;
const UNIT_JOB: u32 = 0xA0;
const UNIT_ITEMS: u32 = 0x108;
const JOB_ID: u32 = 0x18;

/// a unit of the run (Corrin, or a person the randomizer changed) that has
/// no weapon its class can use gets one; `why` names the hook, for the log
pub(super) fn ensure(state: &mut State, system: &mut System, why: &str) {
    let Some(cat) = state.catalog.clone() else { return };
    let Some(active) = state.active.as_ref() else { return };
    // only runs with rules version 3 (their classes may be any kind); a run
    // made before is left exactly as it was
    if active.plan.version < super::catalog::SINCE_ALL || active.plan.options & super::plan::CLASSES == 0 {
        return;
    }
    let units = r32(&mut system.memory, crate::region::us(UNIT_POOL));
    if units == 0 || !system.memory.is_readable(units, UNIT_SIZE * UNITS) {
        return;
    }
    let corrin: Vec<u32> = active.corrin_at.iter().map(|&(at, _)| at).collect();
    let mut given = Vec::new();
    for i in 0..UNITS {
        let u = units + i * UNIT_SIZE;
        let m = &mut system.memory;
        let person = r32(m, u + UNIT_PERSON);
        let job_at = r32(m, u + UNIT_JOB);
        if person == 0 || job_at == 0 || !m.is_readable(job_at, 0x20) {
            continue;
        }
        if !corrin.contains(&person) && !active.pool.contains_key(&person) {
            continue;
        }
        let Some(job) = cat.job(r16(m, job_at + JOB_ID)) else { continue };
        if job.main_kind().is_none() || job.staff_only() {
            continue;
        }
        let mut free = None;
        let mut first_weapon = None;
        let mut usable = false;
        for k in 0..5 {
            let id = r16(m, u + UNIT_ITEMS + 4 * k);
            if id == 0 {
                free = free.or(Some(k));
                continue;
            }
            let Some(item) = cat.items.iter().find(|it| it.id == id) else { continue };
            if let Some(kind) = item.kind() {
                if job.uses(kind) {
                    usable = true;
                }
                first_weapon = first_weapon.or(Some(item));
            }
        }
        // a unit with no weapon at all is left as the game made it (a unit
        // that is given its weapons later, or none on purpose)
        let (false, Some(slot), Some(held)) = (usable, free, first_weapon) else { continue };
        let Some(new) = super::replacement_weapon(&cat, job, held) else { continue };
        let new = &cat.items[new];
        w16(m, u + UNIT_ITEMS + 4 * slot, new.id);
        w16(m, u + UNIT_ITEMS + 4 * slot + 2, 0);
        let who = if corrin.contains(&person) {
            "Corrin".to_string()
        } else {
            active.pool.get(&person).map_or_else(String::new, |&s| cat.identities[active.plan.shown(s)].name.clone())
        };
        given.push(format!("{who} ({}) {} besides {}", job.name, new.name, held.name));
    }
    if !given.is_empty() {
        log::info!("randomizer: weapons handed out ({why}): {}", given.join("; "));
    }
}
