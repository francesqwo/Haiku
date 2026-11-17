//! hdpacks: mods that arrive as downloads, unpacked once into a mod folder.
//!
//! HD texture packs for Fates are made for Citra and shared as large zips:
//! a Google Drive download split into parts (`<folder>-<time>Z-1-002.zip`,
//! `-003`, ...), each holding more zips (`Character Artwork.zip`, ...) under
//! the game's title id folder, meant to be extracted into Citra's custom
//! texture folder. The mod manager used to unpack a zip at start, all of it
//! read into memory, on the window's thread: a 2.4 GB zip kept the window
//! from answering until Windows closed it, and the zips inside were never
//! opened, so nothing was found to draw.
//!
//! Here a large zip, a zip of zips, or the parts of a split download are a
//! **download**: one row on the Mods screen, named from its readme
//! ("README - Combined HD Pack for Fates.txt") or its file name. Turning
//! it on unpacks it, once, on a thread of its own, with the progress on the
//! Mods screen:
//!
//! - every part, and every zip inside, is read from disk as it goes
//!   (nothing is held in memory), into `mods/.unpacking-<name>/`;
//! - a texture pack goes into `textures/` (the title id folder left out),
//!   its readme beside it, and Citra's optional alternates (`ALT-tex1_...`,
//!   which the pack's readme says to rename by hand) into `alternates/`;
//!   a mod in another layout (romfs/, luma/, ...) is unpacked as it is;
//! - the folder becomes `mods/<name>/` with a mod.toml, and the downloaded
//!   zips are moved to `mods/_downloads/`, which the mod manager leaves
//!   alone, so nothing is lost and nothing is loaded twice. They are not
//!   needed any more and can be deleted.
//!
//! Unpacking rather than reading the pictures out of the zips as they are
//! needed: the inner zips are themselves deflated inside the parts, so a
//! picture can only be reached by inflating its part from the start; and
//! the pictures are PNGs, already compressed, so the folder takes about the
//! space the zips did.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// zips larger than this are downloads, unpacked on their own thread when
/// turned on rather than at start.
const LARGE: u64 = 64 << 20;

/// the folder of the downloaded zips once unpacked.
pub const DOWNLOADS: &str = "_downloads";

/// one entry of a zip's central directory.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    packed: u64,
    size: u64,
    local: u64,
}

/// a download in the mods folder: one zip or the parts of one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Download {
    /// the mod's id and the folder it is unpacked into
    pub id: String,
    pub name: String,
    pub parts: Vec<PathBuf>,
    /// bytes of the zips, and of what they hold
    pub packed: u64,
    pub unpacked: u64,
    /// zips inside, and pictures named for textures outside them
    pub nested: usize,
    pub pictures: usize,
    /// a texture pack (no romfs/, luma/ ... at its top)
    pub texture_pack: bool,
}

/// an unpacking under way or done.
#[derive(Debug, Default)]
pub struct Progress {
    pub id: String,
    pub name: String,
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub finished: AtomicBool,
    /// the folder made, or why it could not be
    pub result: Mutex<Option<Result<PathBuf, String>>>,
    /// the app has been told it finished
    pub reported: AtomicBool,
}

