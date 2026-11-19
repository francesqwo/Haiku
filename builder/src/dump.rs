//! Telling what a file the player picked is: the game Haiku is made for
//! (Fire Emblem Fates Special Edition, USA: title 0004000000179800,
//! CTR-P-BFZE), decrypted and whole, or what is wrong with it.
//!
//! Every check reads the dump's own headers and hashes (the NCCH header's
//! hashes of the exheader, ExeFS and RomFS superblocks, the ExeFS's of
//! .code), and .code against the one Haiku's code was recompiled from.
//! `verify_romfs` then checks the whole RomFS against its IVFC hash tree.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const TITLE_ID: u64 = 0x0004_0000_0017_9800;
pub const PRODUCT: &str = "CTR-P-BFZE";
/// SHA-256 of the ExeFS .code (as stored, compressed) of the Special
/// Edition cartridge, the code Haiku's overrides are written against.
pub const CODE_SHA256: &str = "5d5c05332236343b9c00bb8fb958afe0e467f0fdfaa1a154e928b50b9744882c";
pub const MEDIA: u64 = 0x200;

/// what is wrong with a file, in words for the player.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// not readable at all.
    Unreadable(String),
    /// not a 3DS game.
    NotADump(String),
    /// an update or DLC where the game was asked for (or the other way).
    WrongKind(String),
    /// a different game.
    WrongGame(String),
    /// Fates, but another region.
    WrongRegion(String),
    /// Fates, USA, but a single-route cartridge (Birthright or Conquest).
    SingleRoute(String),
    /// the right title with different code.
    WrongVersion(String),
    /// still encrypted.
    Encrypted(String),
    /// damaged or incomplete.
    Corrupt(String),
}

impl Problem {
    pub fn message(&self) -> &str {
        match self {
            Problem::Unreadable(m)
            | Problem::NotADump(m)
            | Problem::WrongKind(m)
            | Problem::WrongGame(m)
            | Problem::WrongRegion(m)
            | Problem::SingleRoute(m)
            | Problem::WrongVersion(m)
            | Problem::Encrypted(m)
            | Problem::Corrupt(m) => m,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Problem::Unreadable(_) => "unreadable",
            Problem::NotADump(_) => "not a dump",
            Problem::WrongKind(_) => "wrong kind",
            Problem::WrongGame(_) => "wrong game",
            Problem::WrongRegion(_) => "wrong region",
            Problem::SingleRoute(_) => "single route",
            Problem::WrongVersion(_) => "wrong version",
            Problem::Encrypted(_) => "encrypted",
            Problem::Corrupt(_) => "corrupt",
        }
    }
}

pub const DECRYPT_HINT: &str = "Decrypt it on your 3DS with GodMode9 (select the file, then \"NCSD image options\" or \"CIA image options\" > \"Decrypt file\"), or dump it again as a decrypted .3ds, and select the decrypted file. Haiku never needs or asks for console keys.";

/// the game dump the player picked, checked.
#[derive(Debug, Clone)]
pub struct Game {
    /// the file the recompiler and the installer read.
    pub path: PathBuf,
    /// what kind of file: "cartridge image (.3ds)", "CIA", "CXI".
    pub kind: &'static str,
    pub size: u64,
    pub ncch_offset: u64,
    pub remaster: u16,
    /// the IVFC tree of the RomFS, for verify_romfs.
    pub romfs_offset: u64,
    pub romfs_size: u64,
    /// eurdlc: which game it is (the DLC picked must fit it)
    pub region: crate::jpn::GameRegion,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(b[at..at + 2].try_into().unwrap())
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

pub struct Dump {
    file: File,
    pub len: u64,
}

impl Dump {
    pub fn open(path: &Path) -> Result<Dump, Problem> {
        let file = File::open(path).map_err(|e| Problem::Unreadable(format!("The file can't be opened ({e}).")))?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Dump { file, len })
    }
    pub fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, Problem> {
        if offset.checked_add(len as u64).is_none_or(|end| end > self.len) {
            return Err(Problem::Corrupt(
                "The file is shorter than its own header says: it is an incomplete copy or download. Copy or dump it again.".into(),
            ));
        }
        let mut bytes = vec![0; len];
        self.file.seek(SeekFrom::Start(offset)).map_err(|e| Problem::Unreadable(format!("The file can't be read ({e}).")))?;
        self.file.read_exact(&mut bytes).map_err(|e| Problem::Unreadable(format!("The file can't be read ({e}).")))?;
        Ok(bytes)
    }
}

