//! The DLC the player picked: a decrypted .cia (GodMode9's "Build CIA"
//! of the installed DLC, the usual way it is dumped), Citra's or Azahar's
//! sdmc folder with the DLC installed in it, or the data/dlc folder of an
//! earlier Haiku. Each is checked to be Fire Emblem Fates' USA DLC
//! (0004008C00179400), decrypted and whole, then installed into
//! data/dlc/<title id> the way Haiku.exe reads it (zakuro_fs::datadir).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dump::{self, Dump, Problem, DECRYPT_HINT};

pub const DLC_TITLE: u64 = 0x0004_008C_0017_9400;

#[derive(Debug, Clone)]
pub enum Kind {
    /// a CIA file.
    Cia,
    /// a folder of .app contents and their .tmd (Citra, Azahar).
    Installed { content_dir: PathBuf, tmd: PathBuf },
    /// data/dlc/<title id> of an earlier install.
    Haiku,
}

#[derive(Debug, Clone)]
pub struct Dlc {
    pub path: PathBuf,
    pub kind: Kind,
    pub title_id: u64,
    pub version: u16,
    /// the contents the title lists.
    pub listed: usize,
    /// the contents this copy has: index, id, flags, size, hash (zero for
    /// the Haiku kind), and for an installed folder the .app file.
    pub contents: Vec<(u16, u32, u16, u64, [u8; 32], Option<PathBuf>)>,
    pub bytes: u64,
}

impl Dlc {
    /// eurdlc: the panel's headline, the DLC's own region
    pub fn headline(&self) -> &'static str {
        if self.title_id == crate::eur::DLC_TITLE {
            "Fire Emblem Fates DLC (Europe)"
        } else if self.title_id == crate::jpn::DLC_TITLE {
            "Fire Emblem if DLC (Japan)"
        } else {
            "Fire Emblem Fates DLC (USA)"
        }
    }

    pub fn describe(&self) -> String {
        let source = match self.kind {
            Kind::Cia => "a decrypted CIA",
            Kind::Installed { .. } => "an emulator's installed DLC",
            Kind::Haiku => "an earlier Haiku install",
        };
        format!(
            "Fire Emblem Fates DLC ({}), title {:016X} v{}: {} of its {} contents, {} MB, from {source}.",
            if self.title_id == crate::eur::DLC_TITLE { "EUR" } else if self.title_id == crate::jpn::DLC_TITLE { "Japan, Fire Emblem if" } else { "USA" }, // eur, jpprep
            self.title_id,
            self.version,
            self.contents.len(),
            self.listed,
            self.bytes / 1_000_000
        )
    }
}

/// eur: the USA DLC (region-free CIAs carry it for every region) or the
/// one the EUR game asks for
fn known_title(title: u64) -> bool {
    // jpprep: and the Japanese DLC (which game it fits: jpn::dlc_fits)
    title == DLC_TITLE || title == crate::eur::DLC_TITLE || title == crate::jpn::DLC_TITLE
}

fn wrong_title(title: u64) -> Problem {
    match title >> 32 {
        0x0004_0000 => Problem::WrongKind("This is a game, not DLC. Select the DLC here (the game was the step before).".into()),
        0x0004_000E => Problem::WrongKind("This is an update, not the DLC. Haiku doesn't need the update; select the DLC.".into()),
        0x0004_008C => Problem::WrongGame(format!(
            // eurdlc: the European DLC is one too
            "This is DLC for a different game (title {title:016X}). Haiku needs Fire Emblem Fates' DLC: the USA DLC (0004008C00179400), or for the European game the European DLC (0004008C00179500)."
        )),
        _ => Problem::NotADump(format!("This is not Fire Emblem Fates DLC (title {title:016X}).")),
    }
}