impl Progress {
    /// how far it is, 0 to 1.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed).max(1);
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

static RUNNING: Mutex<Vec<Arc<Progress>>> = Mutex::new(Vec::new());

/// the unpackings started in this run, under way or done.
pub fn all() -> Vec<Arc<Progress>> {
    RUNNING.lock().map(|list| list.clone()).unwrap_or_default()
}

/// the one under way, if any.
pub fn running() -> Option<Arc<Progress>> {
    all().into_iter().find(|progress| !progress.finished.load(Ordering::Relaxed))
}

/// those that finished since the last call.
pub fn take_finished() -> Vec<Arc<Progress>> {
    all().into_iter().filter(|progress| progress.finished.load(Ordering::Relaxed) && !progress.reported.swap(true, Ordering::Relaxed)).collect()
}

/// whether this mod is being unpacked now.
pub fn is_running(id: &str) -> bool {
    all().iter().any(|progress| progress.id.eq_ignore_ascii_case(id) && !progress.finished.load(Ordering::Relaxed))
}

/// for the interface's pictures (FATES_UI_SNAPSHOT): an unpacking 42%
/// done that never ends, or none.
pub fn snapshot_progress(on: bool) {
    let Ok(mut list) = RUNNING.lock() else { return };
    list.retain(|progress| progress.id != "snapshot");
    if on {
        let progress = Progress { id: "snapshot".into(), name: "Combined HD Pack for Fates".into(), ..Progress::default() };
        progress.total.store(6_400 << 20, Ordering::Relaxed);
        progress.done.store(2_700 << 20, Ordering::Relaxed);
        list.push(Arc::new(progress));
    }
}

/// the zips of the mods folder: the downloads among them, and the small
/// zips the mod manager unpacks into its cache as before.
pub fn find(folder: &Path) -> (Vec<Download>, Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return (Vec::new(), Vec::new()) };
    let mut zips: Vec<PathBuf> =
        entries.flatten().map(|entry| entry.path()).filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))).collect();
    zips.sort();
    let mut small = Vec::new();
    // the parts of a download by what their names share
    let mut groups: BTreeMap<String, Vec<(PathBuf, u64, Vec<Entry>)>> = BTreeMap::new();
    for zip in zips {
        let size = std::fs::metadata(&zip).map_or(0, |meta| meta.len());
        let stem = zip.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let part = drive_part(&stem);
        let entries = File::open(&zip).map_err(|error| error.to_string()).and_then(|mut file| directory(&mut file));
        let entries = match entries {
            Ok(entries) => entries,
            // the mod manager says why it can't be read
            Err(_) => {
                small.push(zip);
                continue;
            }
        };
        let nested = entries.iter().any(|entry| entry.name.to_ascii_lowercase().ends_with(".zip"));
        if size <= LARGE && !nested && part.is_none() {
            small.push(zip);
            continue;
        }
        let key = part.map(|(base, group)| format!("{base}\u{0}{group}")).unwrap_or_else(|| stem.clone());
        groups.entry(key).or_default().push((zip, size, entries));
    }
    let mut downloads = Vec::new();
    for (key, parts) in groups {
        let base = key.split('\u{0}').next().unwrap_or(&key).to_owned();
        let all: Vec<&Entry> = parts.iter().flat_map(|(_, _, entries)| entries.iter()).collect();
        // the name: the readme's ("README - <name>.txt"), else the file's
        let readme = all.iter().filter_map(|entry| readme_name(file_name(&entry.name))).next();
        let name = readme.unwrap_or_else(|| {
            let base = drive_part(&base).map_or(base.clone(), |(base, _)| base);
            if base.eq_ignore_ascii_case(crate::haiku_mods::PROGRAM_ID) {
                "Texture Pack".to_owned()
            } else {
                base
            }
        });
        let pictures = all.iter().filter(|entry| is_picture(file_name(&entry.name))).count();
        downloads.push(Download {
            id: folder_name(&name),
            name,
            parts: parts.iter().map(|(path, _, _)| path.clone()).collect(),
            packed: parts.iter().map(|(_, size, _)| size).sum(),
            unpacked: all.iter().map(|entry| entry.size).sum(),
            nested: all.iter().filter(|entry| entry.name.to_ascii_lowercase().ends_with(".zip")).count(),
            pictures,
            texture_pack: !is_mod_layout(all.iter().map(|entry| entry.name.as_str())),
        });
    }
    (downloads, small)
}