fn digest(dump: &mut Dump, offset: u64, len: u64) -> Result<[u8; 32], Problem> {
    let mut hasher = Sha256::new();
    let mut at = offset;
    while at < offset + len {
        let n = (offset + len - at).min(8 << 20) as usize;
        hasher.update(dump.read(at, n)?);
        at += n as u64;
    }
    Ok(hasher.finalize().into())
}

/// a TMD's signature size, by its type.
pub fn tmd_body(tmd: &[u8]) -> Option<usize> {
    Some(match be32(tmd.get(..4)?, 0) & 0xFFFF {
        0x0000 | 0x0003 => 4 + 0x200 + 0x3C,
        0x0001 | 0x0004 => 4 + 0x100 + 0x3C,
        0x0002 | 0x0005 => 4 + 0x3C + 0x40,
        _ => return None,
    })
}

/// a TMD's content records: index, id, flags, size, hash.
pub fn tmd_contents(tmd: &[u8]) -> Option<(u64, u16, Vec<(u16, u32, u16, u64, [u8; 32])>)> {
    let body = tmd_body(tmd)?;
    let title = be64(tmd.get(body..body + 0xA0)?, 0x4C);
    let version = be16(tmd, body + 0x9C);
    let count = be16(tmd, body + 0x9E) as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let at = body + 0xC4 + 64 * 0x24 + i * 0x30;
        let r = tmd.get(at..at + 0x30)?;
        out.push((be16(r, 4), be32(r, 0), be16(r, 6), be64(r, 8), r[0x10..0x30].try_into().ok()?));
    }
    Some((title, version, out))
}

fn region(letter: char) -> &'static str {
    match letter {
        'E' => "North America",
        'P' => "Europe and Australia",
        'J' => "Japan",
        'K' => "Korea",
        'C' | 'T' => "China or Taiwan",
        _ => "another region",
    }
}

/// what an NCCH's title says it is, a problem when it is not Haiku's game.
pub fn judge_title(program_id: u64, product: &str, wanted_game: bool) -> Result<(), Problem> {
    let high = program_id >> 32;
    if wanted_game {
        match high {
            0x0004_000E => {
                return Err(Problem::WrongKind(
                    "This is an update, not the game. Select the game itself: the Special Edition cartridge's .3ds dump (or its .cia).".into(),
                ))
            }
            0x0004_008C => return Err(Problem::WrongKind("This is downloadable content. Select the game here; the DLC comes in the next step.".into())),
            _ => {}
        }
    }
    if program_id == TITLE_ID && product == PRODUCT {
        return Ok(());
    }
    // eur: the EUR Special Edition is built with its own address map
    if crate::eur::is_eur(program_id, product) {
        return Ok(());
    }
    // jpprep: so is a Japanese build (Fire Emblem if), when this Haiku has
    // the map for its code (checked with the code's hash, check_game)
    if crate::jpn::is_jpn(program_id, product) {
        return Ok(());
    }
    if product.starts_with("CTR-P-BF") && product.len() == 10 {
        let letter = product.chars().last().unwrap_or('?');
        // regions: Haiku runs the USA, EUR and Japanese Special Editions, so
        // only another region is a wrong one; a single-route dump of these
        // three is told which Special Edition it needs
        let (name, edition) = match letter {
            'E' => ("US", "CTR-P-BFZE"),
            'P' => ("European", "CTR-P-BFZP"),
            'J' => ("Japanese", "CTR-P-BFZJ"),
            _ => {
                return Err(Problem::WrongRegion(format!(
                    "This is Fire Emblem Fates for {} ({product}). Haiku runs the US, European and Japanese Special Editions (CTR-P-BFZE, CTR-P-BFZP and CTR-P-BFZJ).",
                    region(letter)
                )))
            }
        };
        return Err(Problem::SingleRoute(format!(
            "This is a {name} Fire Emblem Fates dump ({product}), but not the Special Edition: Birthright and Conquest, on cartridge or from the eShop, hold one route. Haiku needs the {name} Special Edition cartridge ({edition}), which has all three routes on it."
        )));
    }
    Err(Problem::WrongGame(format!(
        "This is a different game ({}, title {program_id:016X}), not Fire Emblem Fates Special Edition.",
        if product.trim().is_empty() { "no product code" } else { product }
    )))
}

