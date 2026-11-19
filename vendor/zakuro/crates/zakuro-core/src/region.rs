//! eur: which build of Fire Emblem Fates Special Edition the recompiled code
//! is, and its addresses. jpprep: three regions, USA, EUR and JPN.
//!
//! Haiku is written against the USA build (CTR-P-BFZE, 0004000000179800):
//! every game address in Haiku's Rust and C is a USA one. The EUR build
//! (CTR-P-BFZP, 000400000017A800) is the same program a few bytes apart; the
//! Japanese one (Fire Emblem if, CTR-P-BFZJ, 000400000012DE00) is the same
//! program built earlier. Their overrides are translated when they are
//! recompiled (dev/region/translate.py); the Rust side asks this module:
//!
//! - [`us`]: the running build's address for a USA one (a constant Haiku
//!   reads or calls);
//! - [`to_us`]: the USA address for one of the running build's (a function
//!   or return address the game handed over), to compare with USA constants.
//!
//! On the USA build both are the identity (one relaxed load and an early
//! return), so the USA build does exactly what it did. The tables are made
//! by dev/region/map.py from the builds' own symbol tables (or, without
//! them, the code's bytes) and code, addresses only: EUR's is
//! dev/region/eur/map-eur.tsv (region_map.rs), JPN's
//! dev/region/jpn/map-jpn.tsv (region_map_jpn.rs, empty until
//! `dev/region/bringup.sh jpn <dump>` makes it).

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[path = "region_map.rs"]
mod map;
// jpprep: the Japanese build's table
#[path = "region_map_jpn.rs"]
mod map_jpn;

pub const USA: u64 = 0x0004_0000_0017_9800;
pub const EUR: u64 = 0x0004_0000_0017_A800;
/// jpprep: Fire Emblem if, the Japanese Special Edition (CTR-P-BFZJ)
pub const JPN: u64 = 0x0004_0000_0012_DE00;
/// jpprep: the Japanese single-route cartridges, Byakuya Oukoku (Birthright,
/// CTR-P-BFXJ) and Anya Oukoku (Conquest, CTR-P-BFYJ): Japanese builds too,
/// each with code of its own (a map is made for the one dumped)
pub const JPN_BIRTHRIGHT: u64 = 0x0004_0000_0012_DC00;
pub const JPN_CONQUEST: u64 = 0x0004_0000_0012_DD00;

/// jpprep: the builds Haiku knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Usa,
    Eur,
    Jpn,
}

impl Region {
    /// the region of a title id, any edition of the game (None: another
    /// game).
    pub fn of(title: u64) -> Option<Region> {
        match title {
            USA => Some(Region::Usa),
            EUR => Some(Region::Eur),
            JPN | JPN_BIRTHRIGHT | JPN_CONQUEST => Some(Region::Jpn),
            _ => None,
        }
    }

    /// "usa", "eur" or "jpn" (mod.toml titles, stamps, logs).
    pub fn tag(self) -> &'static str {
        match self {
            Region::Usa => "usa",
            Region::Eur => "eur",
            Region::Jpn => "jpn",
        }
    }
}

/// what [`to_us`] answers for an address of the running build's code Haiku
/// has no USA name for: never a USA constant (those are all even or in the
/// table)
pub const UNKNOWN: u32 = 0xFFFF_FFFF;

const REGION_USA: u8 = 0;
const REGION_EUR: u8 = 1;
const REGION_JPN: u8 = 2;
static REGION: AtomicU8 = AtomicU8::new(REGION_USA);
/// jpprep: the exact title (a Japanese build is one of three)
static TITLE: AtomicU64 = AtomicU64::new(USA);

/// the build the code linked in was recompiled from, set once at start-up.
pub fn set(title: u64) {
    let region = match Region::of(title) {
        Some(Region::Eur) => REGION_EUR,
        Some(Region::Jpn) => REGION_JPN,
        _ => REGION_USA,
    };
    REGION.store(region, Ordering::Relaxed);
    TITLE.store(if region == REGION_USA { USA } else { title }, Ordering::Relaxed);
    if region == REGION_EUR {
        log::info!(
            "eur: the code is the EUR Special Edition's (CTR-P-BFZP): {} USA addresses mapped, {} functions, {} data runs",
            map::EXACT.len(),
            map::FUNCTIONS.len(),
            map::DATA.len()
        );
    }
    if region == REGION_JPN {
        let t = tables(REGION_JPN);
        if t.exact.is_empty() {
            // never quietly: every address Haiku reads would be a USA one
            log::error!(
                "jpprep: the code is a Japanese build's ({title:016X}), but this Haiku has no JPN address table (region_map_jpn.rs is empty): make it with dev/region/bringup.sh jpn <dump>, then build again"
            );
        } else {
            log::info!(
                "jpprep: the code is the Japanese build's ({title:016X}): {} USA addresses mapped, {} functions, {} data runs",
                t.exact.len(),
                t.functions.len(),
                t.data.len()
            );
        }
    }
}

