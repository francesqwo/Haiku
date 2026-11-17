//! randomizer: the save list's mark for a randomized save (rando3).
//!
//! The save list (ChapterMenuItem::SetupParam, overrides/randomizer.c)
//! asks for each slot whether its save is randomized. The answer comes
//! from the save file itself, read from the save folder on the host: its
//! body (after "PMOC", compressed by nn::cx with Huffman or LZ) holds the
//! randomizer's four script values (RZHK, seed, options, check) when the
//! run is randomized. Nothing is written; an ordinary save has no mark.

use std::path::{Path, PathBuf};

fn huffman(data: &[u8]) -> Option<Vec<u8>> {
    let size = data[1] as usize | (data[2] as usize) << 8 | (data[3] as usize) << 16;
    let bits = (data[0] & 0xF) as u32;
    let tree = 4usize;
    let tree_size = (*data.get(4)? as usize + 1) * 2;
    let mut stream = 4 + tree_size;
    let mut out = Vec::with_capacity(size);
    let (mut pending, mut count) = (0u32, 0u32);
    let mut node = tree + 1;
    while out.len() < size {
        let word = u32::from_le_bytes(data.get(stream..stream + 4)?.try_into().ok()?);
        stream += 4;
        for bit in (0..32).rev() {
            let value = *data.get(node)? as usize;
            let offset = value & 0x3F;
            let child = (node & !1) + offset * 2 + 2;
            let right = (word >> bit) & 1;
            let leaf = value & if right == 1 { 0x40 } else { 0x80 };
            node = child + right as usize;
            if leaf != 0 {
                pending |= (*data.get(node)? as u32) << (count * bits);
                count += 1;
                node = tree + 1;
                if count * bits == 8 {
                    out.push(pending as u8);
                    pending = 0;
                    count = 0;
                    if out.len() >= size {
                        break;
                    }
                }
            }
        }
    }
    Some(out)
}

fn lz(data: &[u8]) -> Option<Vec<u8>> {
    let size = data[1] as usize | (data[2] as usize) << 8 | (data[3] as usize) << 16;
    let mut pos = 4;
    let mut out: Vec<u8> = Vec::with_capacity(size);
    while out.len() < size {
        let flags = *data.get(pos)?;
        pos += 1;
        for bit in 0..8 {
            if out.len() >= size {
                break;
            }
            if flags & (0x80 >> bit) == 0 {
                out.push(*data.get(pos)?);
                pos += 1;
                continue;
            }
            let b = *data.get(pos)? as usize;
            let (n, d) = if data[0] == 0x11 {
                match b >> 4 {
                    0 => {
                        let r = ((b & 0xF) << 4 | (*data.get(pos + 1)? as usize) >> 4) + 0x11;
                        let d = ((*data.get(pos + 1)? as usize & 0xF) << 8 | *data.get(pos + 2)? as usize) + 1;
                        pos += 3;
                        (r, d)
                    }
                    1 => {
                        let r = ((b & 0xF) << 12 | (*data.get(pos + 1)? as usize) << 4 | (*data.get(pos + 2)? as usize) >> 4) + 0x111;
                        let d = ((*data.get(pos + 2)? as usize & 0xF) << 8 | *data.get(pos + 3)? as usize) + 1;
                        pos += 4;
                        (r, d)
                    }
                    ind => {
                        let d = ((b & 0xF) << 8 | *data.get(pos + 1)? as usize) + 1;
                        pos += 2;
                        (ind + 1, d)
                    }
                }
            } else {
                let d = ((b & 0xF) << 8 | *data.get(pos + 1)? as usize) + 1;
                pos += 2;
                ((b >> 4) + 3, d)
            };
            if d > out.len() {
                return None;
            }
            for _ in 0..n {
                out.push(out[out.len() - d]);
            }
        }
    }
    Some(out)
}

/// the uncompressed body of a save file
pub fn save_body(raw: &[u8]) -> Option<Vec<u8>> {
    let at = raw.windows(4).position(|w| w == b"PMOC")?;
    let packed = raw.get(at + 16..)?;
    match packed.first()? & 0xF0 {
        0x20 => huffman(packed),
        0x10 => lz(packed),
        _ => Some(packed.to_vec()),
    }
}

/// the run a save file was randomized with: (seed, options, version)
pub fn marker_in(raw: &[u8], magic: u32) -> Option<(u32, u32, u32)> {
    let body = save_body(raw)?;
    let m = magic.to_le_bytes();
    let mut from = 0;
    while let Some(p) = body[from..].windows(4).position(|w| w == m) {
        let at = from + p;
        let word = |k: usize| body.get(at + k..at + k + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        if let (Some(seed), Some(options), Some(check)) = (word(4), word(8), word(12)) {
            if check == magic ^ seed ^ options && (1..=16).contains(&(options >> 16)) {
                return Some((seed, options & 0xFFFF, options >> 16));
            }
        }
        from = at + 1;
    }
    None
}

/// the host files that may hold a game save path: "data:/Chapter0" in the
/// title's save data folder, "ext:/Chapter0.bak" in its extra data folder
/// (where the host keeps two copies, name^ and name~), newest first
pub fn save_files(user_dir: &Path, program_id: u64, game_path: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(name) = game_path.strip_prefix("data:/") {
        if !name.contains("..") {
            out.push(user_dir.join("savedata").join(format!("{program_id:016X}")).join(name));
        }
    } else if let Some(name) = game_path.strip_prefix("ext:/") {
        if !name.contains("..") {
            if let Ok(dirs) = std::fs::read_dir(user_dir.join("extdata")) {
                for d in dirs.flatten() {
                    for suffix in ["", "^", "~"] {
                        let f = d.path().join(format!("{name}{suffix}"));
                        if f.is_file() {
                            out.push(f);
                        }
                    }
                }
            }
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(std::fs::metadata(f).and_then(|m| m.modified()).ok()));
    out
}