/// a Google Drive download's part: "<base>-<yyyymmdd>T<hhmmss>Z-<n>-<nnn>"
/// gives (base, its group), and "<base>-<nnn>" (a folder downloaded alone)
/// gives (base, "").
fn drive_part(stem: &str) -> Option<(String, String)> {
    let (rest, number) = stem.rsplit_once('-')?;
    if number.len() != 3 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some((rest2, batch)) = rest.rsplit_once('-') {
        if let Some((base, stamp)) = rest2.rsplit_once('-') {
            let b = stamp.as_bytes();
            let stamp_ok = b.len() == 16 && b[8] == b'T' && b[15] == b'Z' && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit);
            if stamp_ok && !batch.is_empty() && batch.bytes().all(|c| c.is_ascii_digit()) {
                return Some((base.to_owned(), format!("{stamp}-{batch}")));
            }
        }
    }
    (!rest.is_empty()).then(|| (rest.to_owned(), String::new()))
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// "README - Combined HD Pack for Fates.txt" gives its name.
fn readme_name(file: &str) -> Option<String> {
    let lower = file.to_ascii_lowercase();
    if !lower.starts_with("readme") || !lower.ends_with(".txt") {
        return None;
    }
    let name = file[6..file.len() - 4].trim_start_matches([' ', '-', '_', ':']).trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// a picture named for a texture, as Citra reads them.
fn is_picture(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.starts_with("tex1_") && lower.ends_with(".png")
}

/// an alternate picture a pack ships for the player to choose by hand.
fn is_alternate(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.starts_with("alt-") || lower.starts_with("alt_")
}

/// whether paths (after the title id folder) are a mod's own layout.
fn is_mod_layout<'a>(paths: impl Iterator<Item = &'a str>) -> bool {
    paths.into_iter().any(|path| {
        let parts = parts_of(path);
        let parts = strip_title(&parts);
        // a single wrapping folder is looked through
        let found = [parts.first(), parts.get(1)].into_iter().flatten().any(|part| {
            let part = part.to_ascii_lowercase();
            ["romfs", "romfs_ext", "luma", "exefs", "mod.toml", "textures", "dlc"].contains(&part.as_str())
        });
        found
    })
}

/// a path's parts, none of them empty or ".".
fn parts_of(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").map(str::to_owned).collect()
}

/// parts without the title id folder Citra's layouts start with.
fn strip_title(parts: &[String]) -> &[String] {
    match parts.first() {
        Some(first) if first.eq_ignore_ascii_case(crate::haiku_mods::PROGRAM_ID) => &parts[1..],
        _ => parts,
    }
}

/// a name Windows takes as a file or folder name.
fn clean(part: &str) -> String {
    let cleaned: String = part.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() {
        "_".to_owned()
    } else {
        cleaned
    }
}

/// a mod's name as a folder name.
pub fn folder_name(name: &str) -> String {
    clean(name.trim())
}

/// reads a zip's central directory, ZIP64 too.
fn directory(file: &mut File) -> Result<Vec<Entry>, String> {
    let length = file.seek(SeekFrom::End(0)).map_err(|error| error.to_string())?;
    let tail = length.min(0x10000 + 22);
    file.seek(SeekFrom::Start(length - tail)).map_err(|error| error.to_string())?;
    let mut end = vec![0; tail as usize];
    file.read_exact(&mut end).map_err(|error| error.to_string())?;
    let at = (0..end.len().saturating_sub(21)).rev().find(|&at| end[at..at + 4] == [0x50, 0x4B, 0x05, 0x06]).ok_or("it is not a zip")?;
    let le = |bytes: &[u8], at: usize, size: usize| -> u64 { bytes[at..at + size].iter().rev().fold(0u64, |value, byte| value << 8 | *byte as u64) };
    let mut count = le(&end, at + 10, 2);
    let mut size = le(&end, at + 12, 4);
    let mut offset = le(&end, at + 16, 4);
    if count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
        // ZIP64: its locator just before, pointing at its own record
        let locator = at.checked_sub(20).filter(|&locator| end[locator..locator + 4] == [0x50, 0x4B, 0x06, 0x07]).ok_or("its ZIP64 end is missing")?;
        let record = le(&end, locator + 8, 8);
        let mut zip64 = [0u8; 56];
        file.seek(SeekFrom::Start(record)).map_err(|error| error.to_string())?;
        file.read_exact(&mut zip64).map_err(|error| error.to_string())?;
        if zip64[..4] != [0x50, 0x4B, 0x06, 0x06] {
            return Err("its ZIP64 end is damaged".into());
        }
        count = le(&zip64, 32, 8);
        size = le(&zip64, 40, 8);
        offset = le(&zip64, 48, 8);
    }
    if offset + size > length || size > 512 << 20 {
        return Err("its file list is damaged".into());
    }
    let mut central = vec![0; size as usize];
    file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
    file.read_exact(&mut central).map_err(|error| error.to_string())?;
    let mut entries = Vec::with_capacity(count as usize);
    let mut at = 0usize;
    for _ in 0..count {
        if at + 46 > central.len() || central[at..at + 4] != [0x50, 0x4B, 0x01, 0x02] {
            return Err("its file list is damaged".into());
        }
        let flags = le(&central, at + 8, 2) as u16;
        let method = le(&central, at + 10, 2) as u16;
        let mut packed = le(&central, at + 20, 4);
        let mut size = le(&central, at + 24, 4);
        let name_len = le(&central, at + 28, 2) as usize;
        let extra_len = le(&central, at + 30, 2) as usize;
        let comment_len = le(&central, at + 32, 2) as usize;
        let mut local = le(&central, at + 42, 4);
        let end = at + 46 + name_len + extra_len + comment_len;
        if end > central.len() {
            return Err("its file list is damaged".into());
        }
        let raw = &central[at + 46..at + 46 + name_len];
        let name = if flags & 0x800 != 0 { String::from_utf8_lossy(raw).into_owned() } else { raw.iter().map(|&b| b as char).collect() };
        // the ZIP64 extra field: the sizes and offset that did not fit
        let mut extra = &central[at + 46 + name_len..at + 46 + name_len + extra_len];
        while extra.len() >= 4 {
            let (id, length) = (le(extra, 0, 2), le(extra, 2, 2) as usize);
            let body = &extra[4..(4 + length).min(extra.len())];
            if id == 1 {
                let mut field = 0;
                for value in [&mut size, &mut packed, &mut local] {
                    if *value == 0xFFFF_FFFF && field + 8 <= body.len() {
                        *value = le(body, field, 8);
                        field += 8;
                    }
                }
            }
            extra = &extra[(4 + length).min(extra.len())..];
        }
        entries.push(Entry { name, method, flags, packed, size, local });
        at = end;
    }
    Ok(entries)
}