fn archive_problem(head: &[u8]) -> Option<Problem> {
    let what = if head.starts_with(b"PK\x03\x04") {
        "a .zip archive"
    } else if head.starts_with(b"7z\xBC\xAF\x27\x1C") {
        "a .7z archive"
    } else if head.starts_with(b"Rar!") {
        "a .rar archive"
    } else {
        return None;
    };
    Some(Problem::NotADump(format!("This is {what}. Extract the dump from it first, then select the .3ds or .cia inside.")))
}

/// checks the game dump at path (a file, or a folder Citra or Azahar
/// installed it in).
pub fn check_game(path: &Path) -> Result<Game, Problem> {
    let path = if path.is_dir() { find_installed_game(path)? } else { path.to_path_buf() };
    let mut dump = Dump::open(&path)?;
    if dump.len == 0 {
        return Err(Problem::NotADump("The file is empty.".into()));
    }
    if let Some(problem) = archive_problem(&dump.read(0, dump.len.min(8) as usize)?) {
        return Err(problem);
    }
    if dump.len < 0x4000 {
        return Err(Problem::NotADump("This file is far too small to be a 3DS game.".into()));
    }
    let head = dump.read(0, 0x200)?;
    let (kind, ncch_offset) = if &head[0x100..0x104] == b"NCSD" {
        ("cartridge image (.3ds)", u32_at(&head, 0x120) as u64 * MEDIA)
    } else if &head[0x100..0x104] == b"NCCH" {
        ("executable (.cxi)", 0)
    } else if u32_at(&head, 0) == 0x2020 {
        ("CIA", cia_game(&mut dump)?)
    } else {
        return Err(Problem::NotADump("This is not a 3DS game dump. Select a decrypted .3ds (.cci), .cia or .cxi file.".into()));
    };
    let header = dump.read(ncch_offset, 0x200)?;
    if &header[0x100..0x104] != b"NCCH" {
        return Err(Problem::Corrupt("The game's partition has no valid header. The dump is damaged or incomplete; dump it again.".into()));
    }
    let program_id = u64_at(&header, 0x118);
    let product = String::from_utf8_lossy(&header[0x150..0x160]).trim_end_matches('\0').to_string();
    judge_title(program_id, &product, true)?;
    // encryption: NoCrypto, or the ExeFS readable as it is (some
    // decryptors leave the flags as they were)
    let flags = &header[0x188..0x190];
    let exefs_offset = ncch_offset + u32_at(&header, 0x1A0) as u64 * MEDIA;
    let exefs_size = u32_at(&header, 0x1A4) as u64 * MEDIA;
    let exefs_header = dump.read(exefs_offset, 0x200)?;
    let no_crypto = flags[7] & 0x04 != 0;
    if !no_crypto && &exefs_header[..8] != b".code\0\0\0" {
        return Err(Problem::Encrypted(format!("This is the right game, but the dump is encrypted. {DECRYPT_HINT}")));
    }
    let content_size = u32_at(&header, 0x104) as u64 * MEDIA;
    if ncch_offset + content_size > dump.len {
        return Err(Problem::Corrupt(
            "The file is shorter than its own header says: it is an incomplete copy or download. Copy or dump it again.".into(),
        ));
    }
    // the exheader, the ExeFS and the RomFS superblock, by the header's hashes
    let exheader_size = u32_at(&header, 0x180) as u64;
    if exheader_size == 0 || digest(&mut dump, ncch_offset + 0x200, exheader_size)? != header[0x160..0x180] {
        return Err(Problem::Corrupt("The game's extended header doesn't match its hash: the dump is damaged. Dump it again.".into()));
    }
    let exefs_hash_size = u32_at(&header, 0x1A8) as u64 * MEDIA;
    if exefs_size == 0 || digest(&mut dump, exefs_offset, exefs_hash_size)? != header[0x1C0..0x1E0] {
        return Err(Problem::Corrupt("The game's ExeFS doesn't match its hash: the dump is damaged. Dump it again.".into()));
    }
    let code = (0..10)
        .map(|i| &exefs_header[i * 16..i * 16 + 16])
        .position(|entry| &entry[..8] == b".code\0\0\0")
        .ok_or_else(|| Problem::Corrupt("The game's ExeFS has no code. The dump is damaged; dump it again.".into()))?;
    let entry = &exefs_header[code * 16..code * 16 + 16];
    let (code_offset, code_size) = (u32_at(entry, 8) as u64, u32_at(entry, 12) as u64);
    let code_hash = digest(&mut dump, exefs_offset + 0x200 + code_offset, code_size)?;
    let listed = &exefs_header[0x200 - 32 * (code + 1)..0x200 - 32 * code];
    if code_hash != listed {
        return Err(Problem::Corrupt("The game's code doesn't match its hash: the dump is damaged. Dump it again.".into()));
    }
    let hex: String = code_hash.iter().map(|b| format!("{b:02x}")).collect();
    let exheader = dump.read(ncch_offset + 0x200, 0x400)?;
    let remaster = u16_at(&exheader, 0x0E);
    // eur: the EUR cartridge's code is the one its map is made for
    let eur = crate::eur::is_eur(program_id, &product);
    // jpprep: a Japanese build's code must be the one this Haiku's map is
    // made for (setup/jpn/build.txt), said before anything is installed
    let jpn = crate::jpn::is_jpn(program_id, &product);
    if jpn {
        crate::jpn::check_code(&crate::jpn::setup_dir(), program_id, &product, &hex)?;
    }
    if !jpn && ((!eur && hex != CODE_SHA256) || (eur && hex != crate::eur::CODE_SHA256)) {
        return Err(Problem::WrongVersion(format!(
            "This is Fire Emblem Fates Special Edition ({}), but its code is not the version Haiku is made for (it reports version {remaster}). The dump may have an update or a code mod merged into it, or come from a different printing. Dump the original Special Edition cartridge.",
            if eur { "Europe" } else { "USA" } // regions
        )));
    }
    let romfs_offset = ncch_offset + u32_at(&header, 0x1B0) as u64 * MEDIA;
    let romfs_size = u32_at(&header, 0x1B4) as u64 * MEDIA;
    let romfs_hash_size = u32_at(&header, 0x1B8) as u64 * MEDIA;
    if romfs_size == 0 || flags[7] & 0x02 != 0 {
        return Err(Problem::Corrupt("The dump has no RomFS (the game's data). It is incomplete; dump it again.".into()));
    }
    if digest(&mut dump, romfs_offset, romfs_hash_size)? != header[0x1E0..0x200] {
        return Err(Problem::Corrupt("The game's RomFS doesn't match its hash: the dump is damaged. Dump it again.".into()));
    }
    if dump.read(romfs_offset, 4)? != b"IVFC" {
        return Err(Problem::Encrypted(format!("The game's data (RomFS) is still encrypted. {DECRYPT_HINT}")));
    }
    let region = if jpn { crate::jpn::GameRegion::Jpn } else if eur { crate::jpn::GameRegion::Eur } else { crate::jpn::GameRegion::Usa }; // eurdlc
    Ok(Game { path, kind, size: dump.len, ncch_offset, remaster, romfs_offset, romfs_size, region })
}