/// checks what the player picked for the DLC.
pub fn check(path: &Path) -> Result<Dlc, Problem> {
    if path.is_dir() {
        return check_folder(path);
    }
    let mut file = Dump::open(path)?;
    if file.len < 0x2020 {
        return Err(Problem::NotADump("This file is too small to be DLC. Select the DLC's decrypted .cia.".into()));
    }
    let header = file.read(0, 0x2020)?;
    if &header[0x100..0x104] == b"NCSD" || &header[0x100..0x104] == b"NCCH" {
        return Err(Problem::WrongKind("This is a game, not DLC. Select the DLC here (the game was the step before).".into()));
    }
    if u32::from_le_bytes(header[0..4].try_into().unwrap()) != 0x2020 {
        return Err(Problem::NotADump("This is not a CIA. Select the DLC's decrypted .cia (or Citra's or Azahar's sdmc folder).".into()));
    }
    let align = |n: u64| n.next_multiple_of(0x40);
    let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap()) as u64;
    let tmd_offset = align(align(align(0x2020) + u32_at(0x08)) + u32_at(0x0C));
    let tmd_size = u32_at(0x10) as usize;
    let tmd = file.read(tmd_offset, tmd_size)?;
    let (title, version, records) = dump::tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The CIA's title metadata can't be read: the file is damaged.".into()))?;
    if !known_title(title) {
        return Err(wrong_title(title));
    }
    let mut contents = Vec::new();
    let mut offset = align(tmd_offset + tmd_size as u64);
    let mut bytes = 0;
    for (index, id, flags, size, hash) in records.iter().copied() {
        let present = header[0x20 + index as usize / 8] & (0x80 >> (index % 8)) != 0;
        if !present {
            continue;
        }
        if flags & 1 != 0 {
            return Err(Problem::Encrypted(format!("This DLC CIA is encrypted (it still has the eShop's title key encryption). {DECRYPT_HINT}")));
        }
        if offset + size > file.len {
            return Err(Problem::Corrupt("The CIA is shorter than its own header says: it is an incomplete copy or download.".into()));
        }
        // the content's own NCCH has to be readable as it is
        let ncch = file.read(offset, 0x200)?;
        if &ncch[0x100..0x104] != b"NCCH" {
            return Err(Problem::Encrypted(format!("This DLC's contents are encrypted. {DECRYPT_HINT}")));
        }
        check_ncch_plain(&ncch, &mut file, offset)?;
        contents.push((index, id, flags, size, hash, None));
        bytes += size;
        offset = align(offset + size);
    }
    if contents.is_empty() {
        return Err(Problem::Corrupt("The CIA holds none of the DLC's contents.".into()));
    }
    Ok(Dlc { path: path.to_path_buf(), kind: Kind::Cia, title_id: title, version, listed: records.len(), contents, bytes })
}

/// an NCCH content must be decrypted: NoCrypto, or its RomFS readable.
fn check_ncch_plain(ncch: &[u8], file: &mut Dump, at: u64) -> Result<(), Problem> {
    let flags = &ncch[0x188..0x190];
    if flags[7] & 0x04 != 0 {
        return Ok(());
    }
    let romfs = u32::from_le_bytes(ncch[0x1B0..0x1B4].try_into().unwrap()) as u64 * dump::MEDIA;
    let size = u32::from_le_bytes(ncch[0x1B4..0x1B8].try_into().unwrap());
    if size == 0 {
        return Ok(());
    }
    if file.read(at + romfs, 4)? != b"IVFC" {
        return Err(Problem::Encrypted(format!("This DLC's contents are encrypted. {DECRYPT_HINT}")));
    }
    Ok(())
}