/// an entry's bytes, read from disk as they are taken.
fn open<'a>(file: &'a mut File, entry: &Entry) -> Result<Box<dyn Read + 'a>, String> {
    if entry.flags & 1 != 0 {
        return Err(format!("{} is encrypted", entry.name));
    }
    let mut header = [0u8; 30];
    file.seek(SeekFrom::Start(entry.local)).map_err(|error| error.to_string())?;
    file.read_exact(&mut header).map_err(|error| error.to_string())?;
    if header[..4] != [0x50, 0x4B, 0x03, 0x04] {
        return Err(format!("{} is damaged", entry.name));
    }
    let skip = u16::from_le_bytes([header[26], header[27]]) as i64 + u16::from_le_bytes([header[28], header[29]]) as i64;
    file.seek(SeekFrom::Current(skip)).map_err(|error| error.to_string())?;
    let data = std::io::Read::take(file, entry.packed);
    match entry.method {
        0 => Ok(Box::new(data)),
        8 => Ok(Box::new(flate2::read::DeflateDecoder::new(std::io::BufReader::with_capacity(1 << 20, data)))),
        method => Err(format!("{} is packed in a way that can't be read (method {method})", entry.name)),
    }
}

/// where a file of a download goes in the mod folder, none for one left out.
fn place(texture_pack: bool, base: &[String], path: &str) -> Result<Option<PathBuf>, String> {
    let parts = parts_of(path);
    if parts.iter().any(|part| part == ".." || part.contains(':')) || path.starts_with('/') || path.starts_with('\\') {
        return Err(format!("it holds a file named {path}, outside its own folder"));
    }
    let mut all: Vec<String> = base.to_vec();
    all.extend(parts);
    let all = strip_title(&all).to_vec();
    let Some(file) = all.last() else { return Ok(None) };
    let mut out = PathBuf::new();
    if texture_pack {
        if readme_name(file).is_some() || file.eq_ignore_ascii_case("readme.txt") {
            out.push(clean(file));
            return Ok(Some(out));
        }
        if is_alternate(file) {
            out.push("alternates");
            out.push(clean(file));
            return Ok(Some(out));
        }
        out.push("textures");
    }
    for part in &all {
        out.push(clean(part));
    }
    Ok(Some(out))
}

/// how much there is to do: every byte written once, a zip inside twice
/// (copied out, then unpacked; what it holds is counted at its own size
/// until it is opened, then as what it really holds).
fn work(download: &Download) -> u64 {
    download.unpacked + nested_bytes(download)
}

