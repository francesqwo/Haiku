//! randomizer: the plan's rules from round 3 on (version 2), for new runs.
//! Runs saved with version 1 keep plan.rs's make_v1.
//!
//! What changed, from the user's playtest (2026-11-12):
//! - **One pool.** Children are in the same pool as everyone else: a child
//!   can appear in an adult's place and an adult in a child's. Their data
//!   is valid either way: personal bases are relative to the class and are
//!   brought to the slot's level with the shown character's growths (a
//!   child's GameData level is 10, the adults' their join level), and a
//!   child's place keeps its parent, paralogue and recruiting rules.
//! - **Classes change.** A class never stays the slot's own when another
//!   fits. Mounts are a preference, not a rule (riders could only become
//!   the one or two classes of their mount, so Hinoka stayed a Sky Knight
//!   and Elise a Troubadour); fliers still prefer fliers. Gunter's place
//!   keeps a horse: Chapter 3's ending throws him off it.
//! - **Tier.** A slot's tier is its own class's: promoted characters
//!   (Ryoma's Swordmaster in the prologue, Jakob's Butler, the late
//!   joiners) get a promoted class, everyone else a base class, and a
//!   child's promoted class is the one it is promoted to when it joins
//!   late.
//! - **Map sprites.** A class is only given to someone whose map sprite has
//!   a head in it (see sprites.rs): no headless Kotaro on a horse.
//! - **Corrin** gets a random class too: an on-foot class that wields a
//!   sword (Corrin's first weapon and Yato are swords).
//!
//! Version 3 (lilith), from the user's next playtest (2026-11-15):
//! - **Everyone in the pool** (the catalog's SINCE_ALL characters): Lilith,
//!   Kana, the amiibo heroes, the named captured bosses and more story
//!   characters. Only when characters are randomized: a classes-only run
//!   leaves them as the game has them.
//! - **Classes of any kind**: no healer rule and no preference for the
//!   place's own kind of mount (Gunter keeps his horse).
//! - **Corrin** any base class that can fight (weapons.rs hands Corrin a
//!   weapon of it).
//! - **Kana** is one character of the seed's gender. The game itself makes
//!   Kana the avatar's other gender, with two persons (PID_カンナ男 and
//!   PID_カンナ女), of which a game uses one; it never asks. The other Kana
//!   is not in the pool, and both of Kana's places show whoever the
//!   permutation put in the seed's Kana's place, so Kana's place looks the
//!   same whatever Corrin is.
//! - **Stats** of a captured boss shown elsewhere: their GameData entry has
//!   no personal bases (a captured unit takes the boss's own), so they count
//!   from level 1 with their growths.

use std::collections::HashMap;

use super::catalog::{key_is, Catalog, SINCE_ALL};
use super::plan::{eff_level, for_gender, is_standard, permute, standard_base, Classes, GlobalPatch, Plan, Rng, CHARACTERS, CHILDREN, CLASSES, CORRIN, STORY, VERSION_CURRENT};

/// the slots whose cutscenes need their mount
const MUST_MOUNT: &[&str] = &["ギュンター"];
/// each route's early healers keep staves
const HEALER_SLOTS: &[&str] = &["サクラ", "エリーゼ", "フェリシア", "ジョーカー"];

/// fliers are one group: a wyvern or a pegasus (or kinshi)
fn ride_group(ride: u8) -> u8 {
    if ride == 3 { 5 } else { ride }
}

/// the first non-empty of the candidates narrowed by keep, else all of them
fn prefer(all: Vec<u16>, keep: impl Fn(u16) -> bool) -> Vec<u16> {
    let kept: Vec<u16> = all.iter().copied().filter(|&c| keep(c)).collect();
    if kept.is_empty() { all } else { kept }
}

/// lilith: Kana's two places (male, female), when the catalog has them
pub fn kana_places(cat: &Catalog) -> Option<[usize; 2]> {
    let find = |k: &str| cat.identities.iter().position(|i| i.key_text == k && i.since >= SINCE_ALL);
    Some([find("カンナ男")?, find("カンナ女")?])
}

/// lilith: the gender of a run's Kana (true: female), from the seed
pub fn kana_female(seed: u32) -> bool {
    Rng::new(seed, 6).below(2) == 1
}