/// whether the code is the EUR build's.
#[inline]
pub fn is_eur() -> bool {
    REGION.load(Ordering::Relaxed) == REGION_EUR
}

/// jpprep: whether the code is a Japanese build's (Fire Emblem if).
#[inline]
pub fn is_jpn() -> bool {
    REGION.load(Ordering::Relaxed) == REGION_JPN
}

/// jpprep: the region of the build running.
pub fn current() -> Region {
    match REGION.load(Ordering::Relaxed) {
        REGION_EUR => Region::Eur,
        REGION_JPN => Region::Jpn,
        _ => Region::Usa,
    }
}

/// the title id of the build running.
pub fn title() -> u64 {
    TITLE.load(Ordering::Relaxed)
}

/// jpprep: whether the running build's addresses can be had (the USA build
/// needs no table, EUR's is built in, JPN's once bringup has made it).
pub fn has_table() -> bool {
    match REGION.load(Ordering::Relaxed) {
        REGION_USA => true,
        region => !tables(region).exact.is_empty(),
    }
}

/// jpprep: one build's tables, USA -> that build.
struct Tables {
    exact: &'static [(u32, u32)],
    functions: &'static [(u32, u32, u32)],
    data: &'static [(u32, u32, i32)],
}

static EUR_TABLES: Tables = Tables { exact: &map::EXACT, functions: &map::FUNCTIONS, data: &map::DATA };
static JPN_TABLES: Tables = Tables { exact: &map_jpn::EXACT, functions: &map_jpn::FUNCTIONS, data: &map_jpn::DATA };

fn tables(region: u8) -> &'static Tables {
    if region == REGION_JPN {
        &JPN_TABLES
    } else {
        &EUR_TABLES
    }
}

/// the running build's address for the USA address `a` (the identity on the
/// USA build): one Haiku names, else by its offset in a function the same
/// size in both builds, else by the move of the data around it. An address
/// none of those place (inside a function the other build changed) is
/// logged once and given back as it is.
#[inline]
pub fn us(a: u32) -> u32 {
    let region = REGION.load(Ordering::Relaxed);
    if region == REGION_USA {
        return a;
    }
    mapped(a, region)
}

#[cold]
fn mapped(a: u32, region: u8) -> u32 {
    let t = tables(region);
    if let Ok(i) = t.exact.binary_search_by_key(&a, |&(us, _)| us) {
        return t.exact[i].1;
    }
    let f = t.functions.partition_point(|&(us, _, _)| us <= a);
    if f > 0 {
        let (us, eu, len) = t.functions[f - 1];
        if a < us + len {
            return eu + (a - us);
        }
    }
    let d = t.data.partition_point(|&(start, _, _)| start <= a);
    if d > 0 {
        let (start, end, shift) = t.data[d - 1];
        if (start..end).contains(&a) {
            return a.wrapping_add_signed(shift);
        }
    }
    if (0x0010_0000..0x0080_0000).contains(&a) {
        missing(a);
    }
    a
}

/// the USA address of the running build's address `a`, to compare with the
/// USA constants (the identity on the USA build), the inverse of [`us`]. An
/// address of the code [`us`] can't give back is [`UNKNOWN`]; anything
/// outside the image (heap, stacks) is given back as it is.
#[inline]
pub fn to_us(a: u32) -> u32 {
    let region = REGION.load(Ordering::Relaxed);
    if region == REGION_USA {
        return a;
    }
    us_of(a, region)
}

struct Inverse {
    exact: Vec<(u32, u32)>,
    functions: Vec<(u32, u32, u32)>,
    data: Vec<(u32, u32, i32)>,
}

impl Inverse {
    fn of(t: &Tables) -> Inverse {
        let mut exact: Vec<(u32, u32)> = t.exact.iter().map(|&(us, eu)| (eu, us)).collect();
        exact.sort_unstable();
        let mut functions: Vec<(u32, u32, u32)> = t.functions.iter().map(|&(us, eu, n)| (eu, us, n)).collect();
        functions.sort_unstable();
        let mut data: Vec<(u32, u32, i32)> = t.data.iter().map(|&(s, e, d)| (s.wrapping_add_signed(d), e.wrapping_add_signed(d), -d)).collect();
        data.sort_unstable();
        Inverse { exact, functions, data }
    }
}

#[cold]
fn us_of(a: u32, region: u8) -> u32 {
    static EUR_INVERSE: std::sync::OnceLock<Inverse> = std::sync::OnceLock::new();
    static JPN_INVERSE: std::sync::OnceLock<Inverse> = std::sync::OnceLock::new();
    let inv = if region == REGION_JPN {
        JPN_INVERSE.get_or_init(|| Inverse::of(&JPN_TABLES))
    } else {
        EUR_INVERSE.get_or_init(|| Inverse::of(&EUR_TABLES))
    };
    if let Ok(i) = inv.exact.binary_search_by_key(&a, |&(eu, _)| eu) {
        return inv.exact[i].1;
    }
    let f = inv.functions.partition_point(|&(eu, _, _)| eu <= a);
    if f > 0 {
        let (eu, us, len) = inv.functions[f - 1];
        if a < eu + len {
            return us + (a - eu);
        }
    }
    if a < 0x0061_1000 && a >= 0x0010_0000 {
        return UNKNOWN;
    }
    let d = inv.data.partition_point(|&(start, _, _)| start <= a);
    if d > 0 {
        let (start, end, shift) = inv.data[d - 1];
        if (start..end).contains(&a) {
            return a.wrapping_add_signed(shift);
        }
    }
    if (0x0010_0000..0x0080_0000).contains(&a) {
        return UNKNOWN;
    }
    a
}

