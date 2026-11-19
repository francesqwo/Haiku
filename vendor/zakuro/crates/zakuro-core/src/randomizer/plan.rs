//! randomizer: a run's choices, made from its seed and options alone, so a
//! seed gives the same run on every PC: who appears in whose place, and the
//! classes, stats and weapons that go with it.
//!
//! Characters are swapped by identity. Every character (a "slot": the part
//! Sakura plays in the story, with her joins, supports and scenes) keeps its
//! place in the game's data, scripts and saves, and is shown as someone
//! else: their portrait, name, model and voice, with that someone's own
//! growths, skills and personal stats (brought to the slot's level). The
//! swap is a permutation, so nobody appears twice, and it covers every
//! place a character shows up: GameData's character, each chapter's
//! versions of them (an enemy Xander, a guest Hinoka) and the talk scenes.

use std::collections::HashMap;

use super::catalog::{Catalog, JobRec, PersonRec};

/// option bits, as a save keeps them
pub const CHARACTERS: u32 = 1;
pub const CLASSES: u32 = 2;
pub const STORY: u32 = 4;
pub const CHILDREN: u32 = 8;
pub const DEFAULT_OPTIONS: u32 = CHARACTERS | CLASSES | STORY | CHILDREN;
/// rando3: Corrin takes part in the swap too (Corrin's place is shown as
/// someone else, and Corrin's look appears in another place)
pub const CORRIN: u32 = 0x10;
/// rando4: children's hair colours are random (hair.rs). Set on every new
/// game that randomizes characters (not classes-only); a save made before
/// has no such bit and keeps the game's colours. It changes nothing in the
/// plan, so the rules stay v2.
pub const HAIR: u32 = 0x20;

/// rando3: the plan's rules. A save keeps the version its run was made
/// with, so a run started before round 3 is planned as it always was.
/// - 1: children swap among children; classes keep their mount; a class
///   is kept if it fits the gender.
/// - 2: one pool for everyone, children included; classes never stay the
///   same where another fits, prefer the mount but don't insist (except
///   Gunter's, for his Chapter 3 fall), always have a map sprite with a
///   head; Corrin gets a random class (with a sword, now or once promoted).
/// - 3 (lilith): everyone in the pool: Lilith, Kana (one of the two, of
///   the seed's gender), the amiibo heroes, the named captured bosses and
///   more story characters (catalog::SINCE_ALL); classes of any kind (no
///   healer rule, no preference for the place's own mount; Gunter keeps his
///   horse); Corrin any base class that can fight, handed a weapon of it
///   (weapons.rs). The catalog of a run before 3 leaves the new characters
///   out and the rules above are only 3's, so an older run's plan is the
///   same.
/// - 4: as 3, without Layla (catalog::SINCE_NO_LAYLA): she never fights,
///   so she has no battle or map head of her own.
pub const VERSION_FIRST: u32 = 1;
/// - 5 (jpn2): as 4; on the Japanese Special Edition, Revelation's story
///   characters come from its DLC (catalog::SINCE_DLC_STORY), so its pool
///   is the 98 of the other editions. Elsewhere the same as 4.
pub const VERSION_CURRENT: u32 = 5;

