//! oneexe: the game's three builds and their address maps, inside Haiku.exe.
//!
//! Haiku's overrides are written against the USA build (CTR-P-BFZE). The
//! EUR build (CTR-P-BFZP) and the Japanese one (Fire Emblem if,
//! CTR-P-BFZJ) have the same functions at other addresses. Haiku Setup
//! used to translate the overrides' C to them before compiling it; now the
//! overrides are compiled once, into Haiku.exe, and every address they use
//! is filled in when the game's module is bound (source/src/module.rs),
//! from these maps: dev/region/eur/map-eur.tsv and manual.tsv, and
//! dev/region/jpn/map-jpn.tsv, manual.tsv and build.txt (addresses only,
//! made by dev/region/map.py and bringup.sh), read as translate.py and
//! eur.rs read them. The game's module is built with its overrides' places
//! at the same addresses (module.rs).

use std::collections::HashMap;
use std::sync::OnceLock;

const EUR_MAP: &str = include_str!("../../dev/region/eur/map-eur.tsv");
const EUR_MANUAL: &str = include_str!("../../dev/region/eur/manual.tsv");
const JPN_MAP: &str = include_str!("../../dev/region/jpn/map-jpn.tsv");
const JPN_MANUAL: &str = include_str!("../../dev/region/jpn/manual.tsv");
/// the Japanese build the JPN map is made for (jpn.rs)
pub const JPN_BUILD: &str = include_str!("../../dev/region/jpn/build.txt");

pub const USA: u64 = 0x0004_0000_0017_9800;
pub const EUR: u64 = crate::eur::TITLE_ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Usa,
    Eur,
    Jpn,
}

impl Region {
    /// the build a title id is, any edition of the game
    pub fn of(title: u64) -> Option<Region> {
        match title {
            USA => Some(Region::Usa),
            EUR => Some(Region::Eur),
            t if crate::jpn::TITLES.contains(&t) => Some(Region::Jpn),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Region::Usa => "usa",
            Region::Eur => "eur",
            Region::Jpn => "jpn",
        }
    }
}

/// the USA -> region table (None for the USA build: the identity)
pub fn map(region: Region) -> Result<Option<&'static HashMap<u32, u32>>, String> {
    static EUR_TABLE: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    static JPN_TABLE: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    match region {
        Region::Usa => Ok(None),
        Region::Eur => Ok(Some(EUR_TABLE.get_or_init(|| crate::eur::parse_map(EUR_MAP, EUR_MANUAL)))),
        Region::Jpn => Ok(Some(JPN_TABLE.get_or_init(|| crate::eur::parse_map(JPN_MAP, JPN_MANUAL)))),
    }
}

/// what the region's table is made of (in the module's stamp: a module
/// built through another map is built again)
pub fn map_hash(region: Region) -> u64 {
    let texts: &[&str] = match region {
        Region::Usa => &[],
        Region::Eur => &[EUR_MAP, EUR_MANUAL],
        Region::Jpn => &[JPN_MAP, JPN_MANUAL, JPN_BUILD],
    };
    let mut hash = crate::module::FNV_START;
    for text in texts {
        crate::module::fnv(&mut hash, text.replace('\r', "").as_bytes());
        crate::module::fnv(&mut hash, b"|");
    }
    hash
}

/// a USA address in the region's build, as the text translation did it
pub fn translate(region: Region, us: u32) -> Result<u32, String> {
    match map(region)? {
        None => Ok(us),
        Some(table) => table.get(&us).copied().ok_or_else(|| format!("0x{us:08X} has no {} address in Haiku's map", region.tag())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_maps_read_as_the_text_translation_read_them() {
        let eur = map(Region::Eur).unwrap().unwrap();
        // map-eur.tsv's first rows and a manual line
        assert_eq!(eur.get(&0x0010_5E18), Some(&0x0010_5E48));
        let jpn = map(Region::Jpn).unwrap().unwrap();
        assert!(jpn.len() > 1000);
        assert_eq!(translate(Region::Usa, 0x0012_3456).unwrap(), 0x0012_3456);
    }
}
