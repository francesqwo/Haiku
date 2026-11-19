//! randomizer: reading the game's own files from the RomFS the game reads
//! (with any mods over it), and the two container formats the randomizer
//! needs: Fates' LZ11 compression and its "bin" data files.
//!
//! Nothing here writes: the installed data folder is never changed.

use std::collections::BTreeMap;
use std::sync::Arc;

/// the title's RomFS, level 3 (the directory tree and the files' bytes).
pub struct Rom {
    layered: Option<Arc<zakuro_fs::layered::Layered>>,
    image: Arc<zakuro_fs::RomImage>,
    /// where level 3 starts in the image, when there is no layered view
    base: u64,
    dir_meta: Vec<u8>,
    file_meta: Vec<u8>,
    file_data: u64,
}

const INVALID: u32 = 0xFFFF_FFFF;

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u32_at(data, at)? as u64 | (u32_at(data, at + 4)? as u64) << 32)
}

fn name_at(data: &[u8], at: usize, len: usize) -> String {
    let units: Vec<u16> = data
        .get(at..at + len)
        .unwrap_or(&[])
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

impl Rom {
    /// the RomFS of the title the system runs.
    pub fn of(title: &zakuro_fs::Title) -> Option<Rom> {
        let image = title.shared_image();
        let layered = title.layered.clone();
        let base = match (&layered, &title.romfs) {
            (Some(_), _) => 0,
            (None, Some(romfs)) => romfs.base,
            (None, None) => return None,
        };
        Rom::from_parts(layered, image, base)
    }

    /// jpn2: the RomFS of one DLC content (a folder installed by Setup, or
    /// a content of a DLC kept as a CIA). The Japanese Special Edition keeps
    /// Revelation's chapters (GameData/Person/C...) there, under local/.
    pub fn of_dlc(dlc: &zakuro_fs::Dlc, index: u16) -> Option<Rom> {
        match dlc.folder(index) {
            Some(layered) => Rom::from_parts(Some(layered), dlc.image(), 0),
            None => {
                let romfs = dlc.content_romfs(index)?;
                Rom::from_parts(None, dlc.image(), romfs.base)
            }
        }
    }

    fn from_parts(layered: Option<Arc<zakuro_fs::layered::Layered>>, image: Arc<zakuro_fs::RomImage>, base: u64) -> Option<Rom> {
        let mut rom = Rom { layered, image, base, dir_meta: Vec::new(), file_meta: Vec::new(), file_data: 0 };
        let mut header = [0u8; 0x28];
        if rom.read_at(0, &mut header) != header.len() || u32_at(&header, 0)? != 0x28 {
            return None;
        }
        let (dir_off, dir_size) = (u32_at(&header, 0x0C)? as u64, u32_at(&header, 0x10)? as usize);
        let (file_off, file_size) = (u32_at(&header, 0x1C)? as u64, u32_at(&header, 0x20)? as usize);
        rom.file_data = u32_at(&header, 0x24)? as u64;
        let mut dir_meta = vec![0u8; dir_size];
        let mut file_meta = vec![0u8; file_size];
        if rom.read_at(dir_off, &mut dir_meta) != dir_size || rom.read_at(file_off, &mut file_meta) != file_size {
            return None;
        }
        rom.dir_meta = dir_meta;
        rom.file_meta = file_meta;
        Some(rom)
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> usize {
        match &self.layered {
            Some(layered) => layered.read(self.image.data(), offset, out),
            None => {
                let data = self.image.data();
                let start = (self.base + offset) as usize;
                if start >= data.len() {
                    return 0;
                }
                let count = out.len().min(data.len() - start);
                out[..count].copy_from_slice(&data[start..start + count]);
                count
            }
        }
    }

    /// (first child dir, first file, name) of the directory at offset.
    fn dir(&self, at: u32) -> Option<(u32, u32, u32, String)> {
        let at = at as usize;
        let sibling = u32_at(&self.dir_meta, at + 4)?;
        let child = u32_at(&self.dir_meta, at + 8)?;
        let file = u32_at(&self.dir_meta, at + 12)?;
        let len = u32_at(&self.dir_meta, at + 20)? as usize;
        Some((sibling, child, file, name_at(&self.dir_meta, at + 24, len)))
    }

    /// (sibling, data offset, size, name) of the file at offset.
    fn file(&self, at: u32) -> Option<(u32, u64, u64, String)> {
        let at = at as usize;
        let sibling = u32_at(&self.file_meta, at + 4)?;
        let offset = u64_at(&self.file_meta, at + 8)?;
        let size = u64_at(&self.file_meta, at + 16)?;
        let len = u32_at(&self.file_meta, at + 28)? as usize;
        Some((sibling, offset, size, name_at(&self.file_meta, at + 32, len)))
    }

    /// the directory a /-separated path names, from the root.
    fn find_dir(&self, path: &str) -> Option<u32> {
        let mut dir = 0u32;
        // a level-3 image's tree may hang off a nameless directory
        let (_, mut child, _, _) = self.dir(0)?;
        while child != INVALID {
            let (sibling, _, _, name) = self.dir(child)?;
            if name.is_empty() {
                dir = child;
                break;
            }
            child = sibling;
        }
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let (_, mut child, _, _) = self.dir(dir)?;
            let mut found = None;
            while child != INVALID {
                let (sibling, _, _, name) = self.dir(child)?;
                if name.eq_ignore_ascii_case(part) {
                    found = Some(child);
                    break;
                }
                child = sibling;
            }
            dir = found?;
        }
        Some(dir)
    }

    /// (offset, size) of a file's bytes.
    fn locate(&self, path: &str) -> Option<(u64, u64)> {
        let (dir, name) = match path.rfind('/') {
            Some(at) => (&path[..at], &path[at + 1..]),
            None => ("", path),
        };
        let dir = self.find_dir(dir)?;
        let (_, _, mut file, _) = self.dir(dir)?;
        while file != INVALID {
            let (sibling, offset, size, file_name) = self.file(file)?;
            if file_name.eq_ignore_ascii_case(name) {
                return Some((self.file_data + offset, size));
            }
            file = sibling;
        }
        None
    }

    /// a whole file.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let (offset, size) = self.locate(path)?;
        let mut data = vec![0u8; size as usize];
        (self.read_at(offset, &mut data) == data.len()).then_some(data)
    }

    /// part of a file.
    pub fn read_part(&self, path: &str, start: u64, len: usize) -> Option<Vec<u8>> {
        let (offset, size) = self.locate(path)?;
        if start >= size {
            return None;
        }
        let mut data = vec![0u8; len.min((size - start) as usize)];
        (self.read_at(offset + start, &mut data) == data.len()).then_some(data)
    }

    /// the files of a directory and of the directories in it, as paths.
    pub fn list(&self, path: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(dir) = self.find_dir(path) {
            self.walk(dir, path.trim_end_matches('/').to_string(), &mut out);
        }
        out
    }

    fn walk(&self, dir: u32, prefix: String, out: &mut Vec<String>) {
        let Some((_, mut child, mut file, _)) = self.dir(dir) else { return };
        while file != INVALID {
            let Some((sibling, _, _, name)) = self.file(file) else { break };
            out.push(format!("{prefix}/{name}"));
            file = sibling;
        }
        while child != INVALID {
            let Some((sibling, _, _, name)) = self.dir(child) else { break };
            self.walk(child, format!("{prefix}/{name}"), out);
            child = sibling;
        }
    }
}