fn missing(a: u32) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&a) {
        seen.push(a);
        log::warn!("eur: no {} address for the USA address 0x{a:08X}, used as it is", if is_jpn() { "JPN" } else { "EUR" });
    }
}

/// the message folder of the language the game shows: `@E` on the USA build;
/// on the EUR build `@U`, `@F`, `@G`, `@I` or `@S` from the language the
/// system reports (what the game's Owner::Initialize picks: CFG 1 English,
/// 2 French, 3 German, 4 Italian, 5 Spanish, anything else English).
/// jpprep: none on the Japanese build, whose messages are `m/X` themselves:
/// use [`message_path`] to join it.
pub fn message_folder(cfg_language: u8) -> &'static str {
    if is_jpn() {
        return "";
    }
    if !is_eur() {
        return "@E";
    }
    match cfg_language {
        2 => "@F",
        3 => "@G",
        4 => "@I",
        5 => "@S",
        _ => "@U",
    }
}

/// jpprep: a message file's path in the build running, from its name under
/// `m/` (`"GameData.bin.lz"`, `"A/A007.bin.lz"`): `m/@E/GameData.bin.lz`,
/// `m/A/@E/A007.bin.lz` on the USA build, `@U` (or the language's) on EUR,
/// `m/GameData.bin.lz`, `m/A/A007.bin.lz` on the Japanese one.
pub fn message_path(name: &str, cfg_language: u8) -> String {
    let folder = message_folder(cfg_language);
    let (dir, file) = match name.rsplit_once('/') {
        Some((dir, file)) => (format!("m/{dir}"), file),
        None => ("m".to_owned(), name),
    };
    if folder.is_empty() {
        format!("{dir}/{file}")
    } else {
        format!("{dir}/{folder}/{file}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_sorted() {
        for t in [&EUR_TABLES, &JPN_TABLES] {
            assert!(t.exact.windows(2).all(|w| w[0].0 < w[1].0));
            assert!(t.functions.windows(2).all(|w| w[0].0 + w[0].2 <= w[1].0));
            assert!(t.data.windows(2).all(|w| w[0].1 <= w[1].0));
        }
    }

    // one test sets the region for all of them: the region is global
    #[test]
    fn usa_is_the_identity_and_eur_goes_both_ways() {
        set(USA);
        assert_eq!(current(), Region::Usa);
        assert_eq!(us(0x006D_8808), 0x006D_8808);
        assert_eq!(to_us(0x006D_89A0), 0x006D_89A0);
        assert_eq!(message_path("A/A007.bin.lz", 1), "m/A/@E/A007.bin.lz");
        set(EUR);
        assert_eq!(title(), EUR);
        // GameSkip* (overrides/qol.c), GameSkip::Trigger
        assert_eq!(us(0x006D_8808), 0x006D_89A0);
        assert_eq!(us(0x004E_B9FC), 0x004E_C428);
        assert_eq!(to_us(0x004E_C428), 0x004E_B9FC);
        // an offset inside a function the same size: Trigger + 8
        assert_eq!(us(0x004E_BA04), 0x004E_C430);
        assert_eq!(to_us(0x004E_C430), 0x004E_BA04);
        // the heap is no game address
        assert_eq!(to_us(0x0812_3456), 0x0812_3456);
        assert_eq!(message_path("GameData.bin.lz", 2), "m/@F/GameData.bin.lz");
        // jpprep: a Japanese build, any of the three titles; its table is
        // whatever bringup made (empty here: the address comes back as it is)
        set(JPN_BIRTHRIGHT);
        assert_eq!((current(), title(), is_jpn(), is_eur()), (Region::Jpn, JPN_BIRTHRIGHT, true, false));
        assert_eq!(message_path("GameData.bin.lz", 1), "m/GameData.bin.lz");
        assert_eq!(message_path("A/A007.bin.lz", 0), "m/A/A007.bin.lz");
        if !has_table() {
            assert_eq!(us(0x006D_8808), 0x006D_8808);
        }
        assert_eq!(to_us(0x0812_3456), 0x0812_3456);
        // an unknown title is the USA build's (as before)
        set(0x0004_0000_0000_1000);
        assert_eq!((current(), title()), (Region::Usa, USA));
        assert_eq!(Region::of(JPN).map(Region::tag), Some("jpn"));
        set(USA);
    }
}
