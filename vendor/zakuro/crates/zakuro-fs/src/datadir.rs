//! os: a game installed as a plain folder instead of a ROM image.
//!
//! The PC port reads the game's data from `data/` next to the executable,
//! extracted once from the user's own dump by `fates-pc --install`:
//!
//! ```text
//! data/
//!   ncch.bin        the NCCH header (0x200 bytes): title id, product code
//!   exheader.bin    the extended header (0x800 bytes): memory layout
//!   exefs.bin       the ExeFS: .code (for .rodata/.data), icon, banner
//!   romfs/          every RomFS file, as ordinary files and folders
//!   dlc/<title id>/ downloadable content
//!     dlc.txt       title id, version and the TMD's content records
//!     <index>/romfs/  each content's RomFS, by content index (4 hex digits)
//!   install.txt     what was installed, from where, and the check results
//! ```
//!
//! Nothing reads the ROM image at run time. The RomFS folder is read file
//! by file, through [`crate::layered::Layered::from_dirs`] for the SDK's
//! own RomFS reader, or directly by path by the native nn::fs layer.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::layered::Layered;
use crate::romfs::{DirEntry, RomFs};
use crate::{Cia, Content, Dlc, ExHeader, ExeFs, FsError, NcchHeader, Ncsd, RomImage, Title, TitleKind};

pub const NCCH_FILE: &str = "ncch.bin";
pub const EXHEADER_FILE: &str = "exheader.bin";
pub const EXEFS_FILE: &str = "exefs.bin";
pub const ROMFS_DIR: &str = "romfs";
pub const DLC_DIR: &str = "dlc";
pub const DLC_FILE: &str = "dlc.txt";
pub const MANIFEST_FILE: &str = "install.txt";
/// every file of a RomFS folder and its size, beside the folder
/// (romfs.tsv for romfs/), so the game starts without listing 645 folders.
pub const INDEX_SUFFIX: &str = ".tsv";

/// whether path is a game installed as a folder.
pub fn is_game_dir(path: &Path) -> bool {
    path.is_dir() && path.join(NCCH_FILE).is_file() && path.join(EXHEADER_FILE).is_file() && path.join(EXEFS_FILE).is_file()
}

/// the game installed in dir.
pub fn title(dir: &Path) -> Result<Title, FsError> {
    let ncch_raw = std::fs::read(dir.join(NCCH_FILE))?;
    let exheader_raw = std::fs::read(dir.join(EXHEADER_FILE))?;
    let exefs_raw = std::fs::read(dir.join(EXEFS_FILE))?;
    if ncch_raw.len() < NcchHeader::SIZE || exheader_raw.len() < ExHeader::FULL_SIZE {
        return Err(FsError::Truncated { offset: 0, len: ncch_raw.len().min(exheader_raw.len()) });
    }
    // the container bytes the rest of the runtime reads through a Title:
    // header, exheader, then the ExeFS
    let exefs_offset = (NcchHeader::SIZE + ExHeader::FULL_SIZE) as u64;
    let mut bytes = Vec::with_capacity(exefs_offset as usize + exefs_raw.len());
    bytes.extend_from_slice(&ncch_raw[..NcchHeader::SIZE]);
    bytes.extend_from_slice(&exheader_raw[..ExHeader::FULL_SIZE]);
    bytes.extend_from_slice(&exefs_raw);
    let image = Arc::new(RomImage::from_bytes(dir, &bytes)?);
    let ncch = NcchHeader::parse(&ncch_raw)?;
    let exheader = ExHeader::parse(&exheader_raw[..ExHeader::HASHED_SIZE])?;
    let exefs = ExeFs::parse(&exefs_raw)?;
    let romfs_dir = dir.join(ROMFS_DIR);
    let layered = if romfs_dir.is_dir() {
        Some(romfs_folder(&romfs_dir)?)
    } else {
        log::warn!("os: {} has no romfs folder", dir.display());
        None
    };
    Ok(Title {
        image,
        ncch_offset: 0,
        ncch,
        exheader,
        exefs,
        exefs_offset,
        romfs: None,
        layered,
        update: None,
        romfs_dir: Some(romfs_dir),
    })
}