fn nested_bytes(download: &Download) -> u64 {
    download
        .parts
        .iter()
        .filter_map(|part| File::open(part).ok().and_then(|mut file| directory(&mut file).ok()))
        .flatten()
        .filter(|entry| entry.name.to_ascii_lowercase().ends_with(".zip"))
        .map(|entry| entry.size)
        .sum()
}

/// the bytes free on the disk a folder is on.
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: a NUL-terminated path and three u64s to write.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    (ok != 0).then_some(available)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// starts unpacking a download into the mods folder, on its own thread.
pub fn start(download: &Download, mods: &Path) -> Arc<Progress> {
    if let Some(running) = all().into_iter().find(|progress| progress.id.eq_ignore_ascii_case(&download.id) && !progress.finished.load(Ordering::Relaxed)) {
        return running;
    }
    let progress = Arc::new(Progress { id: download.id.clone(), name: download.name.clone(), ..Progress::default() });
    if let Ok(mut list) = RUNNING.lock() {
        list.push(progress.clone());
    }
    let (download, mods, shared) = (download.clone(), mods.to_path_buf(), progress.clone());
    let spawned = std::thread::Builder::new().name("mods: unpacking".into()).spawn(move || {
        let started = std::time::Instant::now();
        let result = unpack(&download, &mods, &shared);
        match &result {
            Ok(folder) => log::info!(
                "mods: unpacked {} ({} parts, {} MB) into {} in {:.1} s",
                download.name,
                download.parts.len(),
                download.packed >> 20,
                folder.display(),
                started.elapsed().as_secs_f32()
            ),
            Err(error) => log::warn!("mods: {} could not be unpacked, {error}", download.name),
        }
        if let Ok(mut slot) = shared.result.lock() {
            *slot = Some(result);
        }
        shared.finished.store(true, Ordering::Relaxed);
    });
    if let Err(error) = spawned {
        if let Ok(mut slot) = progress.result.lock() {
            *slot = Some(Err(error.to_string()));
        }
        progress.finished.store(true, Ordering::Relaxed);
    }
    progress
}

