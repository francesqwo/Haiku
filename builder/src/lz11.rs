//! LZ11, in Fates' 0x13 wrapper: the compression of the game's `.lz` files.
//!
//! `compress` is the exact algorithm of dev/mods/make_examples.py's
//! `lz11_compress` (greedy, the last 64 places a 3-byte key was seen, the
//! nearest of the longest), which made Haiku's own `.lz` files, so the
//! files the builder makes are byte for byte the ones in the repository.

use std::collections::HashMap;

/// the plain bytes of an LZ11 stream, with or without the 0x13 wrapper.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut data = data;
    if data.first() == Some(&0x13) && data.len() >= 4 {
        data = &data[4..];
    }
    if data.first() != Some(&0x11) || data.len() < 4 {
        return Err("not LZ11".into());
    }
    let mut size = data[1] as usize | (data[2] as usize) << 8 | (data[3] as usize) << 16;
    let mut pos = 4;
    if size == 0 {
        let word = data.get(4..8).ok_or("truncated LZ11 header")?;
        size = u32::from_le_bytes(word.try_into().unwrap()) as usize;
        pos = 8;
    }
    let byte = |at: usize| data.get(at).copied().ok_or_else(|| "truncated LZ11 stream".to_string());
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let flags = byte(pos)?;
        pos += 1;
        for bit in 0..8 {
            if out.len() >= size {
                break;
            }
            if flags & (0x80 >> bit) == 0 {
                out.push(byte(pos)?);
                pos += 1;
                continue;
            }
            let b = byte(pos)?;
            let (length, disp);
            match b >> 4 {
                0 => {
                    let b1 = byte(pos + 1)?;
                    let b2 = byte(pos + 2)?;
                    length = (((b as usize & 0xF) << 4) | (b1 as usize >> 4)) + 0x11;
                    disp = (((b1 as usize & 0xF) << 8) | b2 as usize) + 1;
                    pos += 3;
                }
                1 => {
                    let b1 = byte(pos + 1)?;
                    let b2 = byte(pos + 2)?;
                    let b3 = byte(pos + 3)?;
                    length = (((b as usize & 0xF) << 12) | ((b1 as usize) << 4) | (b2 as usize >> 4)) + 0x111;
                    disp = (((b2 as usize & 0xF) << 8) | b3 as usize) + 1;
                    pos += 4;
                }
                n => {
                    let b1 = byte(pos + 1)?;
                    length = n as usize + 1;
                    disp = (((b as usize & 0xF) << 8) | b1 as usize) + 1;
                    pos += 2;
                }
            }
            if disp > out.len() {
                return Err("LZ11 reference before the start".into());
            }
            let start = out.len() - disp;
            for i in 0..length {
                let value = out[start + i];
                out.push(value);
            }
        }
    }
    out.truncate(size);
    Ok(out)
}

/// compresses data the way make_examples.py's lz11_compress does.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let size = data.len();
    let mut out = vec![0x11, size as u8, (size >> 8) as u8, (size >> 16) as u8];
    // python's dict of bytes slices: a key is up to 3 bytes (shorter at the
    // very end), so the length is part of it
    let key = |at: usize| -> (u32, u8) {
        let end = (at + 3).min(size);
        let mut value = 0u32;
        for (i, &b) in data[at..end].iter().enumerate() {
            value |= (b as u32) << (8 * i);
        }
        (value, (end - at) as u8)
    };
    let mut index: HashMap<(u32, u8), Vec<usize>> = HashMap::new();
    let mut pos = 0;
    while pos < size {
        let flag_at = out.len();
        out.push(0);
        let mut flags = 0u8;
        for bit in 0..8 {
            if pos >= size {
                break;
            }
            let (mut best_len, mut best_disp) = (0usize, 0usize);
            if let Some(list) = index.get(&key(pos)) {
                let from = list.len().saturating_sub(64);
                for &start in list[from..].iter().rev() {
                    let disp = pos - start;
                    if disp > 0x1000 {
                        continue;
                    }
                    let mut length = 0;
                    while length < 0x10110 && pos + length < size && data[start + length] == data[pos + length] {
                        length += 1;
                    }
                    if length > best_len {
                        best_len = length;
                        best_disp = disp;
                    }
                }
            }
            let step;
            if best_len >= 3 {
                flags |= 0x80 >> bit;
                let d = best_disp - 1;
                if best_len <= 0x10 {
                    out.extend_from_slice(&[((best_len - 1) << 4 | d >> 8) as u8, d as u8]);
                } else if best_len <= 0x110 {
                    let n = best_len - 0x11;
                    out.extend_from_slice(&[(n >> 4) as u8, ((n & 0xF) << 4 | d >> 8) as u8, d as u8]);
                } else {
                    let n = best_len - 0x111;
                    out.extend_from_slice(&[(0x10 | n >> 12) as u8, (n >> 4) as u8, ((n & 0xF) << 4 | d >> 8) as u8, d as u8]);
                }
                step = best_len;
            } else {
                out.push(data[pos]);
                step = 1;
            }
            for i in 0..step {
                index.entry(key(pos + i)).or_default().push(pos + i);
            }
            pos += step;
        }
        out[flag_at] = flags;
    }
    let mut wrapped = vec![0x13, size as u8, (size >> 8) as u8, (size >> 16) as u8];
    wrapped.extend_from_slice(&out);
    wrapped
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 13) as u8).collect();
        assert_eq!(super::decompress(&super::compress(&data)).unwrap(), data);
    }
}
