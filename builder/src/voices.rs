//! jpvoice: the optional Japanese voices. The player gives Haiku Setup a
//! dump with Japanese voices: an EUR or USA "undub" (the Western game with
//! the Japanese sound files put in), or a Japanese Fire Emblem if
//! (CTR-P-BFZJ, 000400000012DE00, or another if cartridge). Haiku Setup
//! takes only the voice and sound files out of it into `data/voices-ja/`,
//! which Haiku lays over the game's files when Voices is Japanese
//! (vendor/zakuro/crates/zakuro/src/haiku_voices.rs). Nothing else of that
//! dump is used: not its code, text, tables or graphics (an undub often has
//! a fan patch in it as well), and the installed game is never written.
//!
//! What is taken (dev/reports/jpvoice-survey.md):
//! - `sound/IRON15_sound.bcsar`, the sound archive (labels, the battle
//!   shouts' wave archives), and `sound/IndirectSound.bin.lz`, its table
//!   (with the aliases that give Kana the Japanese lines): always, whole;
//! - `sound/stream/*` that differ from the installed game's: the voice
//!   clips (.aac), the movie soundtracks and Azura's song (.bcstm). A stream
//!   that differs only in its header's version byte is the same audio and is
//!   left out;
//! - `movie/**/*_bgm.bin` that differ: the movies' sound cues, which name
//!   the Japanese cue streams.
//!
//! Checks: what the dump is (a Fates title, decrypted or with the fixed
//! key, which is the public all-zero key for a game: no console key is ever
//! needed), its whole RomFS against its IVFC hash tree, and that its sound
//! archive really has Japanese voices: not the installed one, the Japanese
//! game's labels (avatar voices 4 and 5), and every voice label of the
//! installed game's but Kana's (which the Japanese table maps) and at most
//! 2% more. Every file written is read back against the SHA-256 taken from
//! the dump. `voices.txt` (written last) records the source.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::aes::{ctr_xor, Aes128};
use crate::dump::{self, Problem, DECRYPT_HINT, MEDIA};

pub const FOLDER: &str = "voices-ja";
pub const MANIFEST: &str = "voices.txt";
const ARCHIVE: &str = "sound/IRON15_sound.bcsar";
const TABLE: &str = "sound/IndirectSound.bin.lz";
/// the Japanese Special Edition, for messages.
pub const JP_SE_TITLE: u64 = 0x0004_0000_0012_DE00;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn short(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

// ---- where the RomFS is ------------------------------------------------------

/// how the RomFS's bytes are stored.
#[derive(Clone)]
enum Crypt {
    Plain,
    /// the fixed (zero) key, AES-CTR from this counter at the RomFS's start.
    Fixed([u8; 16]),
}

/// a RomFS in a dump file, or an extracted RomFS folder.
#[derive(Clone)]
enum Origin {
    Image { path: PathBuf, romfs_offset: u64, romfs_size: u64, crypt: Crypt },
    Folder(PathBuf),
}

/// a dump with (perhaps) Japanese voices, checked as far as its headers go.
#[derive(Clone)]
pub struct VoiceDump {
    pub path: PathBuf,
    /// "cartridge image (.3ds)", "CIA", "CXI", "RomFS folder".
    pub kind: &'static str,
    pub title: u64,
    pub product: String,
    /// "decrypted", "fixed key" or "a folder".
    pub crypto: &'static str,
    origin: Origin,
}

impl VoiceDump {
    pub fn describe(&self) -> String {
        let what = match self.product.chars().last() {
            _ if self.kind == "RomFS folder" => "an extracted RomFS".to_owned(),
            _ if self.title == JP_SE_TITLE => format!("Fire Emblem if Special Edition (Japan, {})", self.product),
            Some('J') => format!("Fire Emblem if (Japan, {})", self.product),
            Some('P') => format!("Fire Emblem Fates (Europe, {})", self.product),
            Some('E') => format!("Fire Emblem Fates (USA, {})", self.product),
            _ => format!("{} ({})", self.product, self.kind),
        };
        format!("{what}, title {:016X}, {}, {}", self.title, self.kind, self.crypto)
    }
}

struct Reader {
    file: File,
    len: u64,
}

impl Reader {
    fn open(path: &Path) -> Result<Reader, Problem> {
        let file = File::open(path).map_err(|e| Problem::Unreadable(format!("The file can't be opened ({e}).")))?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Reader { file, len })
    }
    fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, Problem> {
        if offset.checked_add(len as u64).is_none_or(|end| end > self.len) {
            return Err(Problem::Corrupt("The file is shorter than its own header says: it is an incomplete copy or download.".into()));
        }
        let mut bytes = vec![0; len];
        self.file.seek(SeekFrom::Start(offset)).map_err(|e| Problem::Unreadable(format!("The file can't be read ({e}).")))?;
        self.file.read_exact(&mut bytes).map_err(|e| Problem::Unreadable(format!("The file can't be read ({e}).")))?;
        Ok(bytes)
    }
}

/// reads a RomFS's bytes, decrypting when it is stored with the fixed key.
struct RomfsReader {
    file: Reader,
    offset: u64,
    size: u64,
    crypt: Crypt,
    aes: Aes128,
}

impl RomfsReader {
    fn new(path: &Path, offset: u64, size: u64, crypt: Crypt) -> Result<RomfsReader, Problem> {
        Ok(RomfsReader { file: Reader::open(path)?, offset, size, crypt, aes: Aes128::new(&[0; 16]) })
    }
    /// len bytes from at, counted from the RomFS's start.
    fn read(&mut self, at: u64, len: usize) -> Result<Vec<u8>, Problem> {
        if at + len as u64 > self.size {
            return Err(Problem::Corrupt("The game's data (RomFS) is cut short: the dump is damaged or incomplete.".into()));
        }
        match &self.crypt {
            Crypt::Plain => self.file.read(self.offset + at, len),
            Crypt::Fixed(counter) => {
                let start = at & !15;
                let pad = (at - start) as usize;
                let mut bytes = self.file.read(self.offset + start, (pad + len).next_multiple_of(16).min((self.size - start) as usize))?;
                ctr_xor(&self.aes, counter, start / 16, &mut bytes);
                bytes.drain(..pad);
                bytes.truncate(len);
                Ok(bytes)
            }
        }
    }
}

/// the NCCH counter for a section (3dbrew "NCCH"): version 0 and 2, the
/// partition id big-endian, the section's type, zeros; version 1, the
/// partition id little-endian, zeros, the section's offset big-endian.
fn ncch_counter(header: &[u8], section_type: u8, section_offset: u64) -> [u8; 16] {
    let partition = u64_at(header, 0x108);
    let version = u16_at(header, 0x112);
    let mut counter = [0u8; 16];
    if version == 1 {
        counter[..8].copy_from_slice(&partition.to_le_bytes());
        counter[12..].copy_from_slice(&(section_offset as u32).to_be_bytes());
    } else {
        counter[..8].copy_from_slice(&partition.to_be_bytes());
        counter[8] = section_type;
    }
    counter
}

/// the executable content's offset in a CIA (title-key encrypted is
/// refused: that needs the console's keys).
fn cia_content(reader: &mut Reader) -> Result<u64, Problem> {
    let header = reader.read(0, 0x2020)?;
    let align = |n: u64| n.next_multiple_of(0x40);
    let tmd_offset = align(align(align(0x2020) + u32_at(&header, 0x08) as u64) + u32_at(&header, 0x0C) as u64);
    let tmd_size = u32_at(&header, 0x10) as usize;
    let tmd = reader.read(tmd_offset, tmd_size)?;
    let (_, _, contents) = dump::tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The CIA's title metadata can't be read: the file is damaged.".into()))?;
    let mut offset = align(tmd_offset + tmd_size as u64);
    for (index, _, flags, size, _) in contents {
        if header[0x20 + index as usize / 8] & (0x80 >> (index % 8)) == 0 {
            continue;
        }
        if index == 0 {
            if flags & 1 != 0 {
                return Err(Problem::Encrypted(format!("This CIA is encrypted (the eShop's title key). {DECRYPT_HINT}")));
            }
            return Ok(offset);
        }
        offset = align(offset + size);
    }
    Err(Problem::Corrupt("The CIA has no game in it.".into()))
}

/// whether a title is a Fire Emblem Fates (if) cartridge or download, by its
/// product code; a problem in words when it is not.
fn judge(program_id: u64, product: &str) -> Result<(), Problem> {
    match program_id >> 32 {
        0x0004_000E => return Err(Problem::WrongKind("This is an update, not a game. Select the game with the Japanese voices itself.".into())),
        0x0004_008C => return Err(Problem::WrongKind("This is downloadable content. Select the game with the Japanese voices itself.".into())),
        _ => {}
    }
    if product.starts_with("CTR-P-BF") || product.starts_with("CTR-N-BF") {
        return Ok(());
    }
    Err(Problem::WrongGame(format!(
        "This is a different game ({}, title {program_id:016X}), not Fire Emblem Fates or Fire Emblem if.",
        if product.trim().is_empty() { "no product code" } else { product }
    )))
}