/// unpacks it now, on this thread: the mod folder made.
pub fn unpack(download: &Download, mods: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let staging = mods.join(format!(".unpacking-{}", download.id));
    let _ = std::fs::remove_dir_all(&staging);
    progress.total.store(work(download).max(1), Ordering::Relaxed);
    if let Some(free) = free_space(mods) {
        // the files, and room for the largest zip inside while it is unpacked
        let need = if download.nested > 0 { download.unpacked / 2 * 3 } else { download.unpacked } + (64 << 20);
        if free < need {
            return Err(format!("there are {} MB free on the disk and it needs about {} MB", free >> 20, need >> 20));
        }
    }
    std::fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let result = (|| {
        let mut pictures = 0usize;
        for part in &download.parts {
            let mut file = File::open(part).map_err(|error| format!("{}: {error}", part.display()))?;
            unpack_zip(&mut file, download.texture_pack, &[], &staging, progress, &mut pictures, 0, None)?;
        }
        // its mod.toml, which names it
        let parts = download.parts.len();
        let what = if download.texture_pack { format!("A texture pack of {pictures} pictures") } else { "A mod".to_owned() };
        let from = if parts == 1 { "a downloaded zip".to_owned() } else { format!("a download in {parts} parts") };
        let description = format!("{what}, unpacked by Haiku from {from}. Its zips are in mods/{DOWNLOADS} and can be deleted.");
        let toml = format!("id = {}\nname = {}\ndescription = {}\n", quote(&download.id), quote(&download.name), quote(&description));
        std::fs::write(staging.join("mod.toml"), toml).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    // in place, beside any older folder of the same name
    let mut folder = mods.join(&download.id);
    let mut n = 2;
    while folder.exists() {
        folder = mods.join(format!("{} ({n})", download.id));
        n += 1;
    }
    std::fs::rename(&staging, &folder).map_err(|error| error.to_string())?;
    // and the zips out of the way
    let downloads = mods.join(DOWNLOADS);
    let _ = std::fs::create_dir_all(&downloads);
    for part in &download.parts {
        let Some(name) = part.file_name() else { continue };
        let mut to = downloads.join(name);
        let mut n = 2;
        while to.exists() {
            to = downloads.join(format!("{} ({n}).zip", part.file_stem().unwrap_or_default().to_string_lossy()));
            n += 1;
        }
        if let Err(error) = std::fs::rename(part, &to) {
            log::warn!("mods: could not move {} to {}, {error}", part.display(), downloads.display());
        }
    }
    progress.done.store(progress.total.load(Ordering::Relaxed), Ordering::Relaxed);
    Ok(folder)
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// unpacks one zip (a part, or a zip inside one at base) into the folder.
#[allow(clippy::too_many_arguments)]
fn unpack_zip(file: &mut File, texture_pack: bool, base: &[String], folder: &Path, progress: &Progress, pictures: &mut usize, depth: u32, counted: Option<u64>) -> Result<(), String> {
    let entries = directory(file)?;
    // a zip inside was counted at its own size: now at what it holds
    if let Some(counted) = counted {
        let holds: u64 = entries.iter().map(|entry| entry.size).sum();
        if holds >= counted {
            progress.total.fetch_add(holds - counted, Ordering::Relaxed);
        } else {
            progress.total.fetch_sub((counted - holds).min(progress.total.load(Ordering::Relaxed)), Ordering::Relaxed);
        }
    }
    for entry in &entries {
        if entry.name.ends_with('/') || entry.name.ends_with('\\') {
            continue;
        }
        let lower = entry.name.to_ascii_lowercase();
        if lower.ends_with(".zip") && depth < 3 {
            // copied out, unpacked where it lay, and let go of
            let scratch = folder.join(format!(".inner-{depth}.zip"));
            {
                let mut reader = open(file, entry)?;
                let mut out = File::create(&scratch).map_err(|error| error.to_string())?;
                copy(&mut reader, &mut out, progress).map_err(|error| format!("{}: {error}", entry.name))?;
            }
            let mut inner = File::open(&scratch).map_err(|error| error.to_string())?;
            let mut here: Vec<String> = base.to_vec();
            let parts = parts_of(&entry.name);
            here.extend(parts[..parts.len().saturating_sub(1)].iter().cloned());
            let result = unpack_zip(&mut inner, texture_pack, &here, folder, progress, pictures, depth + 1, Some(entry.size));
            drop(inner);
            let _ = std::fs::remove_file(&scratch);
            result.map_err(|error| format!("{}: {error}", entry.name))?;
            continue;
        }
        let Some(relative) = place(texture_pack, base, &entry.name)? else { continue };
        let path = folder.join(&relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut reader = open(file, entry)?;
        let mut out = File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let written = copy(&mut reader, &mut out, progress).map_err(|error| format!("{}: {error}", entry.name))?;
        if written != entry.size {
            return Err(format!("{} is damaged", entry.name));
        }
        if is_picture(file_name(&entry.name)) {
            *pictures += 1;
        }
    }
    Ok(())
}

/// copies, counting the bytes as done.
fn copy(reader: &mut dyn Read, out: &mut File, progress: &Progress) -> std::io::Result<u64> {
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0;
    let mut writer = std::io::BufWriter::with_capacity(1 << 20, out);
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        total += n as u64;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    writer.flush()?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_parts_are_told_apart() {
        assert_eq!(drive_part("0004000000179800-20261110T234824Z-1-002"), Some(("0004000000179800".into(), "20261110T234824Z-1".into())));
        assert_eq!(drive_part("Map Floors-001"), Some(("Map Floors".into(), String::new())));
        assert_eq!(drive_part("My Mod"), None);
        assert_eq!(drive_part("mod-v2"), None);
        assert_eq!(readme_name("README - Combined HD Pack for Fates.txt"), Some("Combined HD Pack for Fates".into()));
        assert_eq!(readme_name("readme.txt"), None);
        assert_eq!(folder_name("A: B?"), "A_ B_");
    }

    #[test]
    fn files_go_where_the_mod_manager_reads_them() {
        let none: Vec<String> = Vec::new();
        let p = |base: &[String], path: &str| place(true, base, path).unwrap().map(|path| path.to_string_lossy().replace('\\', "/"));
        assert_eq!(p(&none, "0004000000179800/README - X.txt"), Some("README - X.txt".into()));
        assert_eq!(p(&none, "0004000000179800/ALT-tex1_8x8_00000000000000AA_13.png"), Some("alternates/ALT-tex1_8x8_00000000000000AA_13.png".into()));
        assert_eq!(p(&["0004000000179800".into()], "UI/tex1_8x8_00000000000000AA_13.png"), Some("textures/UI/tex1_8x8_00000000000000AA_13.png".into()));
        assert_eq!(p(&none, "Map Floors/tex1_8x8_00000000000000AA_12.png"), Some("textures/Map Floors/tex1_8x8_00000000000000AA_12.png".into()));
        assert!(place(true, &none, "../x.png").is_err());
        assert_eq!(place(false, &none, "romfs/a.bin").unwrap().map(|path| path.to_string_lossy().replace('\\', "/")), Some("romfs/a.bin".into()));
        assert!(is_mod_layout(["Wrapped/romfs/a.bin"].into_iter()));
        assert!(!is_mod_layout(["0004000000179800/UI.zip", "0004000000179800/README - X.txt"].into_iter()));
    }

    /// a stored zip of files.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let local = out.len() as u32;
            let header = |sig: u32, central: bool| {
                let mut h = sig.to_le_bytes().to_vec();
                if central {
                    h.extend_from_slice(&20u16.to_le_bytes());
                }
                h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                h.extend_from_slice(&0u32.to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(name.len() as u16).to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes());
                h
            };
            out.extend(header(0x0403_4B50, false));
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            let mut c = header(0x0201_4B50, true);
            c.extend_from_slice(&[0; 10]);
            c.extend_from_slice(&local.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            central.push(c);
        }
        let at = out.len() as u32;
        let size: usize = central.iter().map(Vec::len).sum();
        for c in &central {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    /// a download in two parts, zips inside, is found as one and unpacked
    /// as Citra would have it, its zips put away.
    #[test]
    fn a_split_download_of_zips_is_one_mod() {
        let mods = std::env::temp_dir().join(format!("haiku-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mods);
        std::fs::create_dir_all(&mods).unwrap();
        let ui = zip(&[("UI/tex1_8x8_00000000000000AA_13.png", b"aa"), ("UI/00_tex1_8x8_00000000000000AB_13.png", b"ab")]);
        let faces = zip(&[("Faces/sub/tex1_8x8_00000000000000BB_13.png", b"bb")]);
        std::fs::write(mods.join("0004000000179800-20261110T234824Z-1-001.zip"), zip(&[("0004000000179800/UI.zip", &ui), ("0004000000179800/README - Test Pack.txt", b"hi")])).unwrap();
        std::fs::write(
            mods.join("0004000000179800-20261110T234824Z-1-002.zip"),
            zip(&[("0004000000179800/Faces.zip", &faces), ("0004000000179800/ALT-tex1_8x8_00000000000000AA_13.png", b"alt")]),
        )
        .unwrap();
        std::fs::write(mods.join("small.zip"), zip(&[("romfs/a.bin", b"a")])).unwrap();
        let (downloads, small) = find(&mods);
        assert_eq!(small, vec![mods.join("small.zip")]);
        assert_eq!(downloads.len(), 1);
        let download = &downloads[0];
        assert_eq!(download.name, "Test Pack");
        assert_eq!(download.parts.len(), 2);
        assert_eq!(download.nested, 2);
        assert!(download.texture_pack);
        let progress = Progress::default();
        let folder = unpack(download, &mods, &progress).unwrap();
        assert_eq!(folder, mods.join("Test Pack"));
        assert_eq!(std::fs::read(folder.join("textures/UI/tex1_8x8_00000000000000AA_13.png")).unwrap(), b"aa");
        assert_eq!(std::fs::read(folder.join("textures/Faces/sub/tex1_8x8_00000000000000BB_13.png")).unwrap(), b"bb");
        assert!(folder.join("alternates/ALT-tex1_8x8_00000000000000AA_13.png").is_file());
        assert!(folder.join("README - Test Pack.txt").is_file());
        assert!(std::fs::read_to_string(folder.join("mod.toml")).unwrap().contains("name = \"Test Pack\""));
        assert!(mods.join(DOWNLOADS).join("0004000000179800-20261110T234824Z-1-002.zip").is_file());
        assert!(!mods.join("0004000000179800-20261110T234824Z-1-001.zip").exists());
        assert_eq!(progress.fraction(), 1.0);
        assert!(!mods.join(".unpacking-Test Pack").exists());
        std::fs::remove_dir_all(&mods).unwrap();
    }
}