/// the RomFS image of a folder, built once per run: the frontend loads a
/// title more than once while it starts it.
fn romfs_folder(romfs_dir: &Path) -> Result<Arc<Layered>, FsError> {
    static BUILT: std::sync::Mutex<Vec<(PathBuf, Arc<Layered>)>> = std::sync::Mutex::new(Vec::new());
    if let Some((_, layered)) = BUILT.lock().unwrap().iter().find(|(at, _)| at == romfs_dir) {
        return Ok(layered.clone());
    }
    let started = std::time::Instant::now();
    let list = folder_list(romfs_dir)?;
    let layered = Arc::new(Layered::from_list(romfs_dir, &list));
    log::info!("os: RomFS from the folder {}, {} files, {} ms", romfs_dir.display(), layered.file_count(), started.elapsed().as_millis());
    BUILT.lock().unwrap().push((romfs_dir.to_path_buf(), layered.clone()));
    Ok(layered)
}

/// the files of a RomFS folder and their sizes, from its index, or listed
/// once and the index kept for the next start.
pub fn folder_list(romfs_dir: &Path) -> Result<Vec<(String, u64)>, FsError> {
    if let Some(list) = read_index(romfs_dir) {
        return Ok(list);
    }
    let mut list = Vec::new();
    scan(romfs_dir, "", &mut list)?;
    list.sort();
    if let Err(error) = write_index(romfs_dir, &list) {
        log::warn!("os: could not write the index of {}, {error}", romfs_dir.display());
    }
    Ok(list)
}

/// mods: the RomFS image of a folder with mods' content folders over it,
/// in order (see Layered::from_list_with_mods). Built each time: only a
/// game started with mods asks for it.
pub fn romfs_folder_with_mods(romfs_dir: &Path, mods: &[&Path]) -> Result<Layered, FsError> {
    let list = folder_list(romfs_dir)?;
    Layered::from_list_with_mods(romfs_dir, &list, mods)
}

/// the files under dir, their paths from the RomFS folder and sizes.
fn scan(dir: &Path, prefix: &str, list: &mut Vec<(String, u64)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            scan(&entry.path(), &path, list)?;
        } else {
            list.push((path, metadata.len()));
        }
    }
    Ok(())
}

/// where the index of a RomFS folder is.
fn index_path(romfs_dir: &Path) -> PathBuf {
    let mut name = romfs_dir.file_name().unwrap_or_default().to_os_string();
    name.push(INDEX_SUFFIX);
    romfs_dir.with_file_name(name)
}

/// the files the index of a RomFS folder lists, when it has one that is
/// complete: the first line is the file count.
/// eurdlc: also for eur::adapt_dlc, which reads what a DLC content holds.
pub(crate) fn read_index(romfs_dir: &Path) -> Option<Vec<(String, u64)>> {
    let text = std::fs::read_to_string(index_path(romfs_dir)).ok()?;
    let mut lines = text.lines();
    let count: usize = lines.next()?.strip_prefix("files ")?.parse().ok()?;
    let list: Vec<(String, u64)> = lines
        .filter_map(|line| {
            let (size, path) = line.split_once('\t')?;
            Some((path.to_owned(), size.parse().ok()?))
        })
        .collect();
    (list.len() == count).then_some(list)
}

/// writes the index of a RomFS folder from the files extracted to it.
/// eurdlc: also for eur::adapt_dlc, which adds the files it copies.
pub(crate) fn write_index(romfs_dir: &Path, list: &[(String, u64)]) -> std::io::Result<()> {
    let mut text = format!("files {}\n", list.len());
    for (path, size) in list {
        text += &format!("{size}\t{path}\n");
    }
    std::fs::write(index_path(romfs_dir), text)
}

/// the DLC installed in dir, data/dlc/<title id>.
pub fn dlc(dir: &Path) -> Result<Dlc, FsError> {
    let text = std::fs::read_to_string(dir.join(DLC_FILE))?;
    let (mut title_id, mut version, mut contents) = (0u64, 0u16, Vec::new());
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["title", id] => title_id = u64::from_str_radix(id, 16).unwrap_or(0),
            ["version", v] => version = v.parse().unwrap_or(0),
            ["content", index, id, flags, size] => {
                let index = u16::from_str_radix(index, 16).unwrap_or(0);
                let held = dir.join(format!("{index:04X}")).is_dir();
                contents.push(Content {
                    index,
                    id: u32::from_str_radix(id, 16).unwrap_or(0),
                    // installed contents are plaintext
                    flags: u16::from_str_radix(flags, 16).unwrap_or(0) & !1,
                    size: size.parse().unwrap_or(0),
                    // an installed content has a place, which marks it held
                    offset: held.then_some(0),
                });
            }
            _ => {}
        }
    }
    if title_id == 0 {
        return Err(FsError::BadCia("dlc.txt names no title"));
    }
    let mut folders = Vec::new();
    for content in &contents {
        let romfs = dir.join(format!("{:04X}", content.index)).join(ROMFS_DIR);
        if romfs.is_dir() {
            folders.push((content.index, romfs_folder(&romfs)?));
        }
    }
    Ok(Dlc { image: Arc::new(RomImage::from_bytes(dir, &[])?), title_id, version, contents, folders })
}

