//! eur: Haiku Setup for the EUR Special Edition (CTR-P-BFZP,
//! 000400000017A800), kept apart from the USA path so the two merge easily.
//!
//! The EUR game is the same program as the USA one, a few bytes apart. Its
//! code is recompiled with Haiku's overrides translated to the EUR addresses
//! (every USA game address in them through the map Haiku ships,
//! setup/eur/map-eur.tsv and manual.tsv, made by dev/region/map.py from the two
//! builds' own symbol tables: addresses only). The DLC is fitted to it after
//! it is installed (zakuro_fs::eur). What the USA path does is unchanged.

use std::collections::HashMap;
use std::path::Path;

pub const TITLE_ID: u64 = 0x0004_0000_0017_A800;
pub const PRODUCT: &str = "CTR-P-BFZP";
/// SHA-256 of the ExeFS .code (as stored, compressed) of the EUR Special
/// Edition cartridge, the code the map is made for.
pub const CODE_SHA256: &str = "7fd7c03775fcac9d412f29f63692d69694cef41a3e06a7d485833d301ab9c131";
/// the DLC the EUR game asks for (its unique id 0x1795)
pub const DLC_TITLE: u64 = 0x0004_008C_0017_9500;

/// whether a title is the EUR Special Edition.
pub fn is_eur(program_id: u64, product: &str) -> bool {
    program_id == TITLE_ID && product == PRODUCT
}

/// the USA -> EUR address table: the map's rows that have an EUR address,
/// then the hand-made ones over them.
pub fn load_map(dir: &Path) -> Result<HashMap<u32, u32>, String> {
    load_map_named(dir, "map-eur.tsv")
}

/// jpprep: any region's table (dir/<map>, then dir/manual.tsv). A manual
/// line may say `none` (0: an address that build doesn't have) or end in ?
/// (used only where the map has none), as dev/region/translate.py reads it.
pub fn load_map_named(dir: &Path, map: &str) -> Result<HashMap<u32, u32>, String> {
    let read = |name: &str| {
        let path = dir.join(name);
        std::fs::read_to_string(&path).map_err(|e| format!("{} is missing ({e}). Extract the whole Haiku download again.", path.display()))
    };
    Ok(parse_map(&read(map)?, &read("manual.tsv")?))
}