/// a CIA's executable content: where it starts, after checking it is the
/// game, not an update or DLC, and not encrypted.
fn cia_game(dump: &mut Dump) -> Result<u64, Problem> {
    let header = dump.read(0, 0x2020)?;
    let align = |n: u64| n.next_multiple_of(0x40);
    let tmd_offset = align(align(align(0x2020) + u32_at(&header, 0x08) as u64) + u32_at(&header, 0x0C) as u64);
    let tmd_size = u32_at(&header, 0x10) as usize;
    let tmd = dump.read(tmd_offset, tmd_size)?;
    let (title, _, contents) =
        tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The CIA's title metadata can't be read: the file is damaged.".into()))?;
    match title >> 32 {
        0x0004_000E => {
            return Err(Problem::WrongKind(
                "This CIA is an update, not the game. Select the game itself: the Special Edition's .3ds dump (or its .cia).".into(),
            ))
        }
        0x0004_008C => return Err(Problem::WrongKind("This CIA is downloadable content. Select the game here; the DLC comes in the next step.".into())),
        _ => {}
    }
    let mut offset = align(tmd_offset + tmd_size as u64);
    for (index, _, flags, size, _) in contents {
        let present = header[0x20 + index as usize / 8] & (0x80 >> (index % 8)) != 0;
        if !present {
            continue;
        }
        if index == 0 {
            if flags & 1 != 0 {
                return Err(Problem::Encrypted(format!("This CIA is encrypted (it still has the eShop's title key encryption). {DECRYPT_HINT}")));
            }
            return Ok(offset);
        }
        offset = align(offset + size);
    }
    Err(Problem::Corrupt("The CIA has no game in it.".into()))
}