/// splitmix64: small, fast and the same everywhere
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u32, stream: u64) -> Rng {
        Rng((seed as u64) << 32 ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

/// the class a slot gets, for the gender of who appears in it: its base
/// class and the promotion it takes.
#[derive(Clone, Copy, Debug)]
pub struct Classes {
    pub base: u16,
    pub promoted: u16,
    pub reclass: [u16; 2],
}

/// the new values of a playable character's entry in GameData.
#[derive(Clone, Debug)]
pub struct GlobalPatch {
    pub index: usize,
    pub identity: usize,
    pub class: u16,
    pub class2: u16,
    pub bases: [i8; 8],
    pub growths: [u8; 8],
    pub modifiers: [i8; 8],
    pub wexp: [u8; 8],
    pub pskills: [u8; 6],
    pub birthday: [u8; 2],
    pub reclass: [u16; 2],
}

pub struct Plan {
    pub seed: u32,
    pub options: u32,
    /// rando3: the rules it was made with (VERSION_*)
    pub version: u32,
    /// rando3: Corrin's classes (base, promoted) for a male and a female
    /// Corrin, when classes are randomized (version 2)
    pub corrin: Option<[(u16, u16); 2]>,
    /// corrinswap: with CORRIN, for a male and a female Corrin: the place
    /// shown as Corrin, and who is shown in Corrin's place (the one that
    /// place showed before, of Corrin's gender)
    pub corrin_swap: Option<[(usize, usize); 2]>,
    /// slot identity -> identity shown in its place
    pub shown: HashMap<usize, usize>,
    /// slot identity -> its classes (for the gender of whoever is shown)
    pub classes: HashMap<usize, Classes>,
    pub globals: Vec<GlobalPatch>,
    pub spoiler: Vec<String>,
}

/// the base classes a randomized character can have (both genders exist
/// for each, with models): no lords, nobles, Songstress, Villager or
/// enemy-only classes.
pub fn standard_base(cat: &Catalog) -> Vec<u16> {
    cat.jobs
        .iter()
        .filter(|j| {
            !j.promoted
                && j.id > 8
                && j.other_gender != 0
                && j.promotions[0] != 0
                && j.flags[4] & 0x80 == 0 // Villager, Nohr Prince(ss), the beasts (added below)
        })
        .map(|j| j.id)
        .chain(cat.jobs.iter().filter(|j| j.id == 95 || j.id == 96 || j.id == 99 || j.id == 100).map(|j| j.id)) // Wolfskin, Kitsune
        .collect()
}

/// whether a class is one the randomizer hands out or takes away: a
/// standard base class or one of their promotions.
pub fn is_standard(cat: &Catalog, id: u16) -> bool {
    let base = standard_base(cat);
    base.contains(&id) || base.iter().any(|&b| cat.job(b).is_some_and(|j| j.promotions.contains(&id)))
}

/// a class for the other gender (Monk <-> Shrine Maiden, Butler <-> Maid).
pub fn for_gender(cat: &Catalog, id: u16, female: bool) -> u16 {
    match cat.job(id) {
        Some(j) if j.female != female && j.other_gender != 0 => j.other_gender,
        _ => id,
    }
}

pub(super) fn eff_level(p: &PersonRec, job: Option<&JobRec>) -> i32 {
    let promoted = job.is_some_and(|j| j.promoted);
    p.level as i32 + if promoted { if p.internal != 0xFF && p.internal > 0 { p.internal as i32 } else { 20 } } else { 0 }
}

/// who is shown in each slot of a pool: a random permutation with as few
/// people left in their own place as can be.
pub(super) fn permute(rng: &mut Rng, pool: &[usize]) -> Vec<(usize, usize)> {
    let mut order = pool.to_vec();
    rng.shuffle(&mut order);
    if order.len() > 1 {
        for i in 0..order.len() {
            if order[i] == pool[i] {
                let j = (i + 1) % order.len();
                order.swap(i, j);
            }
        }
    }
    pool.iter().copied().zip(order).collect()
}

impl Plan {
    pub fn make(cat: &Catalog, seed: u32, options: u32) -> Plan {
        Plan::make_version(cat, seed, options, VERSION_FIRST)
    }

    /// a run's plan with the rules of `version`
    pub fn make_version(cat: &Catalog, seed: u32, options: u32, version: u32) -> Plan {
        if version >= 2 {
            return super::plan2::make(cat, seed, options, version);
        }
        Plan::make_v1(cat, seed, options)
    }

    /// the first round's rules, unchanged, for runs saved with them
    fn make_v1(cat: &Catalog, seed: u32, options: u32) -> Plan {
        let mut shown = HashMap::new();
        let idents = &cat.identities;
        let adults: Vec<usize> = (0..idents.len()).filter(|&i| !idents[i].child && (!idents[i].story || options & STORY != 0)).collect();
        let children: Vec<usize> = (0..idents.len()).filter(|&i| idents[i].child).collect();
        if options & CHARACTERS != 0 {
            let mut rng = Rng::new(seed, 1);
            for (slot, who) in permute(&mut rng, &adults) {
                shown.insert(slot, who);
            }
            if options & CHILDREN != 0 {
                let mut rng = Rng::new(seed, 2);
                for (slot, who) in permute(&mut rng, &children) {
                    shown.insert(slot, who);
                }
            }
        }
        let who = |slot: usize| *shown.get(&slot).unwrap_or(&slot);

        // classes: a base class and its promotion for each slot, for the
        // gender of whoever is shown there
        let base = standard_base(cat);
        let healers: Vec<u16> = base.iter().copied().filter(|&b| cat.job(b).is_some_and(|j| j.uses(6))).collect();
        let mut classes = HashMap::new();
        let mut rng = Rng::new(seed, 3);
        // a healer for the first retainer and each route's royal healer, so
        // every path keeps staves from its early chapters
        let healer_slots: Vec<&str> = vec!["サクラ", "エリーゼ", "フェリシア", "ジョーカー"];
        let promoted_pool: Vec<u16> = cat.jobs.iter().filter(|j| j.promoted && is_standard(cat, j.id)).map(|j| j.id).collect();
        for slot in 0..idents.len() {
            let female = idents[who(slot)].female;
            let fits = |id: u16| cat.job(id).is_some_and(|j| j.female == female);
            let healer = healer_slots.iter().any(|k| super::catalog::key_is(&idents[slot].key, k));
            // the class the slot has in its own place: its tier and what it
            // rides are kept, so fliers stay fliers and riders riders (maps
            // and cutscenes are made for them), and a promoted character
            // stays promoted
            let own = cat.job(idents[slot].class);
            let ride = own.map_or(0, |j| j.ride);
            let rides = |id: u16| cat.job(id).is_some_and(|j| j.ride == ride);
            let staff = |id: u16| !healer || cat.job(id).is_some_and(|j| j.uses(6));
            let narrow = |pool: &[u16], keep: &dyn Fn(u16) -> bool| -> Vec<u16> {
                let all: Vec<u16> = pool.iter().copied().filter(|&c| fits(c) && staff(c)).collect();
                let kept: Vec<u16> = all.iter().copied().filter(|&c| keep(c)).collect();
                if kept.is_empty() { all } else { kept }
            };
            let (pick, promoted) = if own.is_some_and(|j| j.promoted) {
                let pool = narrow(&promoted_pool, &rides);
                let promoted = pool[rng.below(pool.len())];
                let job = cat.job(promoted).unwrap();
                let from: Vec<u16> = job.from.iter().copied().filter(|&b| b != 0 && fits(b) && base.contains(&b)).collect();
                (from.first().copied().unwrap_or(promoted), promoted)
            } else {
                let pool = narrow(if healer { &healers } else { &base }, &rides);
                let pick = pool[rng.below(pool.len())];
                let job = cat.job(pick).unwrap();
                let promos: Vec<u16> = job.promotions.iter().copied().filter(|&p| p != 0 && fits(p)).collect();
                let promos = {
                    let same: Vec<u16> = promos.iter().copied().filter(|&p| rides(p) && staff(p)).collect();
                    if same.is_empty() { promos } else { same }
                };
                (pick, promos.get(rng.below(promos.len().max(1))).copied().unwrap_or(pick))
            };
            let others: Vec<u16> = base.iter().copied().filter(|&b| fits(b) && b != pick).collect();
            let r1 = others[rng.below(others.len())];
            let r2 = others[rng.below(others.len())];
            classes.insert(slot, Classes { base: pick, promoted, reclass: [r1, if r2 == r1 { 0 } else { r2 }] });
        }

        // GameData's playable characters
        let mut globals = Vec::new();
        let mut spoiler = Vec::new();
        for (slot, id) in idents.iter().enumerate() {
            let Some(index) = id.global else { continue };
            let s = &cat.persons.iter().find(|p| p.index == index).unwrap().clone();
            let shown_id = who(slot);
            let shown_identity = &idents[shown_id];
            let shown_person = shown_identity.global.and_then(|g| cat.persons.iter().find(|p| p.index == g));
            let female = shown_identity.female;
            let slot_job = cat.job(s.class);
            let promoted = slot_job.is_some_and(|j| j.promoted);
            let (class, class2, reclass) = if options & CLASSES != 0 {
                let c = classes[&slot];
                if promoted {
                    (c.promoted, c.base, c.reclass)
                } else if s.class2 != 0 {
                    (c.base, c.promoted, c.reclass)
                } else {
                    (c.base, 0, c.reclass)
                }
            } else {
                let mut class = for_gender(cat, s.class, female);
                if cat.job(class).is_some_and(|j| j.female != female) {
                    // no such class for this gender (Songstress): the slot's
                    // own random one
                    let c = classes[&slot];
                    class = if promoted { c.promoted } else { c.base };
                }
                let other = if s.class2 != 0 { for_gender(cat, s.class2, female) } else { 0 };
                (class, other, [for_gender(cat, s.reclass[0], female), for_gender(cat, s.reclass[1], female)])
            };
            let job = cat.job(class);
            // personal stats: the shown character's own, brought to the
            // slot's level with their growths
            let (bases, growths, modifiers, pskills, birthday) = match shown_person {
                Some(g) if shown_id != slot => {
                    let delta = eff_level(s, slot_job) - eff_level(g, cat.job(g.class));
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
            // weapon ranks: the slot's best rank, in the new class's weapons
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
                "{:<10} -> {:<10} {}",
                id.name,
                shown_identity.name,
                job.map_or(String::new(), |j| j.name.clone())
            ));
            globals.push(GlobalPatch { index, identity: shown_id, class, class2, bases, growths, modifiers, wexp, pskills, birthday, reclass });
        }
        for (slot, id) in idents.iter().enumerate() {
            if id.global.is_none() && shown.contains_key(&slot) {
                spoiler.push(format!("{:<10} -> {:<10} (story)", id.name, idents[who(slot)].name));
            }
        }
        Plan { seed, options, version: VERSION_FIRST, corrin: None, corrin_swap: None, shown, classes, globals, spoiler }
    }

    /// corrinswap: the swap for a Corrin of this gender: (the place shown
    /// as Corrin, who is shown in Corrin's place)
    pub fn corrin_pair(&self, female: bool) -> Option<(usize, usize)> {
        self.corrin_swap.map(|p| p[female as usize])
    }

    pub fn shown(&self, slot: usize) -> usize {
        *self.shown.get(&slot).unwrap_or(&slot)
    }

    /// lilith: whether the run changes this place at all (its look or its
    /// class). The characters added in version 3 are left exactly as the
    /// game has them in a classes-only run (it keeps the game's joins), so
    /// their chapter entries, cutscene casts and amiibo units stay theirs.
    pub fn touches(&self, slot: usize) -> bool {
        self.shown(slot) != slot || self.classes.contains_key(&slot)
    }

    /// the class a chapter's version of a slot gets in place of its own
    /// class, for the gender of whoever is shown.
    pub fn chapter_class(&self, cat: &Catalog, slot: usize, original: u16) -> u16 {
        let female = cat.identities[self.shown(slot)].female;
        let job = cat.job(original);
        let promoted = job.is_some_and(|j| j.promoted);
        let c = self.classes.get(&slot);
        let random = || c.map_or(original, |c| if promoted { c.promoted } else { c.base });
        if original == 0 {
            return 0;
        }
        if self.options & CLASSES != 0 {
            // every class a slot has, standard or a boss's own, becomes the
            // slot's random one of the same tier
            return random();
        }
        let converted = for_gender(cat, original, female);
        if cat.job(converted).is_some_and(|j| j.female != female) {
            return random();
        }
        // rando3: a class whose map sprite has no head for whoever is shown
        if self.version >= 2 && cat.job(converted).is_some_and(|j| !cat.sprites.fits(&j.key, &cat.identities[self.shown(slot)].aid_key)) {
            return random();
        }
        converted
    }
}
