//! jpprep: Haiku Setup for the Japanese game, Fire Emblem if (the Special
//! Edition CTR-P-BFZJ, 000400000012DE00; the single-route cartridges are
//! Japanese builds too, 000400000012DC00 and DD00), and which DLC goes with
//! which game. Kept apart from the USA and EUR paths so they merge easily.
//!
//! The Japanese game is built like the EUR one (eur.rs): its code is
//! recompiled with Haiku's overrides translated through a map Haiku ships,
//! setup/jpn/map-jpn.tsv and manual.tsv, made by dev/region/bringup.sh from
//! the player's own dump (addresses only). setup/jpn/build.txt names the
//! title and the .code that map is made for: a Japanese dump is accepted
//! when its code is that one, and refused, saying why, before anything is
//! installed, when this Haiku has no map for it yet.
//!
//! Its DLC is the Japanese DLC, 0004008C0012DC00 (one title for every
//! Japanese edition; on the Special Edition it holds the third route), used
//! as it is. A USA or EUR DLC can't serve the Japanese game, nor the
//! Japanese DLC the others (jpdlc: its voices go in Haiku's voices screen).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dump::Problem;

pub const SPECIAL_EDITION: u64 = 0x0004_0000_0012_DE00;
pub const BIRTHRIGHT: u64 = 0x0004_0000_0012_DC00;
pub const CONQUEST: u64 = 0x0004_0000_0012_DD00;
pub const TITLES: [u64; 3] = [SPECIAL_EDITION, BIRTHRIGHT, CONQUEST];
/// the Japanese DLC, for every Japanese edition
pub const DLC_TITLE: u64 = 0x0004_008C_0012_DC00;
/// the USA DLC and the one the EUR game asks for
pub const USA_DLC: u64 = 0x0004_008C_0017_9400;
pub const EUR_DLC: u64 = 0x0004_008C_0017_9500;

/// whether a title is a Japanese build of the game: one of its titles with
/// a Japanese Fire Emblem product code (CTR-P-BF?J).
pub fn is_jpn(program_id: u64, product: &str) -> bool {
    TITLES.contains(&program_id) && product.len() == 10 && product.starts_with("CTR-P-BF") && product.ends_with('J')
}

/// the edition's name, as the panel says it.
pub fn edition(program_id: u64) -> &'static str {
    match program_id {
        BIRTHRIGHT => "Fire Emblem if: Byakuya Oukoku (Birthright)",
        CONQUEST => "Fire Emblem if: Anya Oukoku (Conquest)",
        _ => "Fire Emblem if Special Edition (Japan)",
    }
}

/// where Haiku Setup keeps what it ships (setup/ beside it);
/// HAIKU_SETUP_DIR for tests and scripts.
pub fn setup_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HAIKU_SETUP_DIR") {
        return PathBuf::from(dir);
    }
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("setup"))).unwrap_or_else(|| PathBuf::from("setup"))
}

/// what setup/jpn/build.txt says: the title and .code its map is for.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    pub title: u64,
    pub code_sha256: String,
}

/// the Japanese build this Haiku has a map for, or why it has none.
pub fn build(setup: &Path) -> Result<Build, String> {
    // oneexe: the map is in Haiku.exe (regions.rs); a setup/jpn folder (a
    // test's) is read instead when there is one
    let dir = setup.join("jpn");
    let text = if dir.is_dir() {
        let text = std::fs::read_to_string(dir.join("build.txt")).map_err(|_| "This Haiku has no address map for the Japanese game yet".to_owned())?;
        if !dir.join("map-jpn.tsv").is_file() {
            return Err("This Haiku's Japanese address map is missing (setup/jpn/map-jpn.tsv): extract the whole Haiku download again".to_owned());
        }
        text
    } else {
        crate::regions::JPN_BUILD.to_owned()
    };
    let mut build = Build { title: 0, code_sha256: String::new() };
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("title", v)) => build.title = u64::from_str_radix(v.trim(), 16).unwrap_or(0),
            Some(("code_sha256", v)) => build.code_sha256 = v.trim().to_ascii_lowercase(),
            _ => {}
        }
    }
    if build.title == 0 || build.code_sha256.len() != 64 {
        return Err("This Haiku's setup/jpn/build.txt is damaged: extract the whole Haiku download again".to_owned());
    }
    Ok(build)
}

/// the check of a Japanese dump's code (its ExeFS .code's SHA-256), against
/// the map this Haiku ships.
pub fn check_code(setup: &Path, program_id: u64, product: &str, code_sha256: &str) -> Result<(), Problem> {
    let name = edition(program_id);
    match build(setup) {
        Err(why) => Err(Problem::WrongVersion(format!(
            "This is {name} ({product}). {why}: it is made from a Japanese dump the first time one is brought up (dev/region/bringup.sh jpn), so this release can't build it. Use a Haiku release that says it supports the Japanese game."
        ))),
        Ok(b) if b.title == program_id && b.code_sha256 == code_sha256 => Ok(()),
        Ok(b) if b.title != program_id => Err(Problem::WrongVersion(format!(
            "This is {name} ({product}, {program_id:016X}), but this Haiku's Japanese map is made for {} ({:016X}), whose code differs. Use that edition's dump.",
            edition(b.title),
            b.title
        ))),
        Ok(_) => Err(Problem::WrongVersion(format!(
            "This is {name} ({product}), but its code is not the version this Haiku's Japanese map is made for. The dump may have an update or a code mod merged into it (the Uncensored Patch's IfPatcher output, for one): dump the original cartridge or eShop game, and add patches as mods."
        ))),
    }
}

