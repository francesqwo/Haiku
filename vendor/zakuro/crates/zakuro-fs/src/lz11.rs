//! fanpatch: LZ11, in Fates' 0x13 wrapper: the compression of the game's
//! `.lz` files (message archives, GameData, models...).
//!
//! The same code as Haiku Setup's `builder/src/lz11.rs` (whose `compress`
//! is dev/mods/make_examples.py's algorithm, byte for byte), here so the
//! engine can rebuild a message archive at boot (haiku_fanpatch) and Haiku
//! Setup can share it through zakuro-fs.

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
        size = u32::from_le_bytes([word[0], word[1], word[2], word[3]]) as usize;
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

/// compresses data as Haiku's own `.lz` files are (greedy, the last 64
/// places a 3-byte key was seen, the nearest of the longest), in the 0x13
/// wrapper the game's files have.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let size = data.len();
    let mut out = vec![0x11, size as u8, (size >> 8) as u8, (size >> 16) as u8];
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

/// compresses data in the same format as [`compress`] (greedy, the nearest
/// of the longest of the last 64 matches), with hash chains in arrays
/// rather than a map of lists: many times faster, for files made at run
/// time (haiku_fanpatch's merged text). The bytes can differ from
/// `compress`'s; the game reads both alike.
pub fn compress_fast(data: &[u8]) -> Vec<u8> {
    const HASH_BITS: u32 = 15;
    const WINDOW: usize = 0x1000;
    const MAX_LEN: usize = 0x10110;
    const CHAIN: usize = 64;
    const NONE: u32 = u32::MAX;
    let size = data.len();
    let mut out = Vec::with_capacity(size / 2 + 16);
    out.extend_from_slice(&[0x13, size as u8, (size >> 8) as u8, (size >> 16) as u8]);
    out.extend_from_slice(&[0x11, size as u8, (size >> 8) as u8, (size >> 16) as u8]);
    let hash = |at: usize| -> usize {
        let v = data[at] as u32 | (data[at + 1] as u32) << 8 | (data[at + 2] as u32) << 16;
        (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
    };
    let mut head = vec![NONE; 1 << HASH_BITS];
    let mut prev = vec![NONE; size];
    let insert = |at: usize, head: &mut Vec<u32>, prev: &mut Vec<u32>| {
        if at + 3 <= size {
            let h = hash(at);
            prev[at] = head[h];
            head[h] = at as u32;
        }
    };
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
            if pos + 3 <= size {
                let mut candidate = head[hash(pos)];
                let mut steps = 0;
                while candidate != NONE && steps < CHAIN {
                    let start = candidate as usize;
                    let disp = pos - start;
                    if disp > WINDOW {
                        break;
                    }
                    let mut length = 0;
                    while length < MAX_LEN && pos + length < size && data[start + length] == data[pos + length] {
                        length += 1;
                    }
                    if length > best_len {
                        best_len = length;
                        best_disp = disp;
                    }
                    candidate = prev[start];
                    steps += 1;
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
                insert(pos + i, &mut head, &mut prev);
            }
            pos += step;
        }
        out[flag_at] = flags;
    }
    out
}

