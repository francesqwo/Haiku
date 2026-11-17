//! The 3DS GPU's texture formats, decoded to RGBA (8x8 tiles, Morton order
//! inside a tile, ETC1 in 4x4 blocks), and the textures a BCH file holds.
//! The builder makes Haiku's UI pictures from the game's own textures with
//! this (derive.rs), the way the repository's assets/ui were decoded.

/// bits per pixel of each format, 0 RGBA8 .. 13 ETC1A4.
pub fn bpp(format: u8) -> usize {
    match format {
        0 => 32,
        1 => 24,
        2..=6 => 16,
        7..=9 => 8,
        10..=12 => 4,
        13 => 8,
        _ => 0,
    }
}

/// the bytes a w x h texture of format takes.
pub fn size(w: u32, h: u32, format: u8) -> usize {
    w as usize * h as usize * bpp(format) / 8
}

const ETC1_MODIFIERS: [(i32, i32); 8] = [(2, 8), (5, 17), (9, 29), (13, 42), (18, 60), (24, 80), (33, 106), (47, 183)];

/// a 64-bit ETC1 block, 16 RGB pixels indexed by x * 4 + y.
fn etc1_block(v: u64) -> [[u8; 3]; 16] {
    let flip = (v >> 32) & 1 != 0;
    let tables = [((v >> 37) & 7) as usize, ((v >> 34) & 7) as usize];
    let (c1, c2): ([i32; 3], [i32; 3]) = if (v >> 33) & 1 != 0 {
        let base = [(v >> 59) & 31, (v >> 51) & 31, (v >> 43) & 31].map(|c| c as i32);
        let delta = [(v >> 56) & 7, (v >> 48) & 7, (v >> 40) & 7].map(|d| if d > 3 { d as i32 - 8 } else { d as i32 });
        let c1 = base.map(|c| (c << 3) | (c >> 2));
        let mut c2 = [0; 3];
        for i in 0..3 {
            let c = (base[i] + delta[i]) & 31;
            c2[i] = (c << 3) | (c >> 2);
        }
        (c1, c2)
    } else {
        ([(v >> 60) & 15, (v >> 52) & 15, (v >> 44) & 15].map(|c| c as i32 * 17), [(v >> 56) & 15, (v >> 48) & 15, (v >> 40) & 15].map(|c| c as i32 * 17))
    };
    let mut out = [[0u8; 3]; 16];
    for x in 0..4 {
        for y in 0..4 {
            let i = x * 4 + y;
            let second = if flip { y >= 2 } else { x >= 2 };
            let (a, b) = ETC1_MODIFIERS[tables[second as usize]];
            let pick = (((v >> (i + 16)) & 1) << 1 | ((v >> i) & 1)) as usize;
            let m = [a, b, -a, -b][pick];
            let base = if second { c2 } else { c1 };
            out[i] = base.map(|c| (c + m).clamp(0, 255) as u8);
        }
    }
    out
}

fn expand5(v: u16) -> u8 {
    let v = (v & 31) as u8;
    (v << 3) | (v >> 2)
}

fn expand6(v: u16) -> u8 {
    let v = (v & 63) as u8;
    (v << 2) | (v >> 4)
}

/// decodes a texture to RGBA, rows in the order the texture stores them
/// (the 3DS stores the bottom row first: see flip).
pub fn decode(data: &[u8], w: u32, h: u32, format: u8) -> Option<Vec<u8>> {
    decode_with(data, w, h, format, false)
}