/// a progress line for the installer.
fn say(line: impl AsRef<str>) {
    println!("{}", line.as_ref());
    let _ = std::io::stdout().flush();
}

/// the executable NCCH of a cartridge dump or CIA, and the image.
fn open_game(rom: &Path) -> Result<(Arc<RomImage>, u64), FsError> {
    let image = Arc::new(RomImage::open(rom)?);
    let data = image.data();
    let offset = if Ncsd::detect(data) {
        Ncsd::parse(data)?.executable_partition()?.offset
    } else if Cia::detect(data) {
        Cia::parse(data)?.executable().ok_or(FsError::NoExecutablePartition)?.0
    } else if NcchHeader::detect(data) {
        0
    } else {
        return Err(FsError::UnknownFormat);
    };
    Ok((image, offset))
}

/// builder: how many files the installer has extracted (or checked) in
/// this process, for a progress bar.
pub static EXTRACTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// totals of an extraction or a check.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tally {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
    /// files missing, a different size, or different bytes.
    pub bad: usize,
}

/// walks a RomFS, calling found with each file's path and its bytes.
fn walk(romfs: &RomFs, data: &[u8], mut found: impl FnMut(&str, Option<&[u8]>, bool)) -> Result<(), FsError> {
    let root = romfs.root()?;
    let mut pending: Vec<(String, DirEntry)> = vec![(String::new(), root)];
    while let Some((path, dir)) = pending.pop() {
        found(&path, None, true);
        for (_, file) in romfs.files(&dir) {
            let start = romfs.file_data_offset(&file) as usize;
            let bytes = data
                .get(start..start + file.data_size as usize)
                .ok_or(FsError::BadRomFs("a file runs past the end of the image"))?;
            found(&join(&path, &file.name), Some(bytes), false);
        }
        for (_, child) in romfs.subdirs(&dir) {
            pending.push((join(&path, &child.name), child));
        }
    }
    Ok(())
}

fn join(path: &str, name: &str) -> String {
    match (path.is_empty(), name.is_empty()) {
        (true, _) => name.to_owned(),
        (_, true) => path.to_owned(),
        _ => format!("{path}/{name}"),
    }
}

/// extracts the RomFS romfs of data into out, or with check only compares
/// out with it.
fn extract_romfs(romfs: &RomFs, data: &[u8], out: &Path, check: bool) -> Result<Tally, FsError> {
    let mut tally = Tally::default();
    let mut error = None;
    let mut seen = std::collections::HashSet::new();
    let mut list: Vec<(String, u64)> = Vec::new();
    walk(romfs, data, |path, bytes, is_dir| {
        if error.is_some() {
            return;
        }
        let target = if path.is_empty() { out.to_path_buf() } else { out.join(path) };
        // Windows folders ignore case, two names differing only in it
        // would land on one file
        if !seen.insert(path.to_lowercase()) {
            log::warn!("os: {path} differs from another entry only in case");
            tally.bad += 1;
        }
        match (is_dir, bytes) {
            (true, _) => {
                tally.dirs += 1;
                if !check {
                    if let Err(e) = std::fs::create_dir_all(&target) {
                        error = Some(e);
                    }
                }
            }
            (false, Some(bytes)) => {
                tally.files += 1;
                EXTRACTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // builder
                tally.bytes += bytes.len() as u64;
                list.push((path.to_owned(), bytes.len() as u64));
                if check {
                    let same = std::fs::read(&target).is_ok_and(|have| have == bytes);
                    if !same {
                        say(format!("  differs: {path}"));
                        tally.bad += 1;
                    }
                } else if let Err(e) = std::fs::write(&target, bytes) {
                    error = Some(e);
                }
                if tally.files % 2000 == 0 {
                    say(format!("  {} files, {} MiB", tally.files, tally.bytes >> 20));
                }
            }
            _ => {}
        }
    })?;
    if let Some(error) = error {
        return Err(error.into());
    }
    if !check {
        write_index(out, &list)?;
    }
    // files in the folder the RomFS doesn't have
    if check {
        let mut extra = 0;
        count_extra(out, out, &seen, &mut extra);
        if extra > 0 {
            say(format!("  {extra} files in {} are not in the dump's RomFS", out.display()));
            tally.bad += extra;
        }
    }
    Ok(tally)
}