fn check_folder(dir: &Path) -> Result<Dlc, Problem> {
    // an earlier Haiku's data/dlc, data or the title folder itself
    // jpprep: the EUR and Japanese DLC's folders too
    for title in [DLC_TITLE, crate::eur::DLC_TITLE, crate::jpn::DLC_TITLE] {
        let tid = format!("{title:016X}");
        for candidate in [dir.to_path_buf(), dir.join(&tid), dir.join("dlc").join(&tid), dir.join("data").join("dlc").join(&tid)] {
            if candidate.join("dlc.txt").is_file() {
                return check_haiku(&candidate);
            }
        }
    }
    // Citra / Azahar: .../title/0004008c/00179400/content/*.tmd + *.app
    let mut tmds = Vec::new();
    let mut cias = Vec::new();
    dump::find_files(dir, 0, &mut |path| {
        let lower = path.to_string_lossy().to_lowercase().replace('\\', "/");
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "tmd" && (lower.contains("/0004008c/00179400/content") || lower.contains("/0004008c/00179500/content") || lower.contains("/0004008c/0012dc00/content")) { // jpprep: and the EUR and Japanese DLC
            tmds.push(path.to_path_buf());
        } else if ext == "cia" {
            cias.push(path.to_path_buf());
        }
    });
    tmds.sort();
    if let Some(tmd) = tmds.last() {
        return check_installed(tmd);
    }
    let fitting: Vec<Dlc> = cias.iter().filter_map(|p| check(p).ok()).collect();
    if fitting.len() == 1 {
        return Ok(fitting.into_iter().next().unwrap());
    }
    Err(Problem::NotADump(
        "This folder doesn't hold Fire Emblem Fates DLC. Select the DLC's decrypted .cia, or Citra's or Azahar's sdmc folder with the DLC installed.".into(),
    ))
}

fn check_haiku(dir: &Path) -> Result<Dlc, Problem> {
    let text = std::fs::read_to_string(dir.join("dlc.txt")).map_err(|e| Problem::Unreadable(format!("dlc.txt can't be read ({e}).")))?;
    let (mut title, mut version, mut contents, mut listed, mut bytes) = (0u64, 0u16, Vec::new(), 0, 0);
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["title", id] => title = u64::from_str_radix(id, 16).unwrap_or(0),
            ["version", v] => version = v.parse().unwrap_or(0),
            ["content", index, id, flags, size] => {
                listed += 1;
                let index = u16::from_str_radix(index, 16).unwrap_or(0);
                let size: u64 = size.parse().unwrap_or(0);
                if dir.join(format!("{index:04X}")).is_dir() {
                    contents.push((index, u32::from_str_radix(id, 16).unwrap_or(0), u16::from_str_radix(flags, 16).unwrap_or(0), size, [0; 32], None));
                    bytes += size;
                }
            }
            _ => {}
        }
    }
    if !known_title(title) {
        return Err(wrong_title(title));
    }
    if contents.is_empty() {
        return Err(Problem::Corrupt("This DLC folder has none of its contents.".into()));
    }
    Ok(Dlc { path: dir.to_path_buf(), kind: Kind::Haiku, title_id: title, version, listed, contents, bytes })
}

fn check_installed(tmd_path: &Path) -> Result<Dlc, Problem> {
    let tmd = std::fs::read(tmd_path).map_err(|e| Problem::Unreadable(format!("The DLC's .tmd can't be read ({e}).")))?;
    let (title, version, records) = dump::tmd_contents(&tmd).ok_or_else(|| Problem::Corrupt("The DLC's .tmd is damaged.".into()))?;
    if !known_title(title) {
        return Err(wrong_title(title));
    }
    let content_dir = tmd_path.parent().unwrap().to_path_buf();
    let mut contents = Vec::new();
    let mut bytes = 0;
    for (index, id, flags, size, hash) in records.iter().copied() {
        let app = content_dir.join(format!("{id:08x}.app"));
        let app = if app.is_file() { app } else { content_dir.join(format!("{id:08X}.app")) };
        if !app.is_file() {
            continue;
        }
        let mut file = Dump::open(&app)?;
        let ncch = file.read(0, 0x200)?;
        if &ncch[0x100..0x104] != b"NCCH" {
            return Err(Problem::Encrypted(format!("This DLC's contents are encrypted. {DECRYPT_HINT}")));
        }
        check_ncch_plain(&ncch, &mut file, 0)?;
        contents.push((index, id, flags & !1, size, hash, Some(app)));
        bytes += size;
    }
    if contents.is_empty() {
        return Err(Problem::Corrupt("The emulator's DLC folder has none of the DLC's contents.".into()));
    }
    Ok(Dlc { path: tmd_path.to_path_buf(), kind: Kind::Installed { content_dir, tmd: tmd_path.to_path_buf() }, title_id: title, version, listed: records.len(), contents, bytes })
}