/// a folder: an extracted RomFS (with sound/ in it, or romfs/sound/), an
/// emulator's sdmc with a Fates title installed, or a folder with one dump.
fn find_in_folder(dir: &Path) -> Result<Result<PathBuf, PathBuf>, Problem> {
    for romfs in [dir.to_path_buf(), dir.join("romfs")] {
        if romfs.join(ARCHIVE).is_file() {
            return Ok(Err(romfs));
        }
    }
    let mut apps = Vec::new();
    let mut dumps = Vec::new();
    dump::find_files(dir, 0, &mut |path| {
        let lower = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        // the Japanese Special Edition and the other if cartridges' downloads
        // are all 00040000/0012xxxx or 0017xxxx; any game content is checked
        if ext == "app" && lower.contains("/00040000/") && lower.contains("/content") {
            apps.push(path.to_path_buf());
        } else if ["3ds", "cci", "cia", "cxi"].contains(&ext.as_str()) {
            dumps.push(path.to_path_buf());
        }
    });
    apps.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    let fitting: Vec<PathBuf> = apps.into_iter().chain(dumps).filter(|p| check(p).is_ok()).collect();
    match fitting.as_slice() {
        [] => Err(Problem::NotADump("This folder doesn't hold a game with Japanese voices. Select the dump file itself (.3ds, .cci, .cia or .cxi), an emulator's sdmc folder with it installed, or a folder with its extracted RomFS.".into())),
        [one, ..] => Ok(Ok(one.clone())),
    }
}

/// what the file or folder at path is, as far as its headers tell (the
/// voices themselves are checked by extract, against the installed game).
pub fn check(path: &Path) -> Result<VoiceDump, Problem> {
    let path = if path.is_dir() {
        match find_in_folder(path)? {
            Ok(file) => file,
            Err(romfs) => {
                return Ok(VoiceDump {
                    path: romfs.clone(),
                    kind: "RomFS folder",
                    title: 0,
                    product: String::new(),
                    crypto: "a folder",
                    origin: Origin::Folder(romfs),
                })
            }
        }
    } else {
        path.to_path_buf()
    };
    let mut reader = Reader::open(&path)?;
    if reader.len < 0x4000 {
        return Err(Problem::NotADump("This file is far too small to be a 3DS game.".into()));
    }
    let head = reader.read(0, 0x200)?;
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"7z\xBC\xAF\x27\x1C") || head.starts_with(b"Rar!") {
        return Err(Problem::NotADump("This is an archive. Extract the dump from it first, then select the .3ds or .cia inside.".into()));
    }
    let (kind, ncch) = if &head[0x100..0x104] == b"NCSD" {
        ("cartridge image (.3ds)", u32_at(&head, 0x120) as u64 * MEDIA)
    } else if &head[0x100..0x104] == b"NCCH" {
        ("executable (.cxi)", 0)
    } else if u32_at(&head, 0) == 0x2020 {
        ("CIA", cia_content(&mut reader)?)
    } else {
        return Err(Problem::NotADump("This is not a 3DS game dump. Select a decrypted .3ds (.cci), .cia or .cxi file.".into()));
    };
    let header = reader.read(ncch, 0x200)?;
    if &header[0x100..0x104] != b"NCCH" {
        return Err(Problem::Corrupt("The game's partition has no valid header. The dump is damaged or incomplete.".into()));
    }
    let program_id = u64_at(&header, 0x118);
    let product = String::from_utf8_lossy(&header[0x150..0x160]).trim_end_matches('\0').to_string();
    judge(program_id, &product)?;
    let flags = &header[0x188..0x190];
    let romfs_offset = ncch + u32_at(&header, 0x1B0) as u64 * MEDIA;
    let romfs_size = u32_at(&header, 0x1B4) as u64 * MEDIA;
    if romfs_size == 0 || flags[7] & 0x02 != 0 {
        return Err(Problem::Corrupt("The dump has no RomFS (the game's data). It is incomplete; dump it again.".into()));
    }
    if romfs_offset + romfs_size > reader.len {
        return Err(Problem::Corrupt("The file is shorter than its own header says: it is an incomplete copy or download.".into()));
    }
    // the RomFS in the clear (decrypted, whatever the flags say: some tools
    // leave them as they were), or with the fixed key, or refused
    let (crypt, crypto) = if reader.read(romfs_offset, 4)? == b"IVFC" {
        (Crypt::Plain, "decrypted")
    } else if flags[7] & 0x04 == 0 && flags[7] & 0x01 != 0 && program_id >> 32 == 0x0004_0000 {
        let counter = ncch_counter(&header, 3, romfs_offset - ncch);
        let mut romfs = RomfsReader::new(&path, romfs_offset, romfs_size, Crypt::Fixed(counter))?;
        if romfs.read(0, 4)? != b"IVFC" {
            return Err(Problem::Corrupt("The dump says its data uses the fixed key, but it doesn't read with it: the dump is damaged.".into()));
        }
        (Crypt::Fixed(counter), "fixed key (no console key needed)")
    } else {
        return Err(Problem::Encrypted(format!("This dump is encrypted. {DECRYPT_HINT}")));
    };
    Ok(VoiceDump { path: path.clone(), kind, title: program_id, product, crypto, origin: Origin::Image { path, romfs_offset, romfs_size, crypt } })
}

// ---- reading the RomFS -----------------------------------------------------------

/// a file of the RomFS: its path and where its bytes are.
struct Entry {
    path: String,
    offset: u64,
    size: u64,
}

enum Romfs {
    Image { reader: RomfsReader, files: Vec<Entry>, level3: u64, sizes: [u64; 3], logs: [u32; 3], master: Vec<u8> },
    Folder { root: PathBuf, files: Vec<Entry> },
}

impl Romfs {
    fn open(dump: &VoiceDump) -> Result<Romfs, Problem> {
        Romfs::open_origin(&dump.origin)
    }

    /// jpdlc: also a DLC content's RomFS.
    fn open_origin(origin: &Origin) -> Result<Romfs, Problem> {
        match origin {
            Origin::Folder(root) => {
                let mut files = Vec::new();
                dump::find_files(root, 0, &mut |path| {
                    if let Ok(relative) = path.strip_prefix(root) {
                        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                        files.push(Entry { path: relative.to_string_lossy().replace('\\', "/"), offset: 0, size });
                    }
                });
                Ok(Romfs::Folder { root: root.clone(), files })
            }
            Origin::Image { path, romfs_offset, romfs_size, crypt } => {
                let mut reader = RomfsReader::new(path, *romfs_offset, *romfs_size, crypt.clone())?;
                let damaged = || Problem::Corrupt("The game's data (RomFS) has a damaged header: the dump is damaged.".into());
                let ivfc = reader.read(0, 0x60)?;
                let master_size = u32_at(&ivfc, 0x08) as u64;
                let mut sizes = [0u64; 3];
                let mut logs = [0u32; 3];
                for i in 0..3 {
                    sizes[i] = u64_at(&ivfc, 0x0C + i * 0x18 + 8);
                    logs[i] = u32_at(&ivfc, 0x0C + i * 0x18 + 0x10);
                }
                if logs.iter().any(|&l| !(9..=24).contains(&l)) || master_size > 1 << 20 {
                    return Err(damaged());
                }
                let master = reader.read(0x60, master_size as usize)?;
                let level3 = (0x60 + master_size).next_multiple_of(1 << logs[2]);
                let head = reader.read(level3, 0x28)?;
                let at = |i: usize| u32_at(&head, 4 + i * 4) as u64;
                let (dir_meta, dir_size, file_meta, file_size, data) = (at(2), at(3), at(6), at(7), at(8));
                if dir_size > 64 << 20 || file_size > 64 << 20 {
                    return Err(damaged());
                }
                let dirs = reader.read(level3 + dir_meta, dir_size as usize)?;
                let metas = reader.read(level3 + file_meta, file_size as usize)?;
                let mut files = Vec::new();
                walk(&dirs, &metas, 0, String::new(), level3 + data, &mut files, 0).ok_or_else(damaged)?;
                Ok(Romfs::Image { reader, files, level3, sizes, logs, master })
            }
        }
    }

    fn files(&self) -> &[Entry] {
        match self {
            Romfs::Image { files, .. } | Romfs::Folder { files, .. } => files,
        }
    }

    fn read(&mut self, entry: &Entry) -> Result<Vec<u8>, Problem> {
        match self {
            Romfs::Image { reader, .. } => reader.read(entry.offset, entry.size as usize),
            Romfs::Folder { root, .. } => std::fs::read(root.join(&entry.path)).map_err(|e| Problem::Unreadable(format!("{} can't be read ({e}).", entry.path))),
        }
    }