/// the game's executable in a folder Citra or Azahar installed it in
/// (sdmc/Nintendo 3DS/<id>/<id>/title/00040000/00179800/content), or a
/// folder holding a dump.
/// regions: the editions an emulator's sdmc folder is searched for (title
/// low words, as Citra and Azahar name the folders), USA first, then EUR, then
/// the Japanese Special Edition and single-route cartridges.
const INSTALLED: [&str; 5] = ["00179800", "0017a800", "0012de00", "0012dc00", "0012dd00"];

fn find_installed_game(dir: &Path) -> Result<PathBuf, Problem> {
    let mut apps = Vec::new();
    let mut dumps = Vec::new();
    find_files(dir, 0, &mut |path| {
        let lower = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        // regions: any of the editions Haiku builds, in INSTALLED's order
        if let Some(rank) = (ext == "app").then(|| INSTALLED.iter().position(|id| lower.contains(&format!("/00040000/{id}/content")))).flatten() {
            apps.push((rank, path.to_path_buf()));
        } else if ["3ds", "cci", "cia", "cxi"].contains(&ext.as_str()) {
            dumps.push(path.to_path_buf());
        }
    });
    // the first edition installed; its executable content is the largest of
    // the title's
    apps.sort_by_key(|(rank, p)| (*rank, std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))));
    if let Some((_, app)) = apps.first() {
        return Ok(app.clone());
    }
    // a single dump in the folder
    let fitting: Vec<PathBuf> = dumps.into_iter().filter(|p| check_game(p).is_ok()).collect();
    if let [one] = fitting.as_slice() {
        return Ok(one.clone());
    }
    Err(Problem::NotADump(
        "This folder doesn't hold the game. Select the dump file itself (.3ds, .cci, .cia or .cxi), or Citra's or Azahar's sdmc folder with the game installed.".into(),
    ))
}

pub fn find_files(dir: &Path, depth: usize, found: &mut dyn FnMut(&Path)) {
    if depth > 9 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_files(&path, depth + 1, found);
        } else {
            found(&path);
        }
    }
}