pub fn make(cat: &Catalog, seed: u32, options: u32, version: u32) -> Plan {
    let idents = &cat.identities;
    let mut shown: HashMap<usize, usize> = HashMap::new();
    // lilith: the characters version 3 adds only take part when characters
    // are randomized; of the two Kanas, the seed's
    let newer = |i: usize| idents[i].since >= SINCE_ALL;
    // lilith: version 3 lets a place's class be any kind (the user saw
    // Sakura and Elise always healers, Camilla and Hinoka always fliers,
    // Leo always mounted): no healer rule and no preference for the place's
    // own kind of mount. Gunter keeps his horse (Chapter 3's fall). No early
    // place (Prologue to Chapter 6, every route) is placed where only a
    // flier can stand, so none has to fly.
    let v3 = version >= SINCE_ALL;
    let kana = kana_places(cat).filter(|_| version >= SINCE_ALL && options & CHARACTERS != 0);
    let (kana_kept, kana_other) = match kana {
        Some([m, f]) => {
            if kana_female(seed) {
                (Some(f), Some(m))
            } else {
                (Some(m), Some(f))
            }
        }
        None => (None, None),
    };
    let pool: Vec<usize> = (0..idents.len())
        .filter(|&i| (!idents[i].child || options & CHILDREN != 0) && (!idents[i].story || options & STORY != 0))
        .filter(|&i| !newer(i) || options & CHARACTERS != 0)
        .filter(|&i| Some(i) != kana_other)
        .collect();
    if options & CHARACTERS != 0 {
        let mut rng = Rng::new(seed, 1);
        for (slot, who) in permute(&mut rng, &pool) {
            shown.insert(slot, who);
        }
        // lilith: the other Kana's place shows the same one
        if let (Some(k), Some(o)) = (kana_kept, kana_other) {
            if let Some(&w) = shown.get(&k) {
                shown.insert(o, w);
            }
        }
    }
    let job_fits = |id: u16, who: usize| cat.job(id).is_some_and(|j| cat.sprites.fits(&j.key, &idents[who].aid_key));
    let horse_fits = |who: usize| cat.jobs.iter().any(|j| j.ride == 4 && j.female == idents[who].female && is_standard(cat, j.id) && job_fits(j.id, who));
    // a slot that needs its mount is shown as someone whose sprite can ride
    for slot in 0..idents.len() {
        if !MUST_MOUNT.iter().any(|k| key_is(&idents[slot].key, k)) {
            continue;
        }
        let who = *shown.get(&slot).unwrap_or(&slot);
        if horse_fits(who) {
            continue;
        }
        if let Some(other) = pool.iter().copied().find(|&o| o != slot && !MUST_MOUNT.iter().any(|k| key_is(&idents[o].key, k)) && shown.get(&o).is_some_and(|&w| horse_fits(w) && w != slot)) {
            let w = shown[&other];
            shown.insert(other, who);
            shown.insert(slot, w);
        }
    }
    let who = |slot: usize| *shown.get(&slot).unwrap_or(&slot);

    // corrinswap: Corrin as a character. For each gender of Corrin (the
    // avatar's is only known after the creator), one place is shown as
    // Corrin, and whoever that place showed takes Corrin's place: Corrin
    // goes into the permutation's cycle there, so everyone still appears
    // once. That someone is of Corrin's gender, so the place's class (made
    // for them) suits Corrin and Corrin's suits them. Gunter's place stays
    // (Chapter 3 throws him off his horse).
    let corrin_swap = (options & CORRIN != 0).then(|| {
        let mut rng = Rng::new(seed, 5);
        [false, true].map(|female| {
            let places: Vec<usize> = pool
                .iter()
                .copied()
                .filter(|&s| !MUST_MOUNT.iter().any(|k| key_is(&idents[s].key, k)) && idents[who(s)].female == female)
                // lilith: not Kana's places (the other one shows the same)
                .filter(|&s| kana.map_or(true, |k| !k.contains(&s)))
                .collect();
            let s = places[rng.below(places.len())];
            (s, who(s))
        })
    });

    let base = standard_base(cat);
    let promoted_pool: Vec<u16> = cat.jobs.iter().filter(|j| j.promoted && is_standard(cat, j.id)).map(|j| j.id).collect();
    let mut classes = HashMap::new();
    let mut rng = Rng::new(seed, 3);
    for slot in 0..idents.len() {
        // lilith: version 3's characters keep their classes in a
        // classes-only run (no classes are planned for them at all)
        if newer(slot) && options & CHARACTERS == 0 {
            continue;
        }
        let shown_id = who(slot);
        let female = idents[shown_id].female;
        let gender = |id: u16| cat.job(id).is_some_and(|j| j.female == female);
        let healer = !v3 && HEALER_SLOTS.iter().any(|k| key_is(&idents[slot].key, k));
        let must_mount = MUST_MOUNT.iter().any(|k| key_is(&idents[slot].key, k));
        let own = cat.job(idents[slot].class);
        let ride = own.map_or(0, |j| j.ride);
        let own_ids: Vec<u16> = own.map_or(Vec::new(), |j| vec![j.id, j.other_gender, for_gender(cat, j.id, female)]);
        let staff = |id: u16| !healer || cat.job(id).is_some_and(|j| j.uses(6));
        let same_ride = |id: u16| cat.job(id).is_some_and(|j| j.ride == ride);
        let same_group = |id: u16| cat.job(id).is_some_and(|j| ride_group(j.ride) == ride_group(ride));
        let sprite = |id: u16| job_fits(id, shown_id);
        let hard = |pool: &[u16]| -> Vec<u16> {
            let all: Vec<u16> = pool.iter().copied().filter(|&c| gender(c) && staff(c)).collect();
            if must_mount { prefer(all, same_ride) } else { all }
        };
        // soft rules, the first most important
        let narrow = |all: Vec<u16>| -> Vec<u16> {
            let a = prefer(all, sprite);
            let a = prefer(a, |c| !own_ids.contains(&c));
            if v3 {
                a
            } else {
                prefer(a, same_group)
            }
        };
        let (pick, promoted) = if own.is_some_and(|j| j.promoted) {
            let pool = narrow(hard(&promoted_pool));
            let promoted = pool[rng.below(pool.len())];
            let job = cat.job(promoted).unwrap();
            let from: Vec<u16> = job.from.iter().copied().filter(|&b| b != 0 && gender(b) && base.contains(&b)).collect();
            let from = prefer(from, sprite);
            (from.first().copied().unwrap_or(promoted), promoted)
        } else {
            // a base class whose promotions keep the healer's staff and fit
            // the sprite too, where one does
            let with_promotion = |b: u16| cat.job(b).is_some_and(|j| j.promotions.iter().any(|&p| p != 0 && gender(p) && staff(p) && sprite(p)));
            let pool = narrow(prefer(hard(&base), with_promotion));
            let pick = pool[rng.below(pool.len())];
            let job = cat.job(pick).unwrap();
            let promos: Vec<u16> = job.promotions.iter().copied().filter(|&p| p != 0 && gender(p)).collect();
            let promos = prefer(prefer(promos, staff), sprite);
            let promos = if v3 { promos } else { prefer(promos, same_group) };
            (pick, promos.get(rng.below(promos.len().max(1))).copied().unwrap_or(pick))
        };
        let others = prefer(base.iter().copied().filter(|&b| gender(b) && b != pick).collect(), sprite);
        let r1 = others[rng.below(others.len())];
        let r2 = others[rng.below(others.len())];
        classes.insert(slot, Classes { base: pick, promoted, reclass: [r1, if r2 == r1 { 0 } else { r2 }] });
    }
    // lilith: both of Kana's places have the same classes
    if let (Some(k), Some(o)) = (kana_kept, kana_other) {
        if let Some(&c) = classes.get(&k) {
            classes.insert(o, c);
        }
    }

    // Corrin: an on-foot base class that wields a sword. Corrin starts with
    // a sword the game gives outside any chapter's units (the first test with
    // a Dark Mage could not attack in the Prologue), and Yato is a sword
    let corrin = (options & CLASSES != 0).then(|| {
        let mut rng = Rng::new(seed, 4);
        let sword = |id: u16| cat.job(id).is_some_and(|j| j.uses(0));
        let male: Vec<u16> = if v3 {
            // lilith: any base class that can fight (not a staff alone, not
            // a beast): Corrin is handed a weapon of the class (weapons.rs)
            // and keeps the sword. Its promotion wields a sword where one
            // does, so Yato can be used once promoted.
            base.iter().copied().filter(|&b| cat.job(b).is_some_and(|j| !j.female && j.main_kind().is_some() && !j.staff_only() && !j.uses(7))).collect()
        } else {
            base.iter().copied().filter(|&b| cat.job(b).is_some_and(|j| !j.female && j.ride == 0 && sword(b))).collect()
        };
        let mut pick = male[rng.below(male.len())];
        // tests: FATES_RANDO_CORRIN_CLASS=<class name> picks Corrin's class
        if let Some(name) = std::env::var("FATES_RANDO_CORRIN_CLASS").ok().filter(|_| v3) {
            if let Some(&c) = male.iter().find(|&&c| cat.job(c).is_some_and(|j| j.name.eq_ignore_ascii_case(&name))) {
                pick = c;
            }
        }
        let promos: Vec<u16> = cat.job(pick).map_or(Vec::new(), |j| j.promotions.iter().copied().filter(|&p| p != 0).collect());
        let promos = prefer(promos, sword);
        let promoted = promos.get(rng.below(promos.len().max(1))).copied().unwrap_or(pick);
        [(pick, promoted), (for_gender(cat, pick, true), for_gender(cat, promoted, true))]
    });

    // GameData's playable characters
    let mut globals = Vec::new();
    let mut spoiler = Vec::new();
    for (slot, id) in idents.iter().enumerate() {
        let Some(index) = id.global else { continue };
        // lilith: a place the run leaves alone (version 3's characters in a
        // classes-only run) keeps its GameData entry
        if !classes.contains_key(&slot) {
            continue;
        }
        let s = &cat.persons.iter().find(|p| p.index == index).unwrap().clone();
        let shown_id = who(slot);
        let shown_identity = &idents[shown_id];
        let shown_person = shown_identity.global.and_then(|g| cat.persons.iter().find(|p| p.index == g));
        let female = shown_identity.female;
        let slot_job = cat.job(s.class);
        let promoted = slot_job.is_some_and(|j| j.promoted);
        let c = classes[&slot];
        let (class, class2, reclass) = if options & CLASSES != 0 {
            if promoted {
                (c.promoted, c.base, c.reclass)
            } else if s.class2 != 0 {
                (c.base, c.promoted, c.reclass)
            } else {
                (c.base, 0, c.reclass)
            }
        } else {
            let mut class = for_gender(cat, s.class, female);
            let mut other = if s.class2 != 0 { for_gender(cat, s.class2, female) } else { 0 };
            if cat.job(class).is_some_and(|j| j.female != female) || !job_fits(class, shown_id) {
                // no such class for this gender (Songstress), or no map
                // sprite with a head: the slot's own random one
                class = if promoted { c.promoted } else { c.base };
                if other != 0 {
                    other = if promoted { c.base } else { c.promoted };
                }
            }
            (class, other, [for_gender(cat, s.reclass[0], female), for_gender(cat, s.reclass[1], female)])
        };
        let job = cat.job(class);
        let (bases, growths, modifiers, pskills, birthday) = match shown_person {
            Some(g) if shown_id != slot => {
                // lilith: a captured boss's entry has no personal bases (the
                // unit takes the boss's own): from level 1 with their growths
                let own_level = if g.bases.iter().all(|&b| b == 0) && newer(shown_id) { 1 } else { eff_level(g, cat.job(g.class)) };
                let delta = eff_level(s, slot_job) - own_level;
                let mut bases = [0i8; 8];
                for i in 0..8 {
                    let floor = -(job.map_or(0, |j| j.bases[i]) as i32) + if i == 0 { 12 } else { 0 };
                    let v = g.bases[i] as i32 + (delta * g.growths[i] as i32 + 50).div_euclid(100);
                    bases[i] = v.clamp(floor, 40) as i8;
                }
                (bases, g.growths, g.modifiers, g.pskills, g.birthday)
            }
            _ => (s.bases, s.growths, s.modifiers, s.pskills, s.birthday),
        };
        let best = *s.wexp.iter().max().unwrap_or(&0);
        let mut wexp = [0u8; 8];
        if let Some(job) = job {
            for k in 0..8 {
                if job.wexp[k] > 0 {
                    wexp[k] = best.max(1).min(job.wexp[k]);
                }
            }
        }
        spoiler.push(format!(
            "{:<10} -> {:<10} {}{}",
            id.name,
            shown_identity.name,
            job.map_or(String::new(), |j| j.name.clone()),
            if shown_identity.child != id.child { if shown_identity.child { "  (a child in an adult's place)" } else { "  (an adult in a child's place)" } } else { "" }
        ));
        globals.push(GlobalPatch { index, identity: shown_id, class, class2, bases, growths, modifiers, wexp, pskills, birthday, reclass });
    }
    for (slot, id) in idents.iter().enumerate() {
        if id.global.is_none() && shown.contains_key(&slot) {
            spoiler.push(format!("{:<10} -> {:<10} (story)", id.name, idents[who(slot)].name));
        }
    }
    if let Some([(m, mp), (f, fp)]) = corrin {
        let name = |id: u16| cat.job(id).map_or("?".to_string(), |j| j.name.clone());
        spoiler.push(format!("Corrin     -> Corrin     {} / {} (promotes to {} / {})", name(m), name(f), name(mp), name(fp)));
    }
    if let Some(pairs) = corrin_swap {
        for (g, (s, y)) in pairs.iter().enumerate() {
            let g = if g == 1 { "female" } else { "male" };
            spoiler.push(format!("Corrin ({g}): {} is shown as Corrin, and Corrin as {}", idents[*s].name, idents[*y].name));
        }
    }
    if let Some(k) = kana_kept {
        spoiler.push(format!("Kana is {} in this run (both of Kana's places show the same one)", if idents[k].female { "female" } else { "male" }));
    }
    Plan { seed, options, version: version.clamp(2, VERSION_CURRENT), corrin, corrin_swap, shown, classes, globals, spoiler }
}