    /// the whole RomFS against its IVFC hash tree (dump::verify_romfs, over
    /// the decrypting reader). A folder has no tree to check.
    fn verify(&mut self, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<bool, Problem> {
        let Romfs::Image { reader, level3, sizes, logs, master, .. } = self else { return Ok(false) };
        let l1 = (*level3 + sizes[2]).next_multiple_of(1 << logs[0]);
        let l2 = (l1 + sizes[0]).next_multiple_of(1 << logs[1]);
        let starts = [l1, l2, *level3];
        let total: u64 = sizes.iter().sum();
        let mut done = 0u64;
        let mut hashes = master.clone();
        for i in 0..3 {
            let block = 1u64 << logs[i];
            let blocks = sizes[i].div_ceil(block);
            if (hashes.len() as u64) < blocks * 32 {
                return Err(Problem::Corrupt("The game's RomFS has a damaged hash tree.".into()));
            }
            let mut level_bytes = Vec::new();
            let chunk = (32u64 << 20).max(block) / block * block;
            let mut at = 0u64;
            while at < blocks * block {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Problem::Unreadable("Stopped.".into()));
                }
                let n = chunk.min(blocks * block - at);
                let len = n.min(reader.size.saturating_sub(starts[i] + at));
                let mut data = reader.read(starts[i] + at, len as usize)?;
                data.resize(n as usize, 0);
                for (k, piece) in data.chunks(block as usize).enumerate() {
                    let index = (at / block) as usize + k;
                    let valid = (sizes[i] - (at + k as u64 * block).min(sizes[i])).min(block) as usize;
                    let mut padded = piece[..valid].to_vec();
                    padded.resize(block as usize, 0);
                    if Sha256::digest(&padded).as_slice() != &hashes[index * 32..index * 32 + 32] {
                        return Err(Problem::Corrupt(format!(
                            "The dump's data is damaged (RomFS level {} block {index} doesn't match its hash). Copy or dump it again.",
                            i + 1
                        )));
                    }
                }
                if i < 2 {
                    level_bytes.extend_from_slice(&data[..n.min(sizes[i] - at) as usize]);
                }
                at += n;
                done += n.min(sizes[i]);
                progress((done as f32 / total as f32).min(1.0));
            }
            if i < 2 {
                hashes = level_bytes;
            }
        }
        Ok(true)
    }
}

/// the RomFS's directory tree: dir_at's files and folders, recursively.
fn walk(dirs: &[u8], metas: &[u8], dir_at: usize, path: String, data: u64, out: &mut Vec<Entry>, depth: usize) -> Option<()> {
    if depth > 32 {
        return None;
    }
    let name = |table: &[u8], at: usize, len: usize| -> Option<String> {
        let bytes = table.get(at..at + len)?;
        Some(String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()))
    };
    let dir = dirs.get(dir_at..dir_at + 0x18)?;
    let mut file = u32_at(dir, 0x0C);
    let mut guard = 0;
    while file != u32::MAX {
        let f = metas.get(file as usize..file as usize + 0x20)?;
        let file_name = name(metas, file as usize + 0x20, u32_at(f, 0x1C) as usize)?;
        out.push(Entry { path: format!("{path}{file_name}"), offset: data + u64_at(f, 0x08), size: u64_at(f, 0x10) });
        file = u32_at(f, 0x04);
        guard += 1;
        if guard > 1 << 20 {
            return None;
        }
    }
    let mut child = u32_at(dir, 0x08);
    while child != u32::MAX {
        let c = dirs.get(child as usize..child as usize + 0x18)?;
        let child_name = name(dirs, child as usize + 0x18, u32_at(c, 0x14) as usize)?;
        walk(dirs, metas, child as usize, format!("{path}{child_name}/"), data, out, depth + 1)?;
        child = u32_at(c, 0x04);
        guard += 1;
        if guard > 1 << 20 {
            return None;
        }
    }
    Some(())
}

// ---- the sound archive's labels ---------------------------------------------------

/// every name in a CSAR's string table (sounds, banks, groups, players).
pub fn csar_labels(data: &[u8]) -> Option<HashSet<String>> {
    if data.get(..4)? != b"CSAR" {
        return None;
    }
    let sections = u16_at(data, 0x10) as usize;
    let mut strg = None;
    for i in 0..sections.min(8) {
        let at = 0x14 + i * 12;
        if u16_at(data, at) == 0x2000 {
            strg = Some(u32_at(data, at + 4) as usize);
        }
    }
    let body = strg? + 8;
    let table = body.checked_add_signed(i32::from_le_bytes(data.get(body + 4..body + 8)?.try_into().ok()?) as isize)?;
    let count = u32_at(data.get(table..table + 4)?, 0) as usize;
    let mut names = HashSet::with_capacity(count);
    for i in 0..count {
        let entry = data.get(table + 4 + i * 12..table + 16 + i * 12)?;
        let (offset, size) = (u32_at(entry, 4) as usize, u32_at(entry, 8) as usize);
        let bytes = data.get(table + offset..table + offset + size.saturating_sub(1))?;
        names.insert(String::from_utf8_lossy(bytes).into_owned());
    }
    Some(names)
}

/// what the Japanese archive is against the installed one: None when it is
/// fit, else why not.
fn judge_archive(candidate: &[u8], installed: &[u8]) -> Result<String, Problem> {
    if candidate == installed {
        return Err(Problem::WrongRegion(
            "This dump has the English voices: its sound files are the same as your game's. Select a dump with the Japanese voices: a Japanese Fire Emblem if, or an \"undub\".".into(),
        ));
    }
    let ours = csar_labels(installed).ok_or_else(|| Problem::Corrupt("Your installed game's sound archive can't be read. Rebuild Haiku first.".into()))?;
    let theirs = csar_labels(candidate).ok_or_else(|| Problem::Corrupt("The dump's sound archive can't be read: the dump is damaged or not the game.".into()))?;
    let japanese = theirs.iter().any(|l| l.starts_with("VOICE_PLAYER_M5_") || l.starts_with("VOICE_PLAYER_F5_"));
    let voices: Vec<&String> = ours.iter().filter(|l| l.starts_with("VOICE_") && !l.contains("KANNA")).collect();
    let missing = voices.iter().filter(|l| !theirs.contains(l.as_str())).count();
    if !japanese {
        return Err(Problem::WrongRegion("This dump's sound archive isn't the Japanese one (it doesn't have the Japanese game's voice labels). Select a dump with the Japanese voices.".into()));
    }
    if missing * 50 > voices.len() {
        return Err(Problem::WrongVersion(format!(
            "This dump's sound archive lacks {missing} of the {} voice labels Haiku needs: it is from a different version or modified.",
            voices.len()
        )));
    }
    Ok(format!("{} labels, {} of the game's {} voice labels have a Japanese line ({} without)", theirs.len(), voices.len() - missing, voices.len(), missing))
}

// ---- extracting -------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct Progress {
    pub percent: f32,
    pub step: String,
    pub detail: String,
}
pub type Shared = Arc<Mutex<Progress>>;

fn say(progress: &Shared, percent: f32, step: &str, detail: String) {
    if let Ok(mut p) = progress.lock() {
        p.percent = percent;
        p.step = step.to_owned();
        p.detail = detail;
    }
}

/// the CSTM header's version (byte 9) aside, whether two streams are the
/// same: the Japanese build wrote an older version number on the same audio.
fn same_stream(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.len() > 10 && a[..9] == b[..9] && a[10..] == b[10..]
}

/// whether a RomFS file is one of the voice layer's, and if so, whether it
/// is to be taken given the installed game's file (None: not there).
fn wanted(path: &str, ours: Option<&[u8]>, theirs: &[u8]) -> bool {
    if path == ARCHIVE || path == TABLE {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    // the sound folder's files (the DSP's firmware, and the DLC's archives,
    // which come with the DLC, aside), and the movies' sound cues
    let sound = lower.starts_with("sound/") && lower != "sound/dspaudio.cdc" && !lower.starts_with("sound/dlc_");
    let cue = lower.starts_with("movie/") && lower.ends_with("_bgm.bin");
    if !sound && !cue {
        return false;
    }
    match ours {
        None => true,
        Some(ours) => ours != theirs && !(lower.ends_with(".bcstm") && same_stream(ours, theirs)),
    }
}

/// what was installed, for the screen and the log.
#[derive(Clone, Debug)]
pub struct Installed {
    pub files: usize,
    pub bytes: u64,
    pub clips: usize,
    pub source: String,
    pub note: String,
}

/// the layer installed in data, from its manifest.
pub fn installed(data: &Path) -> Option<Installed> {
    let text = std::fs::read_to_string(data.join(FOLDER).join(MANIFEST)).ok()?;
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_owned())).unwrap_or_default();
    Some(Installed {
        files: field("files ").parse().unwrap_or(0),
        bytes: field("bytes ").parse().unwrap_or(0),
        clips: field("clips ").parse().unwrap_or(0),
        source: field("source "),
        note: field("archive "),
    })
}