/// checks the whole RomFS against its IVFC hash tree (the master hash the
/// header's own hash covers, then levels 1, 2 and 3), telling progress the
/// fraction done. a damaged byte anywhere in the game's data fails it.
pub fn verify_romfs(game: &Game, progress: &dyn Fn(f32)) -> Result<(), Problem> {
    let mut dump = Dump::open(&game.path)?;
    let base = game.romfs_offset;
    let ivfc = dump.read(base, 0x60)?;
    let master_size = u32_at(&ivfc, 0x08) as u64;
    let level = |i: usize| {
        let at = 0x0C + i * 0x18;
        (u64_at(&ivfc, at + 8), u32_at(&ivfc, at + 0x10))
    };
    let (sizes, logs): (Vec<u64>, Vec<u32>) = (0..3).map(level).unzip();
    if logs.iter().any(|&l| !(9..=24).contains(&l)) {
        return Err(Problem::Corrupt("The game's RomFS has a damaged hash tree. Dump it again.".into()));
    }
    let master = dump.read(base + 0x60, master_size as usize)?;
    // level 3 first, after the header and master hash; then levels 1 and 2
    let l3 = base + (0x60 + master_size).next_multiple_of(1 << logs[2]);
    let l1 = (l3 + sizes[2]).next_multiple_of(1 << logs[0]);
    let l2 = (l1 + sizes[0]).next_multiple_of(1 << logs[1]);
    let starts = [l1, l2, l3];
    let total: u64 = sizes.iter().sum();
    let mut done = 0u64;
    let mut hashes = master;
    for i in 0..3 {
        let block = 1u64 << logs[i];
        let blocks = sizes[i].div_ceil(block);
        if (hashes.len() as u64) < blocks * 32 {
            return Err(Problem::Corrupt("The game's RomFS has a damaged hash tree. Dump it again.".into()));
        }
        let mut level_bytes = if i < 2 { Vec::with_capacity(sizes[i] as usize) } else { Vec::new() };
        let chunk = (block * (64 << 20).max(block) / block).max(block);
        let mut at = 0u64;
        while at < sizes[i] {
            let n = chunk.min(blocks * block - at);
            let len = n.min(dump.len.saturating_sub(starts[i] + at));
            let mut data = dump.read(starts[i] + at, len as usize)?;
            data.resize(n as usize, 0);
            for (k, piece) in data.chunks(block as usize).enumerate() {
                let index = ((at / block) as usize) + k;
                // the last block is hashed only as far as the level goes,
                // padded with zeros to the block size
                let wanted = &hashes[index * 32..index * 32 + 32];
                let valid = (sizes[i] - (at + k as u64 * block)).min(block) as usize;
                let mut padded;
                let piece = if valid < piece.len() {
                    padded = piece[..valid].to_vec();
                    padded.resize(block as usize, 0);
                    &padded[..]
                } else {
                    piece
                };
                if Sha256::digest(piece).as_slice() != wanted {
                    return Err(Problem::Corrupt(format!(
                        "The game's data is damaged (RomFS level {} block {index} doesn't match its hash). Copy or dump it again.",
                        i + 1
                    )));
                }
            }
            if i < 2 {
                level_bytes.extend_from_slice(&data[..(n.min(sizes[i] - at)) as usize]);
            }
            at += n;
            done += n.min(sizes[i]);
            progress((done as f32 / total as f32).min(1.0));
        }
        if i < 2 {
            hashes = level_bytes;
        }
    }
    Ok(())
}

#[cfg(test)]
mod regions_tests {
    use super::*;

    fn says(product: &str) -> String {
        judge_title(0x0004_0000_0000_0001, product, true).unwrap_err().message().to_owned()
    }

    #[test]
    fn every_edition_haiku_builds_is_accepted() {
        assert!(judge_title(TITLE_ID, PRODUCT, true).is_ok());
        assert!(judge_title(crate::eur::TITLE_ID, crate::eur::PRODUCT, true).is_ok());
        assert!(judge_title(crate::jpn::SPECIAL_EDITION, "CTR-P-BFZJ", true).is_ok());
    }

    #[test]
    fn no_message_says_usa_only() {
        for product in ["CTR-P-BFXP", "CTR-P-BFWE", "CTR-P-BFZK", "CTR-P-BFZC", "CTR-P-ABCD", ""] {
            let message = says(product);
            assert!(!message.contains("runs only"), "{product}: {message}");
            assert!(!message.contains("(USA)"), "{product}: {message}");
        }
        assert!(says("CTR-P-BFXP").contains("European Special Edition cartridge (CTR-P-BFZP)"));
        assert!(says("CTR-P-BFXE").contains("US Special Edition cartridge (CTR-P-BFZE)"));
        assert!(says("CTR-P-BFZK").contains("US, European and Japanese"));
    }

    #[test]
    fn emulator_folders_of_every_edition_are_found() {
        let root = std::env::temp_dir().join(format!("haiku-regions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for id in ["0017a800", "0012de00"] {
            let content = root.join(id).join("sdmc/Nintendo 3DS/0/0/title/00040000").join(id).join("content");
            std::fs::create_dir_all(&content).unwrap();
            std::fs::write(content.join("00000000.app"), b"").unwrap();
            // find_installed_game hands the content on; check_game reads it
            assert_eq!(find_installed_game(&root.join(id)).unwrap(), content.join("00000000.app"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