fn count_extra(root: &Path, dir: &Path, seen: &std::collections::HashSet<String>, extra: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count_extra(root, &path, seen, extra);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
            if !seen.contains(&key) {
                *extra += 1;
            }
        }
    }
}

/// installs the game in rom (a .3ds/.cci, .cxi or .cia dump) into out, or
/// with check only compares an installed out with it. a line per step on
/// stdout, the RomFS tally back.
pub fn install_game(rom: &Path, out: &Path, check: bool) -> Result<Tally, FsError> {
    let (image, offset) = open_game(rom)?;
    let data = image.data();
    let ncch_bytes = data.get(offset as usize..).ok_or(FsError::NoExecutablePartition)?;
    let ncch = NcchHeader::parse(ncch_bytes)?;
    let mut header = ncch_bytes[..NcchHeader::SIZE].to_vec();
    if !ncch.is_decrypted() && ncch.crypto_method() != 0 {
        // some decryptors leave the crypto flags as they were: the dump is
        // plaintext when its ExeFS header names .code in the clear
        let exefs_name = ncch_bytes.get(ncch.exefs_offset as usize..ncch.exefs_offset as usize + 8);
        if exefs_name != Some(&b".code\0\0\0"[..]) {
            return Err(FsError::Encrypted(ncch.crypto_method()));
        }
        say("the dump is decrypted but its header still says encrypted, the installed header says NoCrypto");
        header[0x18B] = 0; // crypto method
        header[0x18F] |= 0x04; // NoCrypto
    }
    say(format!("{} title {:016X} [{}]", rom.display(), ncch.program_id, ncch.product_code));
    let header = &header[..];
    let exheader = &ncch_bytes[NcchHeader::SIZE..NcchHeader::SIZE + ExHeader::FULL_SIZE];
    let exefs_start = ncch.exefs_offset as usize;
    let exefs = ncch_bytes.get(exefs_start..exefs_start + ncch.exefs_size as usize).ok_or(FsError::NoCode)?;
    let mut bad = 0;
    for (name, bytes) in [(NCCH_FILE, header), (EXHEADER_FILE, exheader), (EXEFS_FILE, exefs)] {
        let target = out.join(name);
        if check {
            if std::fs::read(&target).map_or(true, |have| have != bytes) {
                say(format!("  differs: {name}"));
                bad += 1;
            }
        } else {
            std::fs::create_dir_all(out)?;
            std::fs::write(&target, bytes)?;
        }
    }
    let romfs = RomFs::parse(data, offset + ncch.romfs_offset).map_err(|e| FsError::UnreadableRomFs(e.to_string()))?;
    let (dirs, files) = romfs.count_entries();
    say(format!("RomFS: {files} files in {dirs} folders, {} the folder {}", if check { "checking" } else { "extracting to" }, out.join(ROMFS_DIR).display()));
    let mut tally = extract_romfs(&romfs, data, &out.join(ROMFS_DIR), check)?;
    tally.bad += bad;
    if !check {
        let manifest = format!(
            "source {}\ntitle {:016X}\nproduct {}\nromfs_files {}\nromfs_dirs {}\nromfs_bytes {}\n",
            rom.display(),
            ncch.program_id,
            ncch.product_code,
            tally.files,
            tally.dirs,
            tally.bytes
        );
        std::fs::write(out.join(MANIFEST_FILE), manifest)?;
    }
    Ok(tally)
}