/// Fates' LZ11, inside its 0x13 wrapper or bare.
pub fn lz11(data: &[u8]) -> Option<Vec<u8>> {
    let data = if data.first() == Some(&0x13) { data.get(4..)? } else { data };
    if data.first() != Some(&0x11) {
        return None;
    }
    let mut size = data[1] as usize | (data[2] as usize) << 8 | (data[3] as usize) << 16;
    let mut at = 4;
    if size == 0 {
        size = u32_at(data, 4)? as usize;
        at = 8;
    }
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let flags = *data.get(at)?;
        at += 1;
        for bit in 0..8 {
            if out.len() >= size {
                break;
            }
            if flags & (0x80 >> bit) == 0 {
                out.push(*data.get(at)?);
                at += 1;
                continue;
            }
            let b = *data.get(at)? as usize;
            let (len, disp) = match b >> 4 {
                0 => {
                    let b1 = *data.get(at + 1)? as usize;
                    let b2 = *data.get(at + 2)? as usize;
                    at += 3;
                    ((((b & 0xF) << 4) | (b1 >> 4)) + 0x11, (((b1 & 0xF) << 8) | b2) + 1)
                }
                1 => {
                    let b1 = *data.get(at + 1)? as usize;
                    let b2 = *data.get(at + 2)? as usize;
                    let b3 = *data.get(at + 3)? as usize;
                    at += 4;
                    ((((b & 0xF) << 12) | (b1 << 4) | (b2 >> 4)) + 0x111, (((b2 & 0xF) << 8) | b3) + 1)
                }
                n => {
                    let b1 = *data.get(at + 1)? as usize;
                    at += 2;
                    (n + 1, (((b & 0xF) << 8) | b1) + 1)
                }
            };
            if disp > out.len() {
                return None;
            }
            for _ in 0..len {
                let byte = out[out.len() - disp];
                out.push(byte);
            }
        }
    }
    out.truncate(size);
    Some(out)
}

/// a Fates "bin" data file: the data (offsets in it are from its start),
/// which of its words are pointers, and its labels.
pub struct Bin {
    pub data: Vec<u8>,
    /// offsets of the words that hold pointers, which the game turns into
    /// addresses when it loads the file
    pub pointers: std::collections::HashSet<u32>,
    pub labels: BTreeMap<Vec<u8>, u32>,
}