/// takes the Japanese voices out of dump into data/voices-ja, after
/// checking it against the game installed in installed_romfs (data/romfs).
pub fn extract(dump: &VoiceDump, installed_romfs: &Path, data: &Path, progress: &Shared, cancel: &AtomicBool) -> Result<Installed, String> {
    let fail = |p: Problem| p.message().to_owned();
    let ours = |path: &str| std::fs::read(installed_romfs.join(path)).ok();
    let installed_archive = ours(ARCHIVE).ok_or("Haiku isn't built yet: the game's own files (data\\romfs) are needed to compare the voices with. Build Haiku first.")?;
    say(progress, 0.0, "Reading the dump", short(&dump.path));
    let mut romfs = Romfs::open(dump).map_err(fail)?;
    // the archive and its table by the Western game's names; a dump that
    // names them otherwise (not seen: the undub keeps the names) by the
    // sound folder's one other archive and table, laid under these names
    let files = romfs.files();
    let pick = |exact: &str, prefix: &str, suffix: &str| -> Option<Entry> {
        let found = files.iter().find(|e| e.path.eq_ignore_ascii_case(exact)).or_else(|| {
            files
                .iter()
                .filter(|e| {
                    let l = e.path.to_ascii_lowercase();
                    l.starts_with(prefix) && l.ends_with(suffix) && !l.starts_with("sound/dlc_") && !l[6..].contains('/')
                })
                .max_by_key(|e| e.size)
        })?;
        Some(Entry { path: found.path.clone(), offset: found.offset, size: found.size })
    };
    let archive_entry = pick(ARCHIVE, "sound/", ".bcsar").ok_or("This dump has no sound archive (sound/IRON15_sound.bcsar): it is not Fire Emblem Fates or if.")?;
    let table_entry = pick(TABLE, "sound/indirectsound", ".lz").ok_or("This dump has no indirect-sound table (sound/IndirectSound.bin.lz): it is damaged or not the game.")?;
    let renamed = |path: &str| -> String {
        if path == archive_entry.path {
            ARCHIVE.to_owned()
        } else if path == table_entry.path {
            TABLE.to_owned()
        } else {
            path.to_owned()
        }
    };
    if archive_entry.path != ARCHIVE || table_entry.path != TABLE {
        crate::log::line(format!("voices: the archive is {} and its table {}: laid as {ARCHIVE} and {TABLE}", archive_entry.path, table_entry.path));
    }
    say(progress, 1.0, "Checking the voices", "the sound archive's labels".into());
    let archive = romfs.read(&archive_entry).map_err(fail)?;
    let note = judge_archive(&archive, &installed_archive).map_err(fail)?;
    crate::log::line(format!("voices: {note}"));
    drop(archive);
    // the whole RomFS against its hash tree
    let verified = romfs
        .verify(&|f| say(progress, 2.0 + 58.0 * f, "Checking the dump", format!("every block of its data against its hash, {:.0}%", f * 100.0)), cancel)
        .map_err(fail)?;
    // the files, into a folder beside the final one, then swapped in
    let out = data.join(FOLDER);
    let temp = data.join(format!("{FOLDER}.new"));
    let _ = std::fs::remove_dir_all(&temp);
    let entries: Vec<Entry> = romfs
        .files()
        .iter()
        .filter(|e| {
            let lower = e.path.to_ascii_lowercase();
            lower.starts_with("sound/") || (lower.starts_with("movie/") && lower.ends_with("_bgm.bin"))
        })
        .map(|e| Entry { path: e.path.clone(), offset: e.offset, size: e.size })
        .collect();
    let mut written: Vec<(String, u64, String)> = Vec::new();
    let (mut bytes, mut clips) = (0u64, 0usize);
    for (i, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_dir_all(&temp);
            return Err("Stopped.".into());
        }
        let data_bytes = romfs.read(entry).map_err(fail)?;
        let path = renamed(&entry.path);
        let ours_bytes = ours(&path);
        if !wanted(&path, ours_bytes.as_deref(), &data_bytes) {
            continue;
        }
        let target = temp.join("romfs").join(&path);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| format!("{} can't be made ({e}).", target.display()))?;
        std::fs::write(&target, &data_bytes).map_err(|e| format!("{} can't be written ({e}). Is the disk full?", target.display()))?;
        let hash: String = Sha256::digest(&data_bytes).iter().map(|b| format!("{b:02x}")).collect();
        bytes += data_bytes.len() as u64;
        clips += entry.path.to_ascii_lowercase().ends_with(".aac") as usize;
        written.push((path.clone(), data_bytes.len() as u64, hash));
        say(progress, 60.0 + 30.0 * (i + 1) as f32 / entries.len() as f32, "Taking out the voices", format!("{} files, {} MB: {}", written.len(), bytes / 1_000_000, entry.path));
    }
    // every file read back against its hash from the dump
    for (i, (path, size, hash)) in written.iter().enumerate() {
        let back = std::fs::read(temp.join("romfs").join(path)).map_err(|e| format!("{path} can't be read back ({e})."))?;
        let again: String = Sha256::digest(&back).iter().map(|b| format!("{b:02x}")).collect();
        if back.len() as u64 != *size || again != *hash {
            return Err(format!("{path} doesn't read back as it was written: the disk may be failing."));
        }
        if i % 64 == 0 {
            say(progress, 90.0 + 9.0 * i as f32 / written.len() as f32, "Checking what was written", format!("{} of {} files", i, written.len()));
        }
    }
    let mut list = String::from("path\tsize\tsha256\n");
    for (path, size, hash) in &written {
        list += &format!("{path}\t{size}\t{hash}\n");
    }
    std::fs::write(temp.join("files.tsv"), list).map_err(|e| e.to_string())?;
    let manifest = format!(
        "# Haiku's Japanese voice layer, made by Haiku Setup from the player's own dump.\n\
         # Only sound files: the sound archive, its table, voice clips, movie audio and cues.\n\
         source {}\ntitle {:016X}\nproduct {}\nkind {}\ncrypto {}\nromfs_verified {}\nfiles {}\nbytes {}\nclips {}\narchive {}\n",
        dump.path.display(),
        dump.title,
        dump.product,
        dump.kind,
        dump.crypto,
        if verified { "yes (IVFC)" } else { "no (a folder has no hash tree)" },
        written.len(),
        bytes,
        clips,
        note
    );
    std::fs::write(temp.join(MANIFEST), &manifest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&temp, &out).map_err(|e| format!("{} can't be put in place ({e}).", out.display()))?;
    say(progress, 100.0, "Done", format!("{} files", written.len()));
    crate::log::line(format!("voices: installed {} files ({} MB, {clips} clips) from {}", written.len(), bytes / 1_000_000, dump.path.display()));
    Ok(Installed { files: written.len(), bytes, clips, source: dump.path.display().to_string(), note })
}

/// takes the layer out (Voices goes back to English with it).
pub fn remove(data: &Path) -> std::io::Result<()> {
    let out = data.join(FOLDER);
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    remove_dlc(data) // jpdlc: the DLC's voices go with them
}

// ---- jpdlc: the Japanese DLC's voices ---------------------------------------
//
// The player can also give the Japanese DLC (Fire Emblem if's, title
// 0004008C0012DC00: a decrypted .cia, or an emulator's sdmc folder with it
// installed). Its 62 contents are the US DLC's, at the same indexes and with
// the same file names (dev/reports/jpdlc-survey.md), so the DLC's voices are
// taken content by content: every file under a content's sound/ folder that
// differs from the installed DLC's file at the same path (the maps' sound
// archives, voice, narration and song streams; a stream differing only in
// its header's version byte is the same audio and is left out), and the
// route contents' movie cue tables that differ. Only paths the installed
// content has; nothing of the contents the US DLC lacks, nor any text,
// script or map. They go into data/voices-ja-dlc/dlc/<index>/romfs/..., the
// mod layout for DLC, which Haiku lays over the installed DLC with the
// game's Japanese voices (haiku_voices.rs).

/// the DLC's Japanese voices, beside FOLDER.
pub const DLC_FOLDER: &str = "voices-ja-dlc";
/// the Japanese DLC, for messages.
pub const JP_DLC_TITLE: u64 = 0x0004_008C_0012_DC00;

/// a content of the DLC picked: its index, its TMD size and hash, the file
/// and offset of its NCCH, and its RomFS (offset, size, how it is stored).
#[derive(Clone)]
struct DlcContent {
    index: u16,
    size: u64,
    hash: [u8; 32],
    file: PathBuf,
    at: u64,
    romfs: Option<(u64, u64, Crypt)>,
}

/// a DLC with (perhaps) Japanese voices, checked as far as its headers go.
#[derive(Clone)]
pub struct VoiceDlc {
    pub path: PathBuf,
    /// "CIA" or "an emulator's installed DLC".
    pub kind: &'static str,
    pub title: u64,
    pub version: u16,
    pub product: String,
    /// "decrypted" or "fixed key".
    pub crypto: &'static str,
    /// the contents its TMD lists.
    pub listed: usize,
    contents: Vec<DlcContent>,
}

impl VoiceDlc {
    pub fn describe(&self) -> String {
        let what = match self.title {
            JP_DLC_TITLE => "Fire Emblem if DLC (Japan)",
            crate::dlc::DLC_TITLE => "Fire Emblem Fates DLC (USA)",
            crate::eur::DLC_TITLE => "Fire Emblem Fates DLC (Europe)",
            _ => "Fire Emblem Fates or if DLC",
        };
        format!(
            "{what}, title {:016X} v{}, {} of its {} contents, {}, {}",
            self.title,
            self.version,
            self.contents.len(),
            self.listed,
            self.kind,
            self.crypto
        )
    }
}