/// whether a checked dump is a Japanese build (its NCCH header).
pub fn game_is_jpn(game: &crate::dump::Game) -> bool {
    let mut dump = match crate::dump::Dump::open(&game.path) {
        Ok(dump) => dump,
        Err(_) => return false,
    };
    let (Ok(id), Ok(product)) = (dump.read(game.ncch_offset + 0x118, 8), dump.read(game.ncch_offset + 0x150, 16)) else { return false };
    let id = u64::from_le_bytes(id.try_into().unwrap_or([0; 8]));
    is_jpn(id, String::from_utf8_lossy(&product).trim_end_matches('\0'))
}

/// which region an installed game is (data/install.txt's product).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GameRegion {
    Usa,
    Eur,
    Jpn,
}

pub fn installed_region(data: &Path) -> Option<GameRegion> {
    let text = std::fs::read_to_string(data.join(zakuro_fs::datadir::MANIFEST_FILE)).ok()?;
    let product = text.lines().find_map(|line| line.strip_prefix("product "))?.trim().to_owned();
    match product.as_str() {
        "CTR-P-BFZE" => Some(GameRegion::Usa),
        "CTR-P-BFZP" => Some(GameRegion::Eur),
        p if p.starts_with("CTR-P-BF") && p.ends_with('J') => Some(GameRegion::Jpn),
        _ => None,
    }
}

/// whether a DLC title can serve the game: the Japanese DLC only the
/// Japanese game; the USA DLC (region-free CIAs carry it) the USA and EUR
/// games (eur.rs files it as the EUR game asks); the EUR DLC the EUR game.
pub fn dlc_fits(game: GameRegion, dlc_title: u64) -> Result<(), String> {
    match (game, dlc_title) {
        (GameRegion::Jpn, DLC_TITLE) | (GameRegion::Usa, USA_DLC) | (GameRegion::Eur, USA_DLC) | (GameRegion::Eur, EUR_DLC) => Ok(()),
        (GameRegion::Jpn, _) => Err(format!(
            "This DLC ({dlc_title:016X}) is the {} one, and your game is the Japanese Fire Emblem if: its DLC is the Japanese DLC (0004008C0012DC00), which also holds the third route on the Special Edition.",
            if dlc_title == EUR_DLC { "European" } else { "USA" }
        )),
        (_, DLC_TITLE) => Err(
            "This is the Japanese DLC (0004008C0012DC00): it can't serve the USA or European game (its maps' text is Japanese, and the Japan-only content isn't in that game). Its Japanese voices can be added in Voices (Add Japanese voices).".to_owned(),
        ),
        (GameRegion::Usa, EUR_DLC) => Err(
            // eurdlc: plainer
            "This is the European DLC (0004008C00179500), and your game is the USA Special Edition, which only reads the USA DLC (0004008C00179400). Pick the USA DLC (a region-free DLC CIA has it), or build from your European game to use this one.".to_owned(),
        ),
        _ => Err(format!("This DLC ({dlc_title:016X}) is for a different game.")),
    }
}

/// the USA -> JPN table: map-jpn.tsv's rows that have an address, then
/// manual.tsv's (`none` is 0: an address the Japanese build doesn't have;
/// a value ending in ? only where the map has none), as
/// dev/region/translate.py reads them.
pub fn load_map(dir: &Path) -> Result<HashMap<u32, u32>, String> {
    crate::eur::load_map_named(dir, "map-jpn.tsv")
}

/// translates every .c and .h file in dir to the Japanese build's
/// addresses, in place (eur.rs's translation with the Japanese title).
pub fn translate_overrides(dir: &Path, setup: &Path) -> Result<usize, String> {
    let b = build(setup)?;
    let table = load_map(&setup.join("jpn"))?;
    crate::eur::translate_dir(dir, &table, &format!("0x{:016X}ull", b.title), "JPN")
}