/// oneexe: a map's text and its manual.tsv's, as load_map_named reads them
/// (regions.rs: the maps Haiku.exe carries).
pub fn parse_map(map_text: &str, manual_text: &str) -> HashMap<u32, u32> {
    let mut table = HashMap::new();
    for (name, text) in [("map", map_text), ("manual.tsv", manual_text)] {
        let column = 1;
        for line in text.lines() {
            if line.starts_with('#') || line.starts_with("us\t") || line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            let parse = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
            // jpprep: none, and ? for a fallback (manual.tsv only)
            let value = cols.get(column).copied().unwrap_or("");
            let fallback = name == "manual.tsv" && value.ends_with('?');
            let value = value.trim_end_matches('?');
            let to = if name == "manual.tsv" && value.eq_ignore_ascii_case("none") { Some(0) } else { parse(value) };
            if let (Some(us), Some(eu)) = (cols.first().and_then(|s| parse(s)), to) {
                if fallback {
                    table.entry(us).or_insert(eu);
                } else {
                    table.insert(us, eu);
                }
            }
        }
    }
    table
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// one piece of C code (no comments, no strings): every `0x00XXXXXX` game
/// address and every `f_00XXXXXX` function name translated.
fn translate_code(code: &str, table: &HashMap<u32, u32>, missing: &mut Vec<u32>) -> String {
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut i = 0;
    while i < b.len() {
        let before_ok = i == 0 || !is_word(b[i - 1]);
        // 0x00[1-7]XXXXX, then optional u/U, then not a word character
        if before_ok && b[i..].starts_with(b"0x00") && i + 10 <= b.len() && (b'1'..=b'7').contains(&b[i + 4]) && b[i + 5..i + 10].iter().all(|&c| is_hex(c)) {
            let mut end = i + 10;
            let suffix = end < b.len() && (b[end] == b'u' || b[end] == b'U');
            if suffix {
                end += 1;
            }
            if end >= b.len() || !is_word(b[end]) {
                let us = u32::from_str_radix(&code[i + 2..i + 10], 16).unwrap();
                match table.get(&us) {
                    Some(&eu) => out.push_str(&format!("0x{eu:08X}{}", if suffix { &code[end - 1..end] } else { "" })),
                    None => {
                        missing.push(us);
                        out.push_str(&code[i..end]);
                    }
                }
                i = end;
                continue;
            }
        }
        if before_ok && b[i..].starts_with(b"f_00") && i + 10 <= b.len() && (b'1'..=b'7').contains(&b[i + 4]) && b[i + 5..i + 10].iter().all(|&c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)) && (i + 10 >= b.len() || !is_word(b[i + 10])) {
            let us = u32::from_str_radix(&code[i + 2..i + 10], 16).unwrap();
            match table.get(&us) {
                Some(&eu) => out.push_str(&format!("f_{eu:08X}")),
                None => {
                    missing.push(us);
                    out.push_str(&code[i..i + 10]);
                }
            }
            i += 10;
            continue;
        }
        let ch = code[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// a C file's text with its game addresses translated outside comments and
/// strings, as dev/region/translate.py does.
pub fn translate_c(text: &str, table: &HashMap<u32, u32>, missing: &mut Vec<u32>) -> String {
    translate_c_titled(text, table, missing, "0x000400000017A800ull")
}

/// jpprep: translate_c for any build, its title (`0x...ull`) written in
/// eur_region.c's line.
pub fn translate_c_titled(text: &str, table: &HashMap<u32, u32>, missing: &mut Vec<u32>, title: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.split_inclusive('\n') {
        let mut rest = line;
        let mut pieces = String::new();
        while !rest.is_empty() {
            if in_block {
                match rest.find("*/") {
                    Some(at) => {
                        pieces.push_str(&rest[..at + 2]);
                        rest = &rest[at + 2..];
                        in_block = false;
                    }
                    None => {
                        pieces.push_str(rest);
                        rest = "";
                    }
                }
                continue;
            }
            let next = [rest.find("/*"), rest.find("//"), rest.find('"')].into_iter().flatten().min();
            let Some(at) = next else {
                pieces.push_str(&translate_code(rest, table, missing));
                break;
            };
            pieces.push_str(&translate_code(&rest[..at], table, missing));
            rest = &rest[at..];
            if rest.starts_with('"') {
                let bytes = rest.as_bytes();
                let mut k = 1;
                while k < bytes.len() && bytes[k] != b'"' {
                    k += if bytes[k] == b'\\' { 2 } else { 1 };
                }
                let end = (k + 1).min(rest.len());
                pieces.push_str(&rest[..end]);
                rest = &rest[end..];
            } else if rest.starts_with("//") {
                pieces.push_str(rest);
                rest = "";
            } else {
                in_block = true;
                pieces.push_str("/*");
                rest = &rest[2..];
            }
        }
        // the title the code is made from (overrides/eur_region.c)
        if pieces.contains("eur: title") {
            pieces = pieces.replace("0x0004000000179800ull", title);
        }
        out.push_str(&pieces);
    }
    out
}

/// translates every .c and .h file in dir in place (amiibo_icons.c is
/// pixels, left as it is); the USA addresses no map has, an error.
pub fn translate_overrides(dir: &Path, map_dir: &Path) -> Result<usize, String> {
    let table = load_map(map_dir)?;
    translate_dir(dir, &table, "0x000400000017A800ull", "EUR")
}

/// jpprep: translate_overrides for any build: its table, title and name.
pub fn translate_dir(dir: &Path, table: &HashMap<u32, u32>, title: &str, region: &str) -> Result<usize, String> {
    let mut missing = Vec::new();
    let mut changed = 0;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(".c") || name.ends_with(".h")) || name == "amiibo_icons.c" {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let new = translate_c_titled(&text, table, &mut missing, title);
        if new != text {
            std::fs::write(&path, new).map_err(|e| format!("writing {}: {e}", path.display()))?;
            changed += 1;
        }
    }
    if !missing.is_empty() {
        missing.sort_unstable();
        missing.dedup();
        let list: Vec<String> = missing.iter().take(8).map(|a| format!("0x{a:08X}")).collect();
        return Err(format!("{} of Haiku's game addresses have no {region} address in the map ({}...)", missing.len(), list.join(", ")));
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_in_code_change_and_comments_and_strings_do_not() {
        let table: HashMap<u32, u32> = [(0x006D_8808, 0x006D_89A0), (0x001334FC, 0x001336B4)].into_iter().collect();
        let mut missing = Vec::new();
        let text = "#define GAME_SKIP 0x006D8808u /* 0x006D8808 */\nCALL(f_001334FC); // 0x006D8808\nputs(\"0x006D8808\"); x = 0x00100000000;\n";
        let out = translate_c(text, &table, &mut missing);
        assert_eq!(out, "#define GAME_SKIP 0x006D89A0u /* 0x006D8808 */\nCALL(f_001336B4); // 0x006D8808\nputs(\"0x006D8808\"); x = 0x00100000000;\n");
        assert!(missing.is_empty());
        let out = translate_c("RECOMP_OVERRIDE(0x00123456) {\n", &table, &mut missing);
        assert_eq!(out, "RECOMP_OVERRIDE(0x00123456) {\n");
        assert_eq!(missing, vec![0x0012_3456]);
        let out = translate_c("const unsigned long long haiku_code_title = 0x0004000000179800ull; /* eur: title */\n", &table, &mut Vec::new());
        assert!(out.contains("0x000400000017A800ull"));
    }
}

/// whether a checked dump is the EUR Special Edition (its NCCH header's
/// program id).
pub fn game_is_eur(game: &crate::dump::Game) -> bool {
    crate::dump::Dump::open(&game.path)
        .and_then(|mut d| d.read(game.ncch_offset + 0x118, 8))
        .is_ok_and(|id| u64::from_le_bytes(id.try_into().unwrap_or([0; 8])) == TITLE_ID)
}