/// a DLC content's NCCH at `at` in path: its program id, product code, and
/// its RomFS (none for a content without one), decrypted or with the fixed
/// key, else refused.
type ContentInfo = (u64, String, Option<(u64, u64, Crypt)>, &'static str);
fn dlc_content(path: &Path, reader: &mut Reader, at: u64) -> Result<ContentInfo, Problem> {
    let header = reader.read(at, 0x200)?;
    if &header[0x100..0x104] != b"NCCH" {
        return Err(Problem::Encrypted(format!("This DLC's contents are encrypted. {DECRYPT_HINT}")));
    }
    let program_id = u64_at(&header, 0x118);
    let product = String::from_utf8_lossy(&header[0x150..0x160]).trim_end_matches('\0').to_string();
    let flags = &header[0x188..0x190];
    let romfs_offset = at + u32_at(&header, 0x1B0) as u64 * MEDIA;
    let romfs_size = u32_at(&header, 0x1B4) as u64 * MEDIA;
    if romfs_size == 0 || flags[7] & 0x02 != 0 {
        return Ok((program_id, product, None, "decrypted"));
    }
    if romfs_offset + romfs_size > reader.len {
        return Err(Problem::Corrupt("The DLC file is shorter than its own header says: it is an incomplete copy or download.".into()));
    }
    if reader.read(romfs_offset, 4)? == b"IVFC" {
        return Ok((program_id, product, Some((romfs_offset - at, romfs_size, Crypt::Plain)), "decrypted"));
    }
    // the fixed key of a title that isn't a system title: the all-zero key
    if flags[7] & 0x04 == 0 && flags[7] & 0x01 != 0 {
        let counter = ncch_counter(&header, 3, romfs_offset - at);
        let mut romfs = RomfsReader::new(path, romfs_offset, romfs_size, Crypt::Fixed(counter))?;
        if romfs.read(0, 4)? == b"IVFC" {
            return Ok((program_id, product, Some((romfs_offset - at, romfs_size, Crypt::Fixed(counter))), "fixed key (no console key needed)"));
        }
        return Err(Problem::Corrupt("The DLC says its data uses the fixed key, but it doesn't read with it: the file is damaged.".into()));
    }
    Err(Problem::Encrypted(format!("This DLC is encrypted. {DECRYPT_HINT}")))
}

/// whether a title is DLC; a problem in words when it is not.
fn judge_dlc_title(title: u64) -> Result<(), Problem> {
    match title >> 32 {
        0x0004_008C => Ok(()),
        0x0004_0000 => Err(Problem::WrongKind("This is a game, not DLC. The game with the Japanese voices goes in Select file; Japanese DLC takes the DLC.".into())),
        0x0004_000E => Err(Problem::WrongKind("This is an update, not DLC. Select the Japanese DLC.".into())),
        _ => Err(Problem::NotADump(format!("This is not DLC (title {title:016X}). Select the Japanese DLC's decrypted .cia."))),
    }
}

/// the product code says Fire Emblem Fates or if DLC (CTR-M-BFX?).
fn judge_dlc_product(title: u64, product: &str) -> Result<(), Problem> {
    if product.starts_with("CTR-M-BFX") {
        return Ok(());
    }
    Err(Problem::WrongGame(format!(
        "This is DLC for a different game ({}, title {title:016X}), not Fire Emblem Fates or Fire Emblem if.",
        if product.trim().is_empty() { "no product code" } else { product }
    )))
}

/// what the file or folder at path is, if it is DLC (its voices are checked
/// by extract_dlc, against the installed DLC).
pub fn check_dlc(path: &Path) -> Result<VoiceDlc, Problem> {
    if path.is_dir() {
        return check_dlc_folder(path);
    }
    let mut reader = Reader::open(path)?;
    if reader.len < 0x2020 {
        return Err(Problem::NotADump("This file is far too small to be DLC.".into()));
    }
    let head = reader.read(0, 0x2020)?;
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"7z\xBC\xAF\x27\x1C") || head.starts_with(b"Rar!") {
        return Err(Problem::NotADump("This is an archive. Extract the DLC from it first, then select the .cia inside.".into()));
    }
    if &head[0x100..0x104] == b"NCSD" || &head[0x100..0x104] == b"NCCH" {
        return Err(Problem::WrongKind("This is a game, not DLC. The game with the Japanese voices goes in Select file; Japanese DLC takes the DLC.".into()));
    }
    if u32_at(&head, 0) != 0x2020 {
        return Err(Problem::NotADump("This is not a CIA. Select the Japanese DLC's decrypted .cia, or an emulator's sdmc folder with it installed.".into()));
    }
    let align = |n: u64| n.next_multiple_of(0x40);
    let tmd_offset = align(align(align(0x2020) + u32_at(&head, 0x08) as u64) + u32_at(&head, 0x0C) as u64);
    let tmd_size = u32_at(&head, 0x10) as usize;
    let tmd = reader.read(tmd_offset, tmd_size)?;
    let (title, version, records) = dump::tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The CIA's title metadata can't be read: the file is damaged.".into()))?;
    judge_dlc_title(title)?;
    let mut contents = Vec::new();
    let mut offset = align(tmd_offset + tmd_size as u64);
    let (mut product, mut crypto) = (String::new(), "decrypted");
    for (index, _, flags, size, hash) in records.iter().copied() {
        if head[0x20 + index as usize / 8] & (0x80 >> (index % 8)) == 0 {
            continue;
        }
        if flags & 1 != 0 {
            return Err(Problem::Encrypted(format!("This DLC CIA is encrypted (the eShop's title key). {DECRYPT_HINT}")));
        }
        if offset + size > reader.len {
            return Err(Problem::Corrupt("The CIA is shorter than its own header says: it is an incomplete copy or download.".into()));
        }
        let (_, content_product, romfs, how) = dlc_content(path, &mut reader, offset)?;
        if product.is_empty() {
            product = content_product;
        }
        if how != "decrypted" {
            crypto = how;
        }
        contents.push(DlcContent { index, size, hash, file: path.to_path_buf(), at: offset, romfs });
        offset = align(offset + size);
    }
    if contents.is_empty() {
        return Err(Problem::Corrupt("The CIA holds none of the DLC's contents.".into()));
    }
    judge_dlc_product(title, &product)?;
    Ok(VoiceDlc { path: path.to_path_buf(), kind: "CIA", title, version, product, crypto, listed: records.len(), contents })
}

/// an emulator's installed DLC: the title's .tmd and its .app contents.
fn check_installed_dlc(tmd_path: &Path) -> Result<VoiceDlc, Problem> {
    let tmd = std::fs::read(tmd_path).map_err(|e| Problem::Unreadable(format!("The DLC's .tmd can't be read ({e}).")))?;
    let (title, version, records) = dump::tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The DLC's .tmd is damaged.".into()))?;
    judge_dlc_title(title)?;
    let dir = tmd_path.parent().unwrap_or(Path::new("."));
    let mut contents = Vec::new();
    let (mut product, mut crypto) = (String::new(), "decrypted");
    for (index, id, _, size, hash) in records.iter().copied() {
        let app = [format!("{id:08x}.app"), format!("{id:08X}.app")].into_iter().map(|n| dir.join(n)).find(|p| p.is_file());
        let Some(app) = app else { continue };
        let mut reader = Reader::open(&app)?;
        let (_, content_product, romfs, how) = dlc_content(&app, &mut reader, 0)?;
        if product.is_empty() {
            product = content_product;
        }
        if how != "decrypted" {
            crypto = how;
        }
        contents.push(DlcContent { index, size, hash, file: app, at: 0, romfs });
    }
    if contents.is_empty() {
        return Err(Problem::Corrupt("The emulator's DLC folder has none of the DLC's contents.".into()));
    }
    judge_dlc_product(title, &product)?;
    Ok(VoiceDlc { path: tmd_path.to_path_buf(), kind: "an emulator's installed DLC", title, version, product, crypto, listed: records.len(), contents })
}

/// a folder: an emulator's sdmc (or any folder under it) with Fates or if
/// DLC installed (the Japanese one first), or a folder with a DLC .cia.
fn check_dlc_folder(dir: &Path) -> Result<VoiceDlc, Problem> {
    let mut tmds = Vec::new();
    let mut cias = Vec::new();
    dump::find_files(dir, 0, &mut |path| {
        let lower = path.to_string_lossy().to_lowercase().replace(std::path::MAIN_SEPARATOR, "/");
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "tmd" && lower.contains("/0004008c/") && lower.contains("/content") {
            tmds.push(path.to_path_buf());
        } else if ext == "cia" {
            cias.push(path.to_path_buf());
        }
    });
    tmds.sort();
    let mut fitting: Vec<VoiceDlc> = tmds.iter().rev().filter_map(|t| check_installed_dlc(t).ok()).collect();
    fitting.extend(cias.iter().filter_map(|c| check_dlc(c).ok()));
    // the Japanese DLC before any other the folder has
    fitting.sort_by_key(|d| d.title != JP_DLC_TITLE);
    fitting.into_iter().next().ok_or_else(|| {
        Problem::NotADump(
            "This folder doesn't hold Fire Emblem if or Fates DLC. Select the Japanese DLC's decrypted .cia, or an emulator's sdmc folder with it installed.".into(),
        )
    })
}

/// every content of the DLC against its TMD hash.
fn verify_dlc(dlc: &VoiceDlc, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), Problem> {
    let total: u64 = dlc.contents.iter().map(|c| c.size).sum::<u64>().max(1);
    let mut done = 0u64;
    for content in &dlc.contents {
        let mut reader = Reader::open(&content.file)?;
        let mut hasher = Sha256::new();
        let mut at = 0;
        while at < content.size {
            if cancel.load(Ordering::Relaxed) {
                return Err(Problem::Unreadable("Stopped.".into()));
            }
            let n = (content.size - at).min(16 << 20) as usize;
            hasher.update(reader.read(content.at + at, n)?);
            at += n as u64;
            done += n as u64;
            progress(done as f32 / total as f32);
        }
        if hasher.finalize().as_slice() != content.hash {
            return Err(Problem::Corrupt(format!("DLC content {:04X} is damaged (it doesn't match its hash). Copy or dump the DLC again.", content.index)));
        }
    }
    Ok(())
}