/// checks every content against the TMD's hash (a CIA's or an installed
/// folder's), telling progress the fraction done.
pub fn verify(dlc: &Dlc, progress: &dyn Fn(f32)) -> Result<(), Problem> {
    let total: u64 = dlc.contents.iter().map(|c| c.3).sum::<u64>().max(1);
    let mut done = 0u64;
    match &dlc.kind {
        Kind::Haiku => Ok(()),
        Kind::Cia => {
            let mut file = Dump::open(&dlc.path)?;
            let header = file.read(0, 0x2020)?;
            let align = |n: u64| n.next_multiple_of(0x40);
            let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap()) as u64;
            let tmd_offset = align(align(align(0x2020) + u32_at(0x08)) + u32_at(0x0C));
            let mut offset = align(tmd_offset + u32_at(0x10));
            for (index, _, _, size, hash, _) in &dlc.contents {
                let mut hasher = Sha256::new();
                let mut at = 0;
                while at < *size {
                    let n = (*size - at).min(8 << 20) as usize;
                    hasher.update(file.read(offset + at, n)?);
                    at += n as u64;
                    done += n as u64;
                    progress(done as f32 / total as f32);
                }
                if hasher.finalize().as_slice() != hash {
                    return Err(Problem::Corrupt(format!("DLC content {index:04X} is damaged (it doesn't match its hash). Dump the DLC again.")));
                }
                offset = align(offset + size);
            }
            Ok(())
        }
        Kind::Installed { .. } => {
            for (index, _, _, size, hash, app) in &dlc.contents {
                let bytes = std::fs::read(app.as_ref().unwrap()).map_err(|e| Problem::Unreadable(format!("DLC content {index:04X} can't be read ({e}).")))?;
                done += size;
                progress(done as f32 / total as f32);
                if Sha256::digest(&bytes).as_slice() != hash {
                    return Err(Problem::Corrupt(format!("DLC content {index:04X} is damaged (it doesn't match its hash).")));
                }
            }
            Ok(())
        }
    }
}

fn copy_tree(from: &Path, to: &Path, count: &mut usize) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target, count)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            *count += 1;
        }
    }
    Ok(())
}

/// installs dlc into data (data/dlc/<title id>).
pub fn install(dlc: &Dlc, data: &Path) -> Result<String, String> {
    let dir = data.join("dlc").join(format!("{:016X}", dlc.title_id));
    match &dlc.kind {
        Kind::Cia => {
            let tally = zakuro_fs::datadir::install_dlc(&dlc.path, data, false).map_err(|e| crate::clean_error(&e.to_string()))?;
            Ok(format!("{} files, {} MB", tally.files, tally.bytes / 1_000_000))
        }
        Kind::Haiku => {
            let mut count = 0;
            if dir.exists() && std::fs::canonicalize(&dir).ok() == std::fs::canonicalize(&dlc.path).ok() {
                return Ok("already in place".into());
            }
            copy_tree(&dlc.path, &dir, &mut count).map_err(|e| format!("copying the DLC: {e}"))?;
            Ok(format!("{count} files copied"))
        }
        Kind::Installed { tmd, .. } => {
            let tmd_bytes = std::fs::read(tmd).map_err(|e| e.to_string())?;
            let (_, _, records) = dump::tmd_contents(&tmd_bytes).ok_or("the .tmd is damaged")?;
            let listed: Vec<zakuro_fs::Content> = records
                .iter()
                .map(|(index, id, flags, size, _)| zakuro_fs::Content { index: *index, id: *id, flags: *flags & !1, size: *size, offset: None })
                .collect();
            let apps: Vec<(u16, PathBuf)> = dlc.contents.iter().map(|c| (c.0, c.5.clone().unwrap())).collect();
            let tally = zakuro_fs::datadir::install_dlc_apps(dlc.title_id, dlc.version, &listed, &apps, data).map_err(|e| crate::clean_error(&e.to_string()))?;
            Ok(format!("{} files, {} MB", tally.files, tally.bytes / 1_000_000))
        }
    }
}