/// decode, with 4-bit values widened by shifting (v << 4, the convention
/// of the tool that decoded Haiku's assets/ui) instead of v * 17.
pub fn decode_with(data: &[u8], w: u32, h: u32, format: u8, shift4: bool) -> Option<Vec<u8>> {
    let n4 = |v: u8| if shift4 { v << 4 } else { v * 17 };
    let (w, h) = (w as usize, h as usize);
    if w % 8 != 0 || h % 8 != 0 || w == 0 || h == 0 || data.len() < size(w as u32, h as u32, format) || bpp(format) == 0 {
        return None;
    }
    let mut px = vec![0u8; w * h * 4];
    let mut put = |x: usize, y: usize, c: [u8; 4]| {
        let at = (y * w + x) * 4;
        px[at..at + 4].copy_from_slice(&c);
    };
    if format == 12 || format == 13 {
        let mut p = 0;
        let word = |at: usize| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
        for ty in (0..h).step_by(8) {
            for tx in (0..w).step_by(8) {
                for (bx, by) in [(0, 0), (4, 0), (0, 4), (4, 4)] {
                    let alpha = if format == 13 {
                        let a = word(p);
                        p += 8;
                        Some(a)
                    } else {
                        None
                    };
                    let rgb = etc1_block(word(p));
                    p += 8;
                    for x in 0..4 {
                        for y in 0..4 {
                            let i = x * 4 + y;
                            let a = alpha.map_or(255, |a| n4(((a >> (i * 4)) & 15) as u8));
                            put(tx + bx + x, ty + by + y, [rgb[i][0], rgb[i][1], rgb[i][2], a]);
                        }
                    }
                }
            }
        }
        return Some(px);
    }
    let bits = bpp(format);
    let mut p = 0usize;
    for ty in (0..h).step_by(8) {
        for tx in (0..w).step_by(8) {
            for i in 0..64usize {
                let (mut x, mut y) = (0, 0);
                for b in 0..3 {
                    x |= ((i >> (2 * b)) & 1) << b;
                    y |= ((i >> (2 * b + 1)) & 1) << b;
                }
                let colour = if bits == 4 {
                    let v = (data[p >> 1] >> ((p & 1) * 4)) & 15;
                    p += 1;
                    let v = n4(v);
                    match format {
                        11 => [255, 255, 255, v],
                        10 => [v, v, v, 255],
                        _ => {
                            // 12 is ETC1, handled above
                            [v, v, v, 255]
                        }
                    }
                } else {
                    let n = bits / 8;
                    let raw = &data[p * n..(p + 1) * n];
                    p += 1;
                    match format {
                        0 => [raw[3], raw[2], raw[1], raw[0]],
                        1 => [raw[2], raw[1], raw[0], 255],
                        2 => {
                            let v = u16::from_le_bytes([raw[0], raw[1]]);
                            [expand5(v >> 11), expand5(v >> 6), expand5(v >> 1), if v & 1 != 0 { 255 } else { 0 }]
                        }
                        3 => {
                            let v = u16::from_le_bytes([raw[0], raw[1]]);
                            [expand5(v >> 11), expand6(v >> 5), expand5(v), 255]
                        }
                        4 => {
                            let v = u16::from_le_bytes([raw[0], raw[1]]);
                            [n4(((v >> 12) & 15) as u8), n4(((v >> 8) & 15) as u8), n4(((v >> 4) & 15) as u8), n4((v & 15) as u8)]
                        }
                        5 => [raw[1], raw[1], raw[1], raw[0]],
                        6 => [raw[1], raw[0], 0, 255],
                        7 => [raw[0], raw[0], raw[0], 255],
                        8 => [255, 255, 255, raw[0]],
                        9 => {
                            let l = n4(raw[0] >> 4);
                            [l, l, l, n4(raw[0] & 15)]
                        }
                        _ => [0, 0, 0, 0],
                    }
                };
                put(tx + x, ty + y, colour);
            }
        }
    }
    Some(px)
}

/// the picture upside down.
pub fn flip(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(px.len());
    for y in (0..h as usize).rev() {
        out.extend_from_slice(&px[y * row..(y + 1) * row]);
    }
    out
}

/// a texture in a file: where its pixels start, its size and format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texture {
    pub offset: usize,
    pub w: u32,
    pub h: u32,
    pub format: u8,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// the textures a BCH holds, read off its GPU commands (texture unit 0's