/// the DLC installed in data (data/dlc/<title> with its dlc.txt), the one
/// whose contents the Japanese voices are laid over.
pub fn installed_dlc_dir(data: &Path) -> Option<(u64, PathBuf)> {
    [crate::dlc::DLC_TITLE, crate::eur::DLC_TITLE]
        .into_iter()
        .map(|t| (t, data.join("dlc").join(format!("{t:016X}"))))
        .find(|(_, dir)| dir.join("dlc.txt").is_file())
}

/// whether a DLC RomFS path is one the DLC voice layer takes (when it
/// differs): the sound folder's files and the movies' sound cues.
fn dlc_sound_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let movie = lower.starts_with("movie/") || lower.contains("/movie/");
    lower.starts_with("sound/") || lower.contains("/sound/") || (movie && lower.ends_with("_bgm.bin"))
}

/// whether a DLC file is taken given the installed DLC's at the same path.
fn dlc_wanted(path: &str, ours: &[u8], theirs: &[u8]) -> bool {
    dlc_sound_path(path) && ours != theirs && !(path.to_ascii_lowercase().ends_with(".bcstm") && same_stream(ours, theirs))
}

/// a voice line's stream (a guest's or a hero's).
fn voice_stream(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("VOICE_") && name.ends_with(".bcstm")
}
/// the narrator's (Heirs of Fate).
fn narration_stream(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("STRM_NRR_") && !name.contains("_OFF") && name.ends_with(".bcstm")
}

/// the files of an installed DLC content's RomFS folder, by path.
fn folder_files(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    dump::find_files(root, 0, &mut |path| {
        if let Ok(relative) = path.strip_prefix(root) {
            out.insert(relative.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"));
        }
    });
    out
}

/// what was installed for the DLC, for the screen and the log.
#[derive(Clone, Debug, Default)]
pub struct InstalledDlc {
    pub files: usize,
    pub bytes: u64,
    /// voice streams with a Japanese version taken.
    pub clips: usize,
    pub narration: usize,
    /// the installed DLC's voice streams the Japanese DLC has the same.
    pub unchanged: usize,
    pub contents: usize,
    pub source: String,
}

/// the DLC voice layer installed in data, from its manifest.
pub fn installed_dlc(data: &Path) -> Option<InstalledDlc> {
    let text = std::fs::read_to_string(data.join(DLC_FOLDER).join(MANIFEST)).ok()?;
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_owned())).unwrap_or_default();
    Some(InstalledDlc {
        files: field("files ").parse().unwrap_or(0),
        bytes: field("bytes ").parse().unwrap_or(0),
        clips: field("clips ").parse().unwrap_or(0),
        narration: field("narration ").parse().unwrap_or(0),
        unchanged: field("unchanged_voices ").parse().unwrap_or(0),
        contents: field("contents ").parse().unwrap_or(0),
        source: field("source "),
    })
}

/// takes the Japanese voices of dlc into data/voices-ja-dlc, after checking
/// it against the DLC installed in data/dlc.
pub fn extract_dlc(dlc: &VoiceDlc, data: &Path, progress: &Shared, cancel: &AtomicBool) -> Result<InstalledDlc, String> {
    let fail = |p: Problem| p.message().to_owned();
    let (installed_title, installed) = installed_dlc_dir(data)
        .ok_or("Haiku has no DLC installed to give the Japanese voices to. Add the DLC first (Change dump or DLC on the Play screen), then its Japanese voices.")?;
    say(progress, 0.0, "Checking the DLC", short(&dlc.path));
    verify_dlc(dlc, &|f| say(progress, 40.0 * f, "Checking the DLC", format!("every content against its hash, {:.0}%", f * 100.0)), cancel).map_err(fail)?;
    let out = data.join(DLC_FOLDER);
    let temp = data.join(format!("{DLC_FOLDER}.new"));
    let _ = std::fs::remove_dir_all(&temp);
    let fail_clean = |message: String| {
        let _ = std::fs::remove_dir_all(&temp);
        message
    };
    let mut written: Vec<(String, u64, String)> = Vec::new();
    let (mut bytes, mut clips, mut narration, mut voices, mut unchanged) = (0u64, 0usize, 0usize, 0usize, 0usize);
    let (mut contents_used, mut not_ours, mut misfits) = (0usize, 0usize, Vec::new());
    for (i, content) in dlc.contents.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(fail_clean("Stopped.".into()));
        }
        let content_name = format!("{:04X}", content.index);
        let ours_root = installed.join(&content_name).join("romfs");
        let Some((romfs_at, romfs_size, crypt)) = content.romfs.clone() else { continue };
        if !ours_root.is_dir() {
            not_ours += 1;
            continue;
        }
        let origin = Origin::Image { path: content.file.clone(), romfs_offset: content.at + romfs_at, romfs_size, crypt };
        let mut romfs = Romfs::open_origin(&origin).map_err(|p| fail_clean(fail(p)))?;
        // the same content: most of the installed content's files (its
        // text aside, which each region keeps in its own folder) are there
        let ours_files = folder_files(&ours_root);
        let theirs: HashSet<String> = romfs.files().iter().map(|e| e.path.clone()).collect();
        let comparable: Vec<&String> = ours_files.iter().filter(|p| !p.contains("/m/") && !p.starts_with("m/") && p.as_str() != "fst.bin").collect();
        let shared = comparable.iter().filter(|p| theirs.contains(p.as_str())).count();
        if comparable.is_empty() {
            // a placeholder in the installed DLC: content it doesn't have
            not_ours += theirs.iter().any(|p| p != "fst.bin") as usize;
            continue;
        }
        if shared * 2 < comparable.len() {
            misfits.push(content_name.clone());
            continue;
        }
        let entries: Vec<Entry> = romfs
            .files()
            .iter()
            .filter(|e| dlc_sound_path(&e.path) && ours_files.contains(&e.path))
            .map(|e| Entry { path: e.path.clone(), offset: e.offset, size: e.size })
            .collect();
        let before = written.len();
        for entry in &entries {
            let theirs_bytes = romfs.read(entry).map_err(|p| fail_clean(fail(p)))?;
            let ours_bytes = std::fs::read(ours_root.join(&entry.path)).map_err(|e| fail_clean(format!("The installed DLC's {} can't be read ({e}).", entry.path)))?;
            let voice = voice_stream(&entry.path);
            voices += voice as usize;
            if !dlc_wanted(&entry.path, &ours_bytes, &theirs_bytes) {
                unchanged += voice as usize;
                continue;
            }
            let path = format!("dlc/{content_name}/romfs/{}", entry.path);
            let target = temp.join(&path);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| fail_clean(format!("{} can't be made ({e}).", target.display())))?;
            std::fs::write(&target, &theirs_bytes).map_err(|e| fail_clean(format!("{} can't be written ({e}). Is the disk full?", target.display())))?;
            let hash: String = Sha256::digest(&theirs_bytes).iter().map(|b| format!("{b:02x}")).collect();
            bytes += theirs_bytes.len() as u64;
            clips += voice as usize;
            narration += narration_stream(&entry.path) as usize;
            written.push((path, theirs_bytes.len() as u64, hash));
        }
        contents_used += (written.len() > before) as usize;
        say(
            progress,
            40.0 + 50.0 * (i + 1) as f32 / dlc.contents.len() as f32,
            "Taking out the DLC's voices",
            format!("content {content_name}: {} files, {} MB", written.len(), bytes / 1_000_000),
        );
    }
    if !misfits.is_empty() {
        crate::log::line(format!("voices: DLC contents {} don't match the installed DLC's; left out", misfits.join(", ")));
    }
    // the language: the installed DLC's voice lines, Japanese in this one
    if voices == 0 {
        return Err(fail_clean("This DLC has none of the voice files of the DLC Haiku has installed: it is a different DLC, or damaged.".into()));
    }
    if clips == 0 {
        return Err(fail_clean(
            "This DLC has the English voices: its voice files are the same as your installed DLC's. Select the Japanese DLC (Fire Emblem if, 0004008C0012DC00).".into(),
        ));
    }
    if clips * 10 < voices * 9 {
        return Err(fail_clean(format!("Only {clips} of this DLC's {voices} voice files differ from your installed DLC's: it isn't the Japanese DLC, or it is modified.")));
    }
    // every file read back against its hash from the DLC
    for (i, (path, size, hash)) in written.iter().enumerate() {
        let back = std::fs::read(temp.join(path)).map_err(|e| fail_clean(format!("{path} can't be read back ({e}).")))?;
        let again: String = Sha256::digest(&back).iter().map(|b| format!("{b:02x}")).collect();
        if back.len() as u64 != *size || again != *hash {
            return Err(fail_clean(format!("{path} doesn't read back as it was written: the disk may be failing.")));
        }
        if i % 32 == 0 {
            say(progress, 90.0 + 9.0 * i as f32 / written.len() as f32, "Checking what was written", format!("{} of {} files", i, written.len()));
        }
    }
    let mut list = String::from("path\tsize\tsha256\n");
    for (path, size, hash) in &written {
        list += &format!("{path}\t{size}\t{hash}\n");
    }
    std::fs::write(temp.join("files.tsv"), list).map_err(|e| fail_clean(e.to_string()))?;
    let manifest = format!(
        "# Haiku's Japanese DLC voice layer, made by Haiku Setup from the player's own DLC.\n\
         # Only sound files of the DLC's contents (archives, voice, narration and song streams) and\n\
         # the route contents' movie cue tables, laid over the installed DLC: dlc/<content>/romfs/...\n\
         source {}\ntitle {:016X}\nversion {}\nproduct {}\nkind {}\ncrypto {}\nverified yes (every content against its TMD hash)\n\
         for_dlc {:016X}\ncontents {}\nfiles {}\nbytes {}\nclips {}\nnarration {}\nvoices_installed {}\nunchanged_voices {}\nnot_in_installed {}\nleft_out {}\n",
        dlc.path.display(),
        dlc.title,
        dlc.version,
        dlc.product,
        dlc.kind,
        dlc.crypto,
        installed_title,
        contents_used,
        written.len(),
        bytes,
        clips,
        narration,
        voices,
        unchanged,
        not_ours,
        if misfits.is_empty() { "none".to_owned() } else { misfits.join(" ") }
    );
    std::fs::write(temp.join(MANIFEST), &manifest).map_err(|e| fail_clean(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&temp, &out).map_err(|e| format!("{} can't be put in place ({e}).", out.display()))?;
    say(progress, 100.0, "Done", format!("{} files", written.len()));
    crate::log::line(format!(
        "voices: DLC: installed {} files ({} MB) in {contents_used} contents: {clips} of the installed DLC's {voices} voice clips, {narration} narration, {unchanged} voice clips the same in both; {not_ours} contents the installed DLC hasn't",
        written.len(),
        bytes / 1_000_000,
    ));
    Ok(InstalledDlc { files: written.len(), bytes, clips, narration, unchanged, contents: contents_used, source: dlc.path.display().to_string() })
}