/// tones: the stream made exactly `target` bytes long without junk after
/// it: zero bytes are added to the end of what it decompresses to, as
/// literals inside the stream (its size headers grown to match), with at
/// most one zero byte left after the stream when a new flag byte would
/// overshoot. The game's own loader does not take kindly to a long run of
/// bytes after an LZ11 stream (Haiku's alternates of one file have to be
/// the same size: zakuro_fs::layered::ALTERNATE). None when the stream is
/// longer than target or not one this reads (a 3-byte size header, 0x11,
/// or Fates' 0x13 wrapper around one).
pub fn pad_to(stream: &[u8], target: usize) -> Option<Vec<u8>> {
    let wrapped = stream.first() == Some(&0x13);
    let start = if wrapped { 4 } else { 0 };
    if stream.get(start) != Some(&0x11) || stream.len() < start + 4 {
        return None;
    }
    let size = stream[start + 1] as usize | (stream[start + 2] as usize) << 8 | (stream[start + 3] as usize) << 16;
    if size == 0 {
        return None;
    }
    // walk the tokens: where the stream ends and how full its last flag
    // group is
    let mut pos = start + 4;
    let mut out = 0usize;
    let mut last_flag_used = 8usize;
    while out < size {
        pos += 1;
        last_flag_used = 0;
        let flags = *stream.get(pos - 1)?;
        for bit in 0..8 {
            if out >= size {
                break;
            }
            last_flag_used += 1;
            if flags & (0x80 >> bit) == 0 {
                out += 1;
                pos += 1;
                continue;
            }
            let b = *stream.get(pos)? as usize;
            match b >> 4 {
                0 => {
                    out += ((b & 0xF) << 4 | *stream.get(pos + 1)? as usize >> 4) + 0x11;
                    pos += 3;
                }
                1 => {
                    out += ((b & 0xF) << 12 | (*stream.get(pos + 1)? as usize) << 4 | *stream.get(pos + 2)? as usize >> 4) + 0x111;
                    pos += 4;
                }
                n => {
                    out += n + 1;
                    pos += 2;
                }
            }
        }
    }
    if pos > stream.len() || pos > target {
        return None;
    }
    let mut padded = stream[..pos].to_vec();
    let mut added = 0usize;
    // literal zeros: one byte each while the last flag group has room, a
    // new flag byte (all literals) and one byte otherwise
    while padded.len() < target {
        if last_flag_used < 8 {
            padded.push(0);
            last_flag_used += 1;
        } else if padded.len() + 2 <= target {
            padded.extend_from_slice(&[0, 0]);
            last_flag_used = 1;
        } else {
            break;
        }
        added += 1;
    }
    let new_size = size + added;
    if new_size > 0xFF_FFFF {
        return None;
    }
    for at in [start].into_iter().chain(wrapped.then_some(0)) {
        padded[at + 1] = new_size as u8;
        padded[at + 2] = (new_size >> 8) as u8;
        padded[at + 3] = (new_size >> 16) as u8;
    }
    padded.resize(target, 0);
    Some(padded)
}

#[cfg(test)]
mod tests {
    /// tones: a stream padded to a length decompresses to the same bytes
    /// and zeros, with at most one byte after it.
    #[test]
    fn padded_streams_keep_their_data() {
        let text: Vec<u8> = "the quick brown fox jumps over the lazy dog; ".repeat(40).bytes().chain((0..300u32).map(|i| (i * 31 % 251) as u8)).collect();
        for packed in [super::compress_fast(&text), super::compress(&text)] {
            for extra in 0..40 {
                let target = packed.len() + extra;
                let padded = super::pad_to(&packed, target).unwrap();
                assert_eq!(padded.len(), target);
                let out = super::decompress(&padded).unwrap();
                assert_eq!(&out[..text.len()], &text[..], "extra {extra}");
                assert!(out[text.len()..].iter().all(|&b| b == 0));
                assert!(out.len() - text.len() + 1 >= extra.saturating_sub(extra / 9 + 2), "most of the room is data");
            }
            assert_eq!(super::pad_to(&packed, packed.len() - 1), None);
        }
    }

    #[test]
    fn round_trip() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 13) as u8).collect();
        assert_eq!(super::decompress(&super::compress(&data)).unwrap(), data);
        assert_eq!(super::decompress(&super::compress_fast(&data)).unwrap(), data);
        let text: Vec<u8> = "the quick brown fox jumps over the lazy dog; ".repeat(400).bytes().chain((0..3000u32).map(|i| (i * 31 % 251) as u8)).collect();
        let fast = super::compress_fast(&text);
        assert_eq!(super::decompress(&fast).unwrap(), text);
        assert!(fast.len() < text.len() / 2, "{} of {}", fast.len(), text.len());
        assert_eq!(super::decompress(&super::compress_fast(b"ab")).unwrap(), b"ab");
    }
}