/// size, address and format registers), as dev/sunglasses/fates.py does.
pub fn bch_textures(bch: &[u8]) -> Vec<Texture> {
    let mut out = Vec::new();
    if bch.get(..4) != Some(b"BCH\0") {
        return out;
    }
    let (Some(cmds_at), Some(data_at)) = (u32_at(bch, 0x10), u32_at(bch, 0x14)) else { return out };
    let (cmds_at, data_at) = (cmds_at as usize, data_at as usize);
    let mut p = cmds_at;
    let (mut w, mut h, mut addr) = (None, None, None);
    while p + 8 <= data_at.min(bch.len()) {
        let param = u32_at(bch, p).unwrap();
        let head = u32_at(bch, p + 4).unwrap();
        let reg = head & 0xFFFF;
        let extra = ((head >> 20) & 0xFF) as usize;
        if reg == 0x82 {
            h = Some(param & 0x7FF);
            w = Some((param >> 16) & 0x7FF);
            addr = None;
        } else if reg == 0x85 && w.is_some() {
            addr = Some(param);
        } else if reg == 0x8E {
            if let (Some(tw), Some(th), Some(a)) = (w, h, addr) {
                let t = Texture { offset: data_at + a as usize, w: tw, h: th, format: (param & 0xF) as u8 };
                if !out.contains(&t) {
                    out.push(t);
                }
            }
            w = None;
            h = None;
            addr = None;
        }
        p += 8 + extra * 4;
        if p % 8 != 0 {
            p += 4;
        }
    }
    out
}

/// the named textures of a version 0x22 BCH (Fates' UI files), with the
/// relocated data pointers, as dev/extract_ui_glyphs.py reads them.
pub fn bch_named_textures(b: &[u8]) -> Vec<(String, Texture)> {
    let mut out = Vec::new();
    let mut read = || -> Option<()> {
        if b.get(..4)? != b"BCH\0" {
            return None;
        }
        let info = u32_at(b, 8)? as usize;
        let strings = u32_at(b, 12)? as usize;
        let commands = u32_at(b, 16)? as usize;
        let raw = u32_at(b, 20)? as usize;
        let extended = u32_at(b, 24)? as usize;
        let relocation = u32_at(b, 28)? as usize;
        let reloc_size = u32_at(b, 0x34)? as usize;
        let count = u32_at(b, info + 0x28)? as usize;
        let table = info + u32_at(b, info + 0x24)? as usize;
        let mut relocations = std::collections::HashMap::new();
        let mut at = relocation;
        while at + 4 <= relocation + reloc_size {
            let value = u32_at(b, at)?;
            let (source, target) = (value >> 29, value >> 25 & 15);
            if source == 2 && (target == 5 || target == 10) {
                relocations.insert(commands + (value & 0x1FF_FFFF) as usize * 4, if target == 5 { raw } else { extended });
            }
            at += 4;
        }
        for i in 0..count.min(4096) {
            let entry = info + u32_at(b, table + i * 4)? as usize;
            let description = commands + u32_at(b, entry)? as usize;
            let name_at = strings + u32_at(b, entry + 0x1C)? as usize;
            let end = b[name_at..].iter().position(|&c| c == 0)? + name_at;
            let name = String::from_utf8_lossy(&b[name_at..end]).into_owned();
            let h = u16::from_le_bytes(b.get(description..description + 2)?.try_into().ok()?) as u32;
            let w = u16::from_le_bytes(b.get(description + 2..description + 4)?.try_into().ok()?) as u32;
            let format = u32_at(b, description + 0x18)? as u8;
            let pointer = description + 0x10;
            let base = relocations.get(&pointer).copied().unwrap_or(raw);
            let offset = base + u32_at(b, pointer)? as usize;
            out.push((name, Texture { offset, w, h, format }));
        }
        Some(())
    };
    let _ = read();
    out
}