/// takes the DLC voice layer out.
pub fn remove_dlc(data: &Path) -> std::io::Result<()> {
    let out = data.join(DLC_FOLDER);
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    Ok(())
}

/// `--voices-dlc <dlc> [--out <data>]`: checks the Japanese DLC and takes its
/// voices out into data/voices-ja-dlc, printing progress, against the DLC
/// installed in data/dlc (data is the data folder beside the setup unless
/// --out names another).
pub fn command_dlc(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: --voices-dlc <dlc .cia or sdmc folder> [--out <data>]");
        return 2;
    };
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let data = option("--out").unwrap_or_else(|| crate::pipeline::Paths::new(crate::root()).data()); // versions: the playing version's
    crate::log::open(&crate::pipeline::Paths::new(crate::root()).log_file(), true);
    run_dlc(Path::new(path), &data)
}

/// checks and installs the DLC's voices, printing progress (the commands).
fn run_dlc(path: &Path, data: &Path) -> i32 {
    let dlc = match check_dlc(path) {
        Ok(d) => d,
        Err(p) => {
            crate::log::line(format!("{}: {}", p.kind(), p.message()));
            return 1;
        }
    };
    crate::log::line(format!("voices: DLC: {}", dlc.describe()));
    let progress: Shared = Arc::new(Mutex::new(Progress::default()));
    let cancel = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| extract_dlc(&dlc, data, &progress, &cancel));
        print_progress(&progress, &|| job.is_finished());
        job.join().unwrap()
    });
    match result {
        Ok(done) => {
            crate::log::line(format!(
                "ok: DLC: {} files, {} MB, {} voice clips, {} narration, {} the same in both",
                done.files,
                done.bytes / 1_000_000,
                done.clips,
                done.narration,
                done.unchanged
            ));
            0
        }
        Err(e) => {
            crate::log::line(format!("failed: {e}"));
            1
        }
    }
}