impl Bin {
    pub fn parse(file: &[u8]) -> Option<Bin> {
        let data_size = u32_at(file, 4)? as usize;
        let pointer_count = u32_at(file, 8)? as usize;
        let label_count = u32_at(file, 12)? as usize;
        // offsets are from 0x20; the strings follow the tables at the end
        // of the file, so the "data" is everything after the header
        let data = file.get(0x20..)?.to_vec();
        if data.len() < data_size {
            return None;
        }
        let table = 0x20 + data_size;
        let mut pointers = std::collections::HashSet::new();
        for i in 0..pointer_count {
            pointers.insert(u32_at(file, table + 4 * i)?);
        }
        let labels_at = table + 4 * pointer_count;
        let names = labels_at + 8 * label_count;
        let mut labels = BTreeMap::new();
        for i in 0..label_count {
            let offset = u32_at(file, labels_at + 8 * i)?;
            let name = names + u32_at(file, labels_at + 8 * i + 4)? as usize;
            let end = file.get(name..)?.iter().position(|&b| b == 0)? + name;
            labels.insert(file[name..end].to_vec(), offset);
        }
        Some(Bin { data, pointers, labels })
    }

    pub fn u8(&self, at: u32) -> u8 {
        self.data.get(at as usize).copied().unwrap_or(0)
    }

    pub fn u16(&self, at: u32) -> u16 {
        let at = at as usize;
        self.data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, at: u32) -> u32 {
        u32_at(&self.data, at as usize).unwrap_or(0)
    }

    /// the pointer word at at, as an offset in the data (None for null).
    pub fn ptr(&self, at: u32) -> Option<u32> {
        self.pointers.contains(&at).then(|| self.u32(at))
    }

    /// the NUL-terminated bytes at an offset (Shift JIS for the game's
    /// names).
    pub fn cstr(&self, at: u32) -> Vec<u8> {
        let at = at as usize;
        let tail = self.data.get(at..).unwrap_or(&[]);
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        tail[..end].to_vec()
    }

    /// the string a pointer field points at.
    pub fn str_field(&self, at: u32) -> Option<Vec<u8>> {
        self.ptr(at).map(|p| self.cstr(p))
    }

    pub fn bytes(&self, at: u32, len: usize) -> &[u8] {
        let at = at as usize;
        self.data.get(at..at + len).unwrap_or(&[])
    }
}

/// a message archive: label -> text.
pub fn messages(file: &[u8]) -> BTreeMap<Vec<u8>, String> {
    let mut out = BTreeMap::new();
    let (Some(data_size), Some(pointers), Some(labels)) = (u32_at(file, 4), u32_at(file, 8), u32_at(file, 12)) else {
        return out;
    };
    let table = 0x20 + data_size as usize + 4 * pointers as usize;
    let names = table + 8 * labels as usize;
    for i in 0..labels as usize {
        let (Some(text), Some(name)) = (u32_at(file, table + 8 * i), u32_at(file, table + 8 * i + 4)) else { break };
        let name = names + name as usize;
        let Some(end) = file.get(name..).and_then(|t| t.iter().position(|&b| b == 0)) else { break };
        let label = file[name..name + end].to_vec();
        let mut units = Vec::new();
        let mut at = 0x20 + text as usize;
        while let Some(b) = file.get(at..at + 2) {
            let unit = u16::from_le_bytes([b[0], b[1]]);
            if unit == 0 {
                break;
            }
            units.push(unit);
            at += 2;
        }
        out.insert(label, String::from_utf16_lossy(&units));
    }
    out
}

/// the names of the sound archive's VOICE_ sounds, from its string block.
pub fn voice_labels(rom: &Rom, path: &str) -> Vec<String> {
    let Some(header) = rom.read_part(path, 0, 0x40) else { return Vec::new() };
    if header.get(0..4) != Some(b"CSAR") {
        return Vec::new();
    }
    let blocks = u16::from_le_bytes([header[0x10], header[0x11]]) as usize;
    let mut out = Vec::new();
    for i in 0..blocks.min(3) {
        let at = 0x14 + 12 * i;
        let (Some(tag), Some(offset), Some(size)) = (u32_at(&header, at), u32_at(&header, at + 4), u32_at(&header, at + 8)) else { break };
        if tag & 0xFFFF != 0x2000 {
            continue;
        }
        let Some(block) = rom.read_part(path, offset as u64, size as usize) else { break };
        if block.get(0..4) != Some(b"STRG") {
            break;
        }
        let Some(table) = u32_at(&block, 12).map(|o| 8 + o as usize) else { break };
        let count = u32_at(&block, table).unwrap_or(0) as usize;
        for n in 0..count {
            let entry = table + 4 + 12 * n;
            let (Some(off), Some(len)) = (u32_at(&block, entry + 4), u32_at(&block, entry + 8)) else { break };
            let start = table + off as usize;
            if let Some(name) = block.get(start..start + (len as usize).saturating_sub(1)) {
                if name.starts_with(b"VOICE_") {
                    out.push(String::from_utf8_lossy(name).into_owned());
                }
            }
        }
    }
    out
}