/// installs the DLC in the CIA at cia into out/dlc/<title id>, or with
/// check compares. the RomFS tally of all its contents back.
pub fn install_dlc(cia_path: &Path, out: &Path, check: bool) -> Result<Tally, FsError> {
    let image = RomImage::open(cia_path)?;
    let data = image.data();
    if !Cia::detect(data) {
        return Err(FsError::BadCia("DLC comes as a CIA"));
    }
    let cia = Cia::read(data)?;
    if cia.kind() != TitleKind::Dlc {
        return Err(FsError::BadCia("it is not DLC"));
    }
    let dir = out.join(DLC_DIR).join(format!("{:016X}", cia.title_id));
    say(format!("{} DLC {:016X} v{}, {} contents", cia_path.display(), cia.title_id, cia.version, cia.contents.len()));
    if !check {
        fresh_dlc_dir(&dir)?; // eurdlc
    }
    let mut list = format!("title {:016X}\nversion {}\n", cia.title_id, cia.version);
    let mut total = Tally::default();
    for content in &cia.contents {
        list += &format!("content {:04X} {:08X} {:04X} {}\n", content.index, content.id, content.flags, content.size);
        let Some(at) = content.offset else { continue };
        if content.encrypted() {
            return Err(FsError::EncryptedCia);
        }
        let Ok(ncch) = NcchHeader::parse(&data[at as usize..]) else {
            say(format!("  content {:04X} is not an NCCH, left out", content.index));
            continue;
        };
        if !ncch.has_romfs() {
            continue;
        }
        let romfs = RomFs::parse(data, at + ncch.romfs_offset).map_err(|e| FsError::UnreadableRomFs(e.to_string()))?;
        let target = dir.join(format!("{:04X}", content.index)).join(ROMFS_DIR);
        let tally = extract_romfs(&romfs, data, &target, check)?;
        total.files += tally.files;
        total.dirs += tally.dirs;
        total.bytes += tally.bytes;
        total.bad += tally.bad;
    }
    if check {
        if std::fs::read_to_string(dir.join(DLC_FILE)).map_or(true, |have| have != list) {
            say("  differs: dlc.txt");
            total.bad += 1;
        }
    } else {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(DLC_FILE), list)?;
    }
    say(format!("DLC: {} files, {} MiB in {}", total.files, total.bytes >> 20, dir.display()));
    Ok(total)
}

/// builder: installs DLC an emulator installed (Citra, Azahar: one
/// decrypted NCCH .app per content, and the title's TMD) into
/// out/dlc/<title id>, as install_dlc does a CIA. listed is every content
/// the TMD lists, apps the files of those there are, by content index.
pub fn install_dlc_apps(title_id: u64, version: u16, listed: &[Content], apps: &[(u16, PathBuf)], out: &Path) -> Result<Tally, FsError> {
    let dir = out.join(DLC_DIR).join(format!("{title_id:016X}"));
    fresh_dlc_dir(&dir)?; // eurdlc
    let mut list = format!("title {title_id:016X}\nversion {version}\n");
    let mut total = Tally::default();
    for content in listed {
        list += &format!("content {:04X} {:08X} {:04X} {}\n", content.index, content.id, content.flags, content.size);
        let Some((_, app)) = apps.iter().find(|(index, _)| *index == content.index) else { continue };
        let image = RomImage::open(app)?;
        let data = image.data();
        let Ok(ncch) = NcchHeader::parse(data) else {
            say(format!("  content {:04X} is not an NCCH, left out", content.index));
            continue;
        };
        if !ncch.has_romfs() {
            continue;
        }
        let romfs = RomFs::parse(data, ncch.romfs_offset).map_err(|e| FsError::UnreadableRomFs(e.to_string()))?;
        let target = dir.join(format!("{:04X}", content.index)).join(ROMFS_DIR);
        let tally = extract_romfs(&romfs, data, &target, false)?;
        total.files += tally.files;
        total.dirs += tally.dirs;
        total.bytes += tally.bytes;
        total.bad += tally.bad;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(DLC_FILE), list)?;
    say(format!("DLC: {} files, {} MiB in {}", total.files, total.bytes >> 20, dir.display()));
    Ok(total)
}

/// eurdlc: a DLC installed again starts from an empty folder, so no file of
/// the copy it replaces stays behind (another dump of the same DLC, or the
/// region-free DLC filed for the EUR game with its English text copied).
pub(crate) fn fresh_dlc_dir(dir: &Path) -> Result<(), FsError> {
    if dir.join(DLC_FILE).is_file() {
        say(format!("replacing the DLC installed in {}", dir.display()));
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// the DLC folders installed beside a folder game, data/dlc/*.
pub fn dlc_folders(game: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(game.join(DLC_DIR))
        .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.join(DLC_FILE).is_file()).collect())
        .unwrap_or_default();
    found.sort();
    found
}
