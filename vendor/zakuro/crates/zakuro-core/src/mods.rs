//! mods, kept in the data directory's mods folder under the title's id, laid
//! out the way Luma3DS and Citra take them.

use std::path::{Path, PathBuf};

use zakuro_common::memory_map::PAGE_SIZE;
use zakuro_fs::ncch::ExHeader;
use zakuro_fs::{lz77, patch, FsError, Title};

/// the folder a title's mods go in.
pub fn dir(data_dir: &Path, program_id: u64) -> PathBuf {
    let mods = data_dir.join("mods");
    let upper = mods.join(format!("{program_id:016X}"));
    let lower = mods.join(format!("{program_id:016x}"));
    if !upper.exists() && lower.exists() { lower } else { upper }
}

/// the folder of a title's texture pack, in its mods folder, laid out as in
/// Citra's load/textures/<title id>.
pub fn textures(data_dir: &Path, program_id: u64) -> PathBuf {
    dir(data_dir, program_id).join("textures")
}

/// the files that change a title's code, each in the mod's folder or in its
/// exefs folder.
const CODE: [&str; 3] = ["code.bin", "code.ips", "code.bps"];

/// the extended header a mod gives in place of the title's, in its folder,
/// which a mod that makes the code longer needs.
const EXHEADER: &str = "exheader.bin";

/// mods: whether a mod's code changes (code.bin, code.ips, code.bps,
/// exheader.bin) are applied. Haiku runs the game's code recompiled ahead
/// of time with no interpreter, so code a mod changed could not run: it
/// turns them off, and its mod manager tells the player instead.
static CODE_PATCHES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// mods: turns a mod's code changes on or off, see CODE_PATCHES.
pub fn allow_code_patches(allowed: bool) {
    CODE_PATCHES.store(allowed, std::sync::atomic::Ordering::Relaxed);
}

fn code_patches() -> bool {
    CODE_PATCHES.load(std::sync::atomic::Ordering::Relaxed)
}

/// mods: the files in a mod's content folder that change the game's code,
/// which code_patches() leaves out: the code itself, patches to it, and an
/// exheader, in the folder or its exefs/.
pub fn code_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = CODE.iter().filter_map(|name| find(dir, name)).collect();
    found.extend([dir.join(EXHEADER), dir.join("exefs").join(EXHEADER)].into_iter().filter(|path| path.is_file()));
    found
}

/// whether the title's mods folder holds a mod.
pub fn present(data_dir: &Path, program_id: u64) -> bool {
    let dir = dir(data_dir, program_id);
    let files = ["romfs", "romfs_ext"].iter().any(|part| std::fs::read_dir(dir.join(part)).is_ok_and(|mut entries| entries.next().is_some()));
    files || CODE.iter().any(|name| find(&dir, name).is_some()) || dir.join(EXHEADER).is_file()
}

/// the exheader.bin of the title's mods, the 0x400 bytes that describe the
/// program and what they say, when it looks like the title's, its code
/// starting where the title's does, as an exheader still encrypted would
/// not. the code in the ExeFS is the game's own, so whether it is
/// compressed stays as the game's exheader says.
pub fn exheader(title: &Title, data_dir: &Path) -> Option<(Vec<u8>, ExHeader)> {
    let path = dir(data_dir, title.program_id()).join(EXHEADER);
    if !code_patches() {
        if path.is_file() {
            log::warn!("mods: {} changes the game's code, which this build can't run, it is left out", path.display());
        }
        return None;
    }
    let mut bytes = std::fs::read(&path).ok()?;
    let Some(described) = bytes.get(..ExHeader::HASHED_SIZE) else {
        log::warn!("mods: {} is {} bytes, an exheader has {}", path.display(), bytes.len(), ExHeader::HASHED_SIZE);
        return None;
    };
    match ExHeader::parse(described) {
        Ok(mut exheader) if exheader.text.address == title.exheader.text.address => {
            exheader.compress_code = title.exheader.compress_code;
            bytes.truncate(ExHeader::HASHED_SIZE);
            bytes[0x0D] = bytes[0x0D] & !1 | title.exheader.compress_code as u8;
            Some((bytes, exheader))
        }
        Ok(_) => {
            log::warn!("mods: {} is not this game's exheader, or is still encrypted, it is left out", path.display());
            None
        }
        Err(error) => {
            log::warn!("mods: {}: {error}", path.display());
            None
        }
    }
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    [dir.join("exefs").join(name), dir.join(name)].into_iter().find(|path| path.is_file())
}

/// a patch format, applying a patch to data.
type Patch = fn(&[u8], &[u8]) -> Result<Vec<u8>, FsError>;

/// the title's code with its mods' changes to it, a code.bin in its place,
/// then a code.ips or code.bps patch over it, and whether there were any.
pub fn code(title: &Title, data_dir: Option<&Path>) -> Result<(Vec<u8>, bool), FsError> {
    let original = title.code()?;
    let Some(data_dir) = data_dir else {
        return Ok((original, false));
    };
    let mut code = original.clone();
    let dir = dir(data_dir, title.program_id());
    if !code_patches() {
        for path in code_files(&dir) {
            log::warn!("mods: {} changes the game's code, which this build can't run, it is left out", path.display());
        }
        return Ok((original, false));
    }
    if let Some(path) = find(&dir, "code.bin") {
        match std::fs::read(&path) {
            Ok(bytes) => {
                code = decompressed(bytes, title);
                log::info!("mods: the game's code is {}", path.display());
            }
            Err(error) => log::warn!("mods: {}: {error}", path.display()),
        }
    }
    let patches: [(&str, Patch); 2] = [("code.ips", patch::ips), ("code.bps", patch::bps)];
    for (name, apply) in patches {
        let Some(path) = find(&dir, name) else { continue };
        match std::fs::read(&path).map_err(FsError::from).and_then(|patch| apply(&patch, &code)) {
            Ok(patched) => {
                code = patched;
                log::info!("mods: {} patched the game's code", path.display());
            }
            Err(error) => log::warn!("mods: {}: {error}", path.display()),
        }
    }
    let modded = code != original;
    Ok((code, modded))
}