/// the menus' font (font/System_L.bfnt.lz) as Haiku's own menus (fates_ui)
/// read it: each sheet decoded to assets/font/System_L_<n>.png and the
/// glyph records as System_L-metrics.json. The Japanese font has two
/// 1024x1024 sheets and 7,089 glyphs (a glyph's sheet is its codepoint's
/// high half); the USA one one 256x256 sheet. Made from the player's dump
/// at install, nothing shipped. Returns the sheets written.
pub fn make_font(romfs: &Path, assets: &Path) -> Result<usize, String> {
    let path = romfs.join("font/System_L.bfnt.lz");
    let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bfnt = crate::lz11::decompress(&raw)?;
    let u16_at = |at: usize| bfnt.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32);
    let u32_at = |at: usize| bfnt.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let (Some(w), Some(h), Some(sheet), Some(sheets), Some(at)) = (u16_at(0x10), u16_at(0x12), u32_at(0x14), u16_at(0x1A), u32_at(0x24)) else {
        return Err(format!("{}: not a font", path.display()));
    };
    if w == 0 || h == 0 || sheets == 0 || sheets > 8 || sheet != crate::tex::size(w, h, FONT_FORMAT) {
        return Err(format!("{}: a font laid out as Haiku doesn't know ({w}x{h}, {sheets} sheets, {sheet} bytes each)", path.display()));
    }
    let dir = assets.join("font");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for s in 0..sheets as usize {
        let data = bfnt.get(at + s * sheet..at + (s + 1) * sheet).ok_or_else(|| format!("{}: sheet {s} is cut short", path.display()))?;
        let rgba = crate::tex::decode(data, w, h, FONT_FORMAT).ok_or_else(|| format!("{}: sheet {s} can't be decoded", path.display()))?;
        let picture = crate::derive::Picture { w, h, rgba: if FONT_FLIP { crate::tex::flip(&rgba, w, h) } else { rgba } };
        crate::derive::write_png(&dir.join(format!("System_L_{s}.png")), &picture)?;
    }
    let metrics = crate::derive::font_metrics(&bfnt, false).ok_or_else(|| format!("{}: its glyph records can't be read", path.display()))?;
    std::fs::write(dir.join("System_L-metrics.json"), metrics).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(sheets as usize)
}

/// the font sheets' texture format and orientation (as the USA sheet the
/// repository's assets/font/System_L_0.png was decoded from: checked by
/// --jpn-font against it)
const FONT_FORMAT: u8 = 10;
const FONT_FLIP: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_products_and_dlc() {
        assert!(is_jpn(SPECIAL_EDITION, "CTR-P-BFZJ"));
        assert!(is_jpn(BIRTHRIGHT, "CTR-P-BFXJ"));
        assert!(!is_jpn(SPECIAL_EDITION, "CTR-P-BFZE"));
        assert!(!is_jpn(0x0004_0000_0017_9800, "CTR-P-BFZJ"));
        assert!(dlc_fits(GameRegion::Jpn, DLC_TITLE).is_ok());
        assert!(dlc_fits(GameRegion::Eur, EUR_DLC).is_ok());
        assert!(dlc_fits(GameRegion::Eur, USA_DLC).is_ok());
        assert!(dlc_fits(GameRegion::Usa, USA_DLC).is_ok());
        assert!(dlc_fits(GameRegion::Jpn, USA_DLC).unwrap_err().contains("Japanese DLC"));
        assert!(dlc_fits(GameRegion::Usa, DLC_TITLE).unwrap_err().contains("Japanese voices"));
        assert!(dlc_fits(GameRegion::Usa, EUR_DLC).is_err());
    }

    #[test]
    fn a_japanese_dump_needs_the_map_it_was_made_for() {
        let setup = std::env::temp_dir().join(format!("haiku-jpn-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&setup);
        std::fs::create_dir_all(setup.join("jpn")).unwrap();
        let code = "ab".repeat(32);
        // no map: refused before anything is installed
        let no = check_code(&setup, SPECIAL_EDITION, "CTR-P-BFZJ", &code).unwrap_err();
        assert!(no.message().contains("no address map for the Japanese game"), "{}", no.message());
        std::fs::write(setup.join("jpn/build.txt"), format!("title 000400000012DE00\ncode_sha256 {code}\nhints = 000400000012DE00 0123456789ABCDEF\n")).unwrap();
        std::fs::write(setup.join("jpn/map-jpn.tsv"), "us\tjpn\n0x00100010\t0x00100020\tx\n").unwrap();
        std::fs::write(setup.join("jpn/manual.tsv"), "us\tjpn\twhy\n0x006BDA6C\tnone?\tLang::Folder\n").unwrap();
        assert!(check_code(&setup, SPECIAL_EDITION, "CTR-P-BFZJ", &code).is_ok());
        assert!(check_code(&setup, SPECIAL_EDITION, "CTR-P-BFZJ", &"cd".repeat(32)).unwrap_err().message().contains("not the version"));
        assert!(check_code(&setup, BIRTHRIGHT, "CTR-P-BFXJ", &code).unwrap_err().message().contains("made for"));
        // the translation: the title, a mapped address, `none?`
        let dir = setup.join("overrides");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.c"), "const unsigned long long haiku_code_title = 0x0004000000179800ull; /* eur: title */\n#define L 0x006BDA6Cu\nRECOMP_OVERRIDE(0x00100010) {}\n").unwrap();
        assert_eq!(translate_overrides(&dir, &setup).unwrap(), 1);
        let out = std::fs::read_to_string(dir.join("a.c")).unwrap();
        assert!(out.contains("0x000400000012DE00ull") && out.contains("#define L 0x00000000u") && out.contains("RECOMP_OVERRIDE(0x00100020)"), "{out}");
        let _ = std::fs::remove_dir_all(&setup);
    }
}
