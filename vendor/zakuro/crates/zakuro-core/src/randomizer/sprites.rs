//! randomizer: which map sprites exist, so a class is only given to a
//! character whose map sprite can show it (rando3).
//!
//! A unit's map sprite is two sheets drawn together
//! (UnitIconObject::ReadSheet): the class's body, `unit/Body/<class>/`, and
//! the character's head, `unit/Head/<AID without AID_>/` (a few classes and
//! characters have one whole sprite instead, `unit/Unique/<class>_<chara>/`).
//! A head sheet (`青1.bch.lz`, 512x128) holds the head in each frame the
//! bodies need, in 32-pixel columns: the mounted bodies (horse, pegasus,
//! kinshi, wyvern) take theirs from columns 1 to 5, the robed ones
//! (Adventurer, Butler, Maid, Monk, Shrine Maiden, Onmyoji...) from columns
//! 6 and 7, every body from 8 to 15. The game's own generic head of a class
//! (`unit/Head/<class>/`, the enemy soldier's) has exactly the columns that
//! class's body uses. A character who never rode in the game has no
//! mounted columns: Kotaro, Hans and Zola have columns 8 to 15 only, and
//! Garon has no head sheet at all, so as a Paladin they had no head.
//!
//! So a class fits a character when the character's head has every column
//! the class's own head has, or when a Unique sprite exists for the pair.
//! Read from the game's files (mods included) when the catalog loads.

use std::collections::{HashMap, HashSet};

use super::rom::{lz11, Rom};

#[derive(Default)]
pub struct Sprites {
    /// head folder -> the 32-pixel columns of its sheet that have pixels
    pub heads: HashMap<String, u16>,
    pub bodies: HashSet<String>,
    /// "<class>_<chara>"
    pub uniques: HashSet<String>,
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// the first texture of a BCH: (width, height, format, its pixels' offset)
fn bch_texture(bch: &[u8]) -> Option<(usize, usize, u32, usize)> {
    if bch.get(0..4)? != b"BCH\0" {
        return None;
    }
    let cmds = u32_at(bch, 0x10)? as usize;
    let data = u32_at(bch, 0x14)? as usize;
    let mut p = cmds;
    let (mut w, mut h, mut addr) = (0usize, 0usize, None);
    while p + 8 <= data {
        let param = u32_at(bch, p)?;
        let head = u32_at(bch, p + 4)?;
        let reg = head & 0xFFFF;
        let extra = ((head >> 20) & 0xFF) as usize;
        match reg {
            0x82 => {
                h = (param & 0x7FF) as usize;
                w = ((param >> 16) & 0x7FF) as usize;
            }
            0x85 if w > 0 => addr = Some(param as usize),
            0x8E if addr.is_some() => return Some((w, h, param & 0xF, data + addr?)),
            _ => {}
        }
        p += 8 + extra * 4;
        if p % 8 != 0 {
            p += 4;
        }
    }
    None
}

/// the 32-pixel columns of a sheet with any pixel that is not clear
fn columns(bch: &[u8]) -> Option<u16> {
    let (w, h, fmt, at) = bch_texture(bch)?;
    let bpp = match fmt {
        0 => 4,
        2 | 3 | 4 => 2,
        _ => return Some(0xFFFF), // a format heads don't use: count it whole
    };
    let pixels = bch.get(at..at + w * h * bpp)?;
    let tiles_x = w / 8;
    let mut mask = 0u16;
    for (i, px) in pixels.chunks_exact(bpp).enumerate() {
        let alpha = match fmt {
            0 => px[0] != 0,
            2 => px[0] & 1 != 0,
            3 => true,
            _ => u16::from_le_bytes([px[0], px[1]]) & 0xF != 0,
        };
        if !alpha {
            continue;
        }
        // 8x8 tiles in rows, each in Morton order
        let tile = i / 64;
        let m = i % 64;
        let x = (tile % tiles_x) * 8 + (m & 1 | (m >> 1) & 2 | (m >> 2) & 4);
        mask |= 1 << (x / 32).min(15);
    }
    Some(mask)
}

impl Sprites {
    pub fn load(rom: &Rom) -> Sprites {
        let mut s = Sprites::default();
        for path in rom.list("unit/Head") {
            // unit/Head/<name>/青1.bch.lz
            let Some(rest) = path.strip_prefix("unit/Head/") else { continue };
            let Some((name, file)) = rest.split_once('/') else { continue };
            if file != "青1.bch.lz" {
                continue;
            }
            if let Some(mask) = rom.read(&path).and_then(|f| lz11(&f)).and_then(|f| columns(&f)) {
                s.heads.insert(name.to_string(), mask);
            }
        }
        for path in rom.list("unit/Body") {
            if let Some(name) = path.strip_prefix("unit/Body/").and_then(|r| r.split_once('/')).map(|(n, _)| n) {
                s.bodies.insert(name.to_string());
            }
        }
        for path in rom.list("unit/Unique") {
            if let Some(name) = path.strip_prefix("unit/Unique/").and_then(|r| r.split_once('/')).map(|(n, _)| n) {
                s.uniques.insert(name.to_string());
            }
        }
        log::info!("randomizer: map sprites: {} heads, {} bodies, {} unique", s.heads.len(), s.bodies.len(), s.uniques.len());
        s
    }