/// prints a job's progress until it is done.
fn print_progress(progress: &Shared, finished: &dyn Fn() -> bool) {
    let mut last = String::new();
    while !finished() {
        let p = progress.lock().unwrap().clone();
        let coarse = format!("{:.0}{}", p.percent / 5.0, p.step);
        if coarse != last {
            println!("{:3.0}% {} {}", p.percent, p.step, p.detail);
            last = coarse;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

// ---- the Voices setting, in Haiku's settings file ---------------------------

/// Haiku's settings file beside the setup (Haiku.exe keeps its settings in
/// userdata/haiku/settings.toml next to it).
pub fn settings_file(root: &Path) -> PathBuf {
    root.join("userdata").join("haiku").join("settings.toml")
}

/// the Voices setting: true for Japanese. versions2: in a Haiku folder
/// with games\<version>\, the playing version's own
/// ([pc.per_version.<tag>] voices, which the game reads before the shared
/// line), else the shared [pc] voices.
pub fn japanese_set(root: &Path) -> bool {
    let path = settings_file(root);
    let own = crate::layout::active(root).and_then(|v| crate::layout::table_value(&path, &crate::layout::own_table(v), "voices"));
    own.or_else(|| crate::layout::pc_value(&path, "voices")).as_deref() == Some("japanese")
}

/// sets Voices in the settings file, keeping every other line as it is (a
/// missing file gets just this; Haiku fills in the rest). versions2: the
/// playing version's own, in a folder with games\<version>\.
pub fn set_japanese(root: &Path, on: bool) -> std::io::Result<()> {
    let path = settings_file(root);
    let value = if on { "japanese" } else { "english" };
    match crate::layout::active(root) {
        Some(version) => crate::layout::set_table_value(&path, &crate::layout::own_table(version), "voices", value),
        None => crate::layout::set_pc_value(&path, "voices", value),
    }
}

/// `--voices <dump> [--dlc <dlc>] [--installed <romfs>] [--out <data>]`:
/// checks the dump and takes its Japanese voices out, printing progress. The
/// installed game is data/romfs beside the setup unless --installed names
/// one; the layer goes in data/voices-ja unless --out names another data
/// folder. jpdlc: --dlc then takes the Japanese DLC's voices too.
pub fn command(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: --voices <dump> [--dlc <dlc>] [--installed <romfs>] [--out <data>]");
        return 2;
    };
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let data = option("--out").unwrap_or_else(|| crate::pipeline::Paths::new(crate::root()).data()); // versions: the playing version's
    let installed = option("--installed").unwrap_or_else(|| data.join("romfs"));
    crate::log::open(&crate::pipeline::Paths::new(crate::root()).log_file(), true);
    let dump = match check(Path::new(path)) {
        Ok(d) => d,
        Err(p) => {
            crate::log::line(format!("{}: {}", p.kind(), p.message()));
            return 1;
        }
    };
    crate::log::line(format!("voices: {}", dump.describe()));
    let progress: Shared = Arc::new(Mutex::new(Progress::default()));
    let cancel = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| extract(&dump, &installed, &data, &progress, &cancel));
        print_progress(&progress, &|| job.is_finished());
        job.join().unwrap()
    });
    match result {
        Ok(done) => {
            crate::log::line(format!("ok: {} files, {} MB, {} clips; {}", done.files, done.bytes / 1_000_000, done.clips, done.note));
            // jpdlc: and the DLC's
            if let Some(dlc) = option("--dlc") {
                return run_dlc(&dlc, &data);
            }
            0
        }
        Err(e) => {
            crate::log::line(format!("failed: {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_differing_only_in_version_are_the_same() {
        let mut a = vec![0u8; 64];
        a[..4].copy_from_slice(b"CSTM");
        let mut b = a.clone();
        b[9] = 1;
        assert!(same_stream(&a, &b));
        b[20] = 7;
        assert!(!same_stream(&a, &b));
        assert!(!wanted("sound/stream/STRM_MAP_A1.bcstm", Some(&a), &{
            let mut c = a.clone();
            c[9] = 3;
            c
        }));
        assert!(wanted("sound/stream/VOICE_X.aac", None, &a));
        assert!(wanted("sound/IRON15_sound.bcsar", Some(&a), &a));
        assert!(!wanted("GameData/GameData.bin.lz", None, &a));
        assert!(!wanted("m/@E/A001.bin.lz", None, &a));
        assert!(wanted("movie/A/s11_bgm.bin", Some(&a), &b));
        assert!(!wanted("movie/s11.moflex", None, &a));
        assert!(wanted("sound/stream_jp/VOICE_X.aac", None, &a));
        assert!(!wanted("sound/dspaudio.cdc", None, &a));
        assert!(!wanted("sound/DLC_000.bcsar", None, &a));
    }

    /// jpdlc: which DLC files the DLC voice layer takes.
    #[test]
    fn dlc_files_taken() {
        let mut a = vec![0u8; 64];
        a[..4].copy_from_slice(b"CSTM");
        let mut version = a.clone();
        version[9] = 1;
        let mut other = a.clone();
        other[30] = 5;
        let voice = "local/sound/stream/VOICE_DLC_000_CHROM_EVT_01.dspadpcm.bcstm";
        assert!(dlc_wanted(voice, &a, &other));
        assert!(!dlc_wanted(voice, &a, &a));
        assert!(!dlc_wanted("local/sound/stream/STRM_MAP_DLC_000_00.bcstm", &a, &version));
        assert!(dlc_wanted("local/sound/DLC_018.bcsar", &a, &other));
        assert!(dlc_wanted("local/movie/A/s11_bgm.bin", &a, &other));
        assert!(!dlc_wanted("local/movie/A/s11_sbt.bin", &a, &other));
        assert!(!dlc_wanted("local/m/E000.bin.lz", &a, &other));
        assert!(!dlc_wanted("local/GameData/Field/Btl_Gold.bin", &a, &other));
        assert!(!dlc_wanted("fst.bin", &a, &other));
        assert!(voice_stream(voice) && !narration_stream(voice));
        assert!(narration_stream("local/sound/stream/STRM_NRR_DLC_000_E035_01.dspadpcm.bcstm"));
        assert!(!narration_stream("local/sound/stream/STRM_NRR_DLC_000_OFF.dspadpcm.bcstm"));
        assert!(judge_dlc_title(JP_DLC_TITLE).is_ok() && judge_dlc_title(0x0004_0000_0012_DE00).is_err());
        assert!(judge_dlc_product(JP_DLC_TITLE, "CTR-M-BFXJ-00").is_ok() && judge_dlc_product(JP_DLC_TITLE, "CTR-M-AAAJ-00").is_err());
    }

    /// jpdlc: the fixed-key path for DLC end to end: a copy of the decrypted
    /// Japanese DLC with every content's RomFS encrypted with the fixed
    /// (zero) key and its flags and TMD hashes to match, read back by
    /// check_dlc and extract_dlc. Needs the DLC, a data folder with the US
    /// DLC installed, and room for a copy: JPDLC_CIA=<.cia> JPDLC_TEMP=<folder>
    /// JPDLC_DATA=<data with dlc/>, cargo test -- --ignored fixed_key_dlc
    #[test]
    #[ignore]
    fn fixed_key_dlc_reads_back() {
        use std::io::Write;
        let (Some(cia), Some(temp), Some(data)) = (std::env::var_os("JPDLC_CIA"), std::env::var_os("JPDLC_TEMP"), std::env::var_os("JPDLC_DATA")) else {
            return;
        };
        let temp = PathBuf::from(temp);
        std::fs::create_dir_all(&temp).unwrap();
        let copy = temp.join("fixed-key-dlc.cia");
        std::fs::copy(&cia, &copy).unwrap();
        {
            let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&copy).unwrap();
            let mut head = vec![0u8; 0x2020];
            file.read_exact(&mut head).unwrap();
            let align = |n: u64| n.next_multiple_of(0x40);
            let tmd_offset = align(align(align(0x2020) + u32_at(&head, 0x08) as u64) + u32_at(&head, 0x0C) as u64);
            let tmd_size = u32_at(&head, 0x10) as usize;
            let mut tmd = vec![0u8; tmd_size];
            file.seek(SeekFrom::Start(tmd_offset)).unwrap();
            file.read_exact(&mut tmd).unwrap();
            let body = dump::tmd_body(&tmd).unwrap();
            let (_, _, records) = dump::tmd_contents(&tmd).unwrap();
            let mut offset = align(tmd_offset + tmd_size as u64);
            let aes = Aes128::new(&[0; 16]);
            for (i, (_, _, _, size, _)) in records.iter().enumerate() {
                let mut content = vec![0u8; *size as usize];
                file.seek(SeekFrom::Start(offset)).unwrap();
                file.read_exact(&mut content).unwrap();
                let romfs = u32_at(&content, 0x1B0) as usize * MEDIA as usize;
                let romfs_size = u32_at(&content, 0x1B4) as usize * MEDIA as usize;
                if romfs_size > 0 {
                    content[0x18F] = (content[0x18F] & !0x04) | 0x01;
                    let counter = ncch_counter(&content[..0x200], 3, romfs as u64);
                    ctr_xor(&aes, &counter, 0, &mut content[romfs..romfs + romfs_size]);
                    file.seek(SeekFrom::Start(offset)).unwrap();
                    file.write_all(&content).unwrap();
                    let hash = Sha256::digest(&content);
                    tmd[body + 0xC4 + 64 * 0x24 + i * 0x30 + 0x10..][..32].copy_from_slice(&hash);
                }
                offset = align(offset + size);
            }
            file.seek(SeekFrom::Start(tmd_offset)).unwrap();
            file.write_all(&tmd).unwrap();
        }
        let checked = check_dlc(&copy).unwrap();
        assert!(checked.crypto.starts_with("fixed key"), "{}", checked.crypto);
        let progress: Shared = Arc::new(Mutex::new(Progress::default()));
        let done = extract_dlc(&checked, Path::new(&data), &progress, &AtomicBool::new(false));
        std::fs::remove_file(&copy).unwrap();
        let done = done.unwrap();
        eprintln!("fixed key DLC: {} files, {} bytes, {} clips, {} narration", done.files, done.bytes, done.clips, done.narration);
        assert!(done.files == 337 && done.clips == 281);
    }

    #[test]
    fn settings_line_is_set_and_kept() {
        let dir = std::env::temp_dir().join(format!("haiku-voices-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!japanese_set(&dir));
        set_japanese(&dir, true).unwrap();
        assert!(japanese_set(&dir));
        let file = settings_file(&dir);
        std::fs::write(&file, "title_id = 1\n\n[pc]\nwindow_mode = \"borderless\"\nvoices = \"japanese\"\n\n[pc.online]\nvoices = 3\n").unwrap();
        set_japanese(&dir, false).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("voices = \"english\"") && text.contains("window_mode = \"borderless\"") && text.contains("[pc.online]\nvoices = 3"));
        assert!(!japanese_set(&dir));
        std::fs::write(&file, "[pc]\nwindow_mode = \"borderless\"\n").unwrap();
        set_japanese(&dir, true).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[pc]\nvoices = \"japanese\"\nwindow_mode = \"borderless\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn versions_keep_their_own_voices() {
        let dir = std::env::temp_dir().join(format!("haiku-voices-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let data = crate::layout::dir(&dir, crate::layout::Version::Eur).join("data");
        std::fs::create_dir_all(&data).unwrap();
        for file in [zakuro_fs::datadir::NCCH_FILE, zakuro_fs::datadir::EXHEADER_FILE, zakuro_fs::datadir::EXEFS_FILE] {
            std::fs::write(data.join(file), b"").unwrap();
        }
        std::fs::write(data.join(zakuro_fs::datadir::MANIFEST_FILE), "title 000400000017A800\n").unwrap();
        let file = settings_file(&dir);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[pc]\nvoices = \"japanese\"\n").unwrap();
        // the shared line is the default
        assert!(japanese_set(&dir));
        set_japanese(&dir, false).unwrap();
        assert!(!japanese_set(&dir));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("[pc]\nvoices = \"japanese\"") && text.contains("[pc.per_version.eur]\nvoices = \"english\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// the fixed-key path end to end: a copy of a decrypted dump with its
    /// RomFS encrypted with the fixed (zero) key as a cartridge would have
    /// it, read back by check and extract. Needs the dump and room for a
    /// copy: JPVOICE_DUMP=<decrypted .3ds> JPVOICE_TEMP=<folder>
    /// JPVOICE_INSTALLED=<data/romfs>, cargo test -- --ignored fixed_key
    #[test]
    #[ignore]
    fn fixed_key_dump_reads_back() {
        let (Some(dump), Some(temp), Some(installed)) = (std::env::var_os("JPVOICE_DUMP"), std::env::var_os("JPVOICE_TEMP"), std::env::var_os("JPVOICE_INSTALLED")) else {
            return;
        };
        let temp = PathBuf::from(temp);
        std::fs::create_dir_all(&temp).unwrap();
        let copy = temp.join("fixed-key.3ds");
        std::fs::copy(&dump, &copy).unwrap();
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&copy).unwrap();
            let mut head = vec![0u8; 0x200];
            file.read_exact(&mut head).unwrap();
            let ncch = u32_at(&head, 0x120) as u64 * MEDIA;
            file.seek(SeekFrom::Start(ncch)).unwrap();
            let mut header = vec![0u8; 0x200];
            file.read_exact(&mut header).unwrap();
            let romfs = ncch + u32_at(&header, 0x1B0) as u64 * MEDIA;
            let size = u32_at(&header, 0x1B4) as u64 * MEDIA;
            // fixed key, not NoCrypto
            header[0x18F] = (header[0x18F] & !0x04) | 0x01;
            file.seek(SeekFrom::Start(ncch)).unwrap();
            file.write_all(&header).unwrap();
            let counter = ncch_counter(&header, 3, romfs - ncch);
            let aes = Aes128::new(&[0; 16]);
            let mut at = 0u64;
            let mut buf = vec![0u8; 16 << 20];
            while at < size {
                let n = (size - at).min(buf.len() as u64) as usize;
                file.seek(SeekFrom::Start(romfs + at)).unwrap();
                file.read_exact(&mut buf[..n]).unwrap();
                ctr_xor(&aes, &counter, at / 16, &mut buf[..n]);
                file.seek(SeekFrom::Start(romfs + at)).unwrap();
                file.write_all(&buf[..n]).unwrap();
                at += n as u64;
            }
        }
        let mut raw = std::fs::File::open(&copy).unwrap();
        let mut probe = vec![0u8; 0x400000];
        raw.read_exact(&mut probe).unwrap();
        let checked = check(&copy).unwrap();
        assert!(checked.crypto.starts_with("fixed key"), "{}", checked.crypto);
        let out = temp.join("data");
        let progress: Shared = Arc::new(Mutex::new(Progress::default()));
        let done = extract(&checked, Path::new(&installed), &out, &progress, &AtomicBool::new(false)).unwrap();
        eprintln!("fixed key: {} files, {} bytes, {} clips; {}", done.files, done.bytes, done.clips, done.note);
        assert!(done.files > 3000 && done.clips > 2900);
        std::fs::remove_file(&copy).unwrap();
    }
}