/// a code.bin as the game runs it. most are kept decompressed, one that is
/// too short for the segments the exheader describes is still compressed.
fn decompressed(bytes: Vec<u8>, title: &Title) -> Vec<u8> {
    let exheader = &title.exheader;
    let needed = (exheader.text.num_pages + exheader.rodata.num_pages) * PAGE_SIZE + exheader.data.size;
    if bytes.len() >= needed as usize {
        return bytes;
    }
    lz77::decompress(&bytes).unwrap_or(bytes)
}

/// mods: lays the title's mods over it, the mods folder in data_dir and
/// then the content folders in mods, in order, saying in the log what they
/// changed and how long it took.
pub fn lay_all(title: &mut Title, data_dir: Option<&Path>, mods: &[PathBuf]) {
    lay(title, data_dir);
    if mods.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    // the mods folder's own first, when it changed anything
    let legacy = data_dir.map(|data_dir| dir(data_dir, title.program_id())).filter(|dir| dir.join("romfs").is_dir() || dir.join("romfs_ext").is_dir());
    let mut dirs: Vec<&Path> = legacy.iter().map(PathBuf::as_path).collect();
    dirs.extend(mods.iter().map(PathBuf::as_path));
    match title.lay_mod_dirs(&dirs) {
        Ok(Some(changes)) => log::info!(
            "mods: {} mods laid over the game's files in {} ms: {} files replaced, {} added, {} patched, {} removed",
            mods.len(),
            started.elapsed().as_millis(),
            changes.replaced,
            changes.added,
            changes.patched,
            changes.removed
        ),
        Ok(None) => log::info!("mods: the {} mods turned on change nothing in the game's files", mods.len()),
        Err(error) => log::warn!("mods: the mods can't be laid over the game, {error}; playing without them"),
    }
}

/// mods: lays the mods' DLC files over the DLC installed as folders. A
/// mod's dlc/<content index>/ folder (4 hex digits) is laid over that
/// content's RomFS. paths are the DLC folders the game was given.
pub fn lay_dlc(dlc: &mut [zakuro_fs::Dlc], paths: &[PathBuf], mods: &[PathBuf]) {
    if mods.is_empty() {
        return;
    }
    for dlc in dlc.iter_mut().filter(|dlc| !dlc.folders.is_empty()) {
        let name = format!("{:016X}", dlc.title_id);
        let Some(base) = paths.iter().find(|path| path.is_dir() && path.file_name().is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(&name))) else {
            continue;
        };
        for (index, layered) in dlc.folders.iter_mut() {
            let content = format!("{index:04X}");
            let dirs: Vec<PathBuf> = mods.iter().map(|path| path.join("dlc").join(&content)).filter(|path| path.is_dir()).collect();
            if dirs.is_empty() {
                continue;
            }
            let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
            match zakuro_fs::datadir::romfs_folder_with_mods(&base.join(&content).join(zakuro_fs::datadir::ROMFS_DIR), &refs) {
                Ok(modded) => {
                    let changes = modded.changes;
                    log::info!("mods: DLC content {content}: {} files replaced, {} added, {} patched, {} removed", changes.replaced, changes.added, changes.patched, changes.removed);
                    *layered = std::sync::Arc::new(modded);
                }
                Err(error) => log::warn!("mods: the mods can't be laid over DLC content {content}, {error}"),
            }
        }
    }
}

/// lays the title's mods over it, saying in the log what they changed.
pub fn lay(title: &mut Title, data_dir: Option<&Path>) {
    let Some(data_dir) = data_dir else {
        return;
    };
    let dir = dir(data_dir, title.program_id());
    if !dir.is_dir() {
        return;
    }
    if let Some((_, exheader)) = exheader(title, data_dir) {
        log::info!("mods: the game's exheader is {}", dir.join(EXHEADER).display());
        title.exheader = exheader;
    }
    match title.lay_mods(&dir) {
        Ok(Some(changes)) => log::info!(
            "mods from {}: {} files replaced, {} added, {} patched, {} removed",
            dir.display(),
            changes.replaced,
            changes.added,
            changes.patched,
            changes.removed
        ),
        Ok(None) => log::info!("mods: {} changes nothing in the game's files", dir.display()),
        Err(error) => log::warn!("mods: {} can't be laid over the game: {error}", dir.display()),
    }
    // and over the update's files, which a game reads besides its own
    if let Some(update) = title.update_mut() {
        match update.title.lay_mods(&dir) {
            Ok(Some(changes)) => log::info!(
                "mods over the update's files: {} replaced, {} added, {} patched, {} removed",
                changes.replaced,
                changes.added,
                changes.patched,
                changes.removed
            ),
            Ok(None) => {}
            Err(error) => log::warn!("mods: {} can't be laid over the update: {error}", dir.display()),
        }
    }
}