    /// whether the map sprite of `chara` (an AID without AID_) in `class` (a
    /// JID without JID_) has a head
    pub fn fits(&self, class: &str, chara: &str) -> bool {
        if self.heads.is_empty() {
            return true; // nothing read: no opinion
        }
        if self.uniques.contains(&format!("{class}_{chara}")) {
            return true;
        }
        if !self.bodies.contains(class) {
            return false;
        }
        let need = self.heads.get(class).copied().unwrap_or(0);
        let have = self.heads.get(chara).copied().unwrap_or(0);
        need & !have == 0
    }
}

/// Shift JIS text of the game's names (katakana, a few kanji and kana,
/// ASCII) as a String; a character outside the names' set becomes '?'
pub fn sjis_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
            continue;
        }
        let Some(&b2) = bytes.get(i + 1) else { break };
        let code = (b as u16) << 8 | b2 as u16;
        i += 2;
        let c = if (0x8340..=0x8396).contains(&code) && code != 0x837F {
            let off = code - 0x8340 - (code > 0x837F) as u16;
            char::from_u32(0x30A1 + off as u32)
        } else if code == 0x815B {
            Some('ー')
        } else {
            SJIS.iter().find(|(_, v)| *v == code).map(|(c, _)| *c)
        };
        out.push(c.unwrap_or('?'));
    }
    out
}

/// the kanji and kana of the game's class, character and model names
/// (from the unit folders, JIDs, PIDs, FIDs and AIDs), with their Shift JIS
/// codes
const SJIS: &[(char, u16)] = &[
    ('い', 0x82A2), ('け', 0x82AF), ('し', 0x82B5), ('た', 0x82BD), ('つ', 0x82C2), ('な', 0x82C8), ('の', 0x82CC),
    ('を', 0x82F0), ('上', 0x8FE3), ('下', 0x89BA), ('不', 0x9573), ('中', 0x9286), ('九', 0x8BE3), ('人', 0x906C),
    ('仏', 0x95A7), ('令', 0x97DF), ('伏', 0x959A), ('伝', 0x9360), ('体', 0x91CC), ('使', 0x8E67), ('侍', 0x8E98),
    ('修', 0x8F43), ('側', 0x91A4), ('兵', 0x95BA), ('内', 0x93E0), ('冶', 0x96E8), ('初', 0x8F89), ('剣', 0x8C95),
    ('右', 0x8945), ('呪', 0x8EF4), ('商', 0x8FA4), ('回', 0x89F1), ('団', 0x9263), ('増', 0x919D), ('壊', 0x89F3),
    ('士', 0x8E6D), ('変', 0x95CF), ('夜', 0x96E9), ('大', 0x91E5), ('天', 0x9356), ('央', 0x899B), ('女', 0x8F97),
    ('妖', 0x9764), ('姫', 0x9550), ('娑', 0x9B4F), ('婆', 0x946B), ('守', 0x8EE7), ('容', 0x9765), ('尾', 0x94F6),
    ('属', 0x91AE), ('山', 0x8E52), ('島', 0x9387), ('巡', 0x8F84), ('左', 0x8DB6), ('巫', 0x9BDE), ('師', 0x8E74),
    ('弓', 0x8B7C), ('強', 0x8BAD), ('形', 0x8C60), ('影', 0x8965), ('忍', 0x9445), ('戦', 0x90ED), ('手', 0x8EE8),
    ('技', 0x8B5A), ('捕', 0x95DF), ('援', 0x8987), ('敵', 0x9347), ('族', 0x91B0), ('明', 0x96BE), ('星', 0x90AF),
    ('暗', 0x88C3), ('期', 0x8AFA), ('村', 0x91BA), ('槍', 0x9184), ('歌', 0x89CC), ('正', 0x90B3), ('武', 0x9590),
    ('死', 0x8E80), ('水', 0x9085), ('法', 0x9640), ('溶', 0x976E), ('無', 0x96B3), ('物', 0x95A8), ('狐', 0x8CCF),
    ('獲', 0x8A6C), ('用', 0x9770), ('男', 0x926A), ('疑', 0x8B5E), ('白', 0x9492), ('盗', 0x9390), ('眷', 0xE1C5),
    ('破', 0x946A), ('神', 0x905F), ('竜', 0x97B3), ('級', 0x8B89), ('絆', 0xE34A), ('絡', 0x978D), ('繰', 0x8C4A),
    ('羅', 0x9785), ('者', 0x8ED2), ('聖', 0x90B9), ('脱', 0x9245), ('自', 0x8EA9), ('落', 0x978E), ('薬', 0x96F2),
    ('虚', 0x8B95), ('血', 0x8C8C), ('術', 0x8F70), ('衛', 0x8971), ('衣', 0x88DF), ('裏', 0x97A0), ('要', 0x9776),
    ('解', 0x89F0), ('警', 0x8C78), ('軍', 0x8C52), ('近', 0x8BDF), ('返', 0x95D4), ('透', 0x93A7), ('部', 0x9594),
    ('金', 0x8BE0), ('鍛', 0x9262), ('鍵', 0x8CAE), ('鎧', 0x8A5A), ('長', 0x92B7), ('闘', 0x93AC), ('防', 0x9668),
    ('陰', 0x8941), ('陽', 0x977A), ('隊', 0x91E0), ('雑', 0x8E47), ('離', 0x97A3), ('面', 0x96CA), ('頭', 0x93AA),
    ('風', 0x9597), ('馬', 0x946E), ('騎', 0x8B52), ('験', 0x8CB1), ('鬼', 0x8B53), ('魔', 0x9682), ('鵄', 0xE9F5),
    ('黒', 0x8D95), ('１', 0x8250), ('２', 0x8251), ('３', 0x8252), ('４', 0x8253), ('５', 0x8254), ('６', 0x8255),
    ('７', 0x8256), ('８', 0x8257), ('９', 0x8258), ('Ｌ', 0x826B), ('＿', 0x8151),
];
