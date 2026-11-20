//! a game's RomFS with a mod laid over it, from the folders Luma3DS and
//! Citra take mods in. romfs/ holds files that replace the game's or are
//! added to them, romfs_ext/ holds .ips and .bps patches for the game's
//! files and .stub files, each taking away the file it is named after.
//! the game's own file data stays where it is in the ROM, the new tree's
//! metadata goes in front of it and the mod's files after it.

use std::path::{Path, PathBuf};

use crate::romfs::RomFs;
use crate::romfs_build::{self, Dir, File};
use crate::{patch, FsError};

/// what a mod changed in the RomFS.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changes {
    pub replaced: usize,
    pub added: usize,
    pub patched: usize,
    pub removed: usize,
}

impl Changes {
    pub fn any(&self) -> bool {
        self.replaced + self.added + self.patched + self.removed > 0
    }
}

/// where some of the layered image's bytes come from.
enum Source {
    /// the ROM image, from this offset.
    Image(u64),
    /// a file of the mod's.
    Host(PathBuf),
    /// a patched file, kept whole.
    Memory(Vec<u8>),
    /// tones: a file with alternates of the same size (the base first),
    /// read from the one ALTERNATE picks.
    Choice(Vec<PathBuf>),
}

/// tones: which alternate of a file with alternates is read (0: the base,
/// k: a mod's `alt/k/romfs` copy; past the last, the base). Haiku's text
/// layer gives the Japanese game's speaking styles (talk tones) this way:
/// every alternate is the same size, so the RomFS's tables never change and
/// the next read of a file gets the style chosen.
pub static ALTERNATE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// tones: the path a Choice reads now, and its index.
fn chosen(paths: &[PathBuf]) -> (usize, &Path) {
    let pick = ALTERNATE.load(std::sync::atomic::Ordering::Relaxed);
    let pick = if pick < paths.len() { pick } else { 0 };
    (pick, &paths[pick])
}

/// a stretch of the layered image after the metadata.
struct Piece {
    start: u64,
    size: u64,
    source: Source,
}

/// a game's RomFS with a mod over it, read like the level-3 image it
/// stands in for.
pub struct Layered {
    /// the header and the metadata tables, up to where the file data starts.
    meta: Vec<u8>,
    /// the file data, in order.
    pieces: Vec<Piece>,
    size: u64,
    pub changes: Changes,
    /// os: host files kept open, by piece, the latest last, so a folder
    /// game's reads don't open a file each time.
    open: std::sync::Mutex<Vec<(usize, std::fs::File)>>,
}

/// os: how many host files a layered image keeps open.
const KEPT_OPEN: usize = 16;

impl std::fmt::Debug for Layered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layered").field("size", &self.size).field("changes", &self.changes).finish()
    }
}

impl Layered {
    /// romfs, a RomFS in image, with the mod in dir over it. None when the
    /// mod changes nothing in it.
    pub fn new(romfs: &RomFs, image: &[u8], dir: &Path) -> Result<Option<Layered>, FsError> {
        Layered::with_mods(romfs, image, &[dir])
    }

    /// mods: romfs, a RomFS in image, with several mods over it, each laid
    /// over the ones before it, so a later mod's file wins over an earlier
    /// one's. Each folder in dirs is a mod's content: its romfs/ and
    /// romfs_ext/. None when they change nothing.
    pub fn with_mods(romfs: &RomFs, image: &[u8], dirs: &[&Path]) -> Result<Option<Layered>, FsError> {
        let mut tree = Tree::read(romfs)?;
        let content = tree.content();
        let mut changes = Changes::default();
        for dir in dirs {
            tree.lay_mod(dir, content, Some((romfs, image)), &mut changes)?;
        }
        if !changes.any() {
            return Ok(None);
        }
        Ok(Some(tree.layout(Some(romfs.base + romfs.header.file_data_offset as u64), changes)))
    }

    /// os: a RomFS read from folders alone, a game installed as a folder,
    /// each folder laid over the ones before it. the image is built in
    /// memory from the folders' tree, and file data is read from the host
    /// files themselves.
    pub fn from_dirs(dirs: &[&Path]) -> Result<Layered, FsError> {
        let mut tree = Tree { dirs: vec![dir(String::new(), 0)], leaves: Vec::new(), game_data: 0 };
        let mut changes = Changes::default();
        for host in dirs {
            tree.lay(host, 0, &mut changes)?;
        }
        Ok(tree.layout(None, changes))
    }

    /// os: a RomFS read from a folder whose files are listed (path from the
    /// folder, / separated, and size), as the installer writes them, so no
    /// directory has to be listed at start.
    pub fn from_list(root: &Path, list: &[(String, u64)]) -> Layered {
        let tree = Tree::from_list(root, list);
        let changes = Changes { added: tree.leaves.len(), ..Changes::default() };
        tree.layout(None, changes)
    }

    /// mods: a RomFS read from a folder whose files are listed, as
    /// from_list, with mods laid over it as with_mods does. The changes
    /// back are the mods' alone.
    pub fn from_list_with_mods(root: &Path, list: &[(String, u64)], dirs: &[&Path]) -> Result<Layered, FsError> {
        let mut tree = Tree::from_list(root, list);
        let mut changes = Changes::default();
        for dir in dirs {
            tree.lay_mod(dir, 0, None, &mut changes)?;
        }
        Ok(tree.layout(None, changes))
    }

    /// os: how many files the image holds.
    pub fn file_count(&self) -> usize {
        self.pieces.iter().filter(|piece| matches!(piece.source, Source::Host(_) | Source::Memory(_) | Source::Choice(_))).count()
    }

    /// how many bytes the layered image has.
    pub fn len(&self) -> u64 {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// statecheck: a digest of where every file is in the image, the
    /// metadata tables (each file's name, offset and size) and the image's
    /// length. Two layered images with the same digest put every file at the
    /// same place, so a saved state's open files and the game's cached
    /// offsets mean the same in both. Where the bytes come from (which
    /// folder, which host path) is left out, so a moved install keeps it.
    pub fn layout_digest(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.meta.iter().copied().chain(self.size.to_le_bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    /// reads the layered image from offset into out, image being the ROM's
    /// bytes. how many bytes there were to read.
    pub fn read(&self, image: &[u8], offset: u64, out: &mut [u8]) -> usize {
        if offset >= self.size {
            return 0;
        }
        let count = (out.len() as u64).min(self.size - offset) as usize;
        let out = &mut out[..count];
        // what no piece covers is padding
        out.fill(0);
        let end = offset + count as u64;

        let meta_end = self.meta.len() as u64;
        if offset < meta_end {
            let to = meta_end.min(end);
            out[..(to - offset) as usize].copy_from_slice(&self.meta[offset as usize..to as usize]);
        }

        let first = self.pieces.partition_point(|piece| piece.start + piece.size <= offset);
        for (index, piece) in self.pieces.iter().enumerate().skip(first).take_while(|(_, piece)| piece.start < end) {
            let from = piece.start.max(offset);
            let to = (piece.start + piece.size).min(end);
            if from >= to {
                continue;
            }
            let dest = &mut out[(from - offset) as usize..(to - offset) as usize];
            let within = from - piece.start;
            match &piece.source {
                Source::Image(at) => {
                    let start = (at + within) as usize;
                    if let Some(bytes) = image.get(start..start + dest.len()) {
                        dest.copy_from_slice(bytes);
                    }
                }
                Source::Memory(data) => dest.copy_from_slice(&data[within as usize..within as usize + dest.len()]),
                Source::Host(path) => self.read_kept(index, path, within, dest),
                // tones: each alternate kept open on a key of its own
                Source::Choice(paths) => {
                    let (pick, path) = chosen(paths);
                    self.read_kept(index | (pick << 48), path, within, dest)
                }
            }
        }
        count
    }

    /// os: reads a host file through the files kept open.
    fn read_kept(&self, index: usize, path: &Path, at: u64, out: &mut [u8]) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut open) = self.open.lock() else { return read_host(path, at, out) };
        let file = match open.iter().position(|(piece, _)| *piece == index) {
            Some(found) => {
                let kept = open.remove(found);
                open.push(kept);
                &mut open.last_mut().unwrap().1
            }
            None => {
                trace_open(path); // jpvoice: HAIKU_ROMFS_TRACE=1
                let Ok(file) = std::fs::File::open(path) else {
                    log::warn!("romfs: {} can't be read", path.display());
                    return;
                };
                if open.len() >= KEPT_OPEN {
                    open.remove(0);
                }
                open.push((index, file));
                &mut open.last_mut().unwrap().1
            }
        };
        if file.seek(SeekFrom::Start(at)).is_err() {
            return;
        }
        let mut filled = 0;
        while filled < out.len() {
            match file.read(&mut out[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(read) => filled += read,
            }
        }
    }

    /// the files the mod put in, replacing, patching or adding them, whose
    /// paths wanted takes, with their bytes. a path goes from where the
    /// mod's paths start, / separated.
    pub fn modded_files(&self, wanted: impl Fn(&str) -> bool) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        let Ok(romfs) = RomFs::parse_level3(&self.meta, 0) else { return files };
        let Ok(root) = romfs.root() else { return files };
        let mut pending = vec![(String::new(), root)];
        while let Some((path, dir)) = pending.pop() {
            for (_, file) in romfs.files(&dir) {
                let path = join(&path, &file.name);
                if !wanted(&path) {
                    continue;
                }
                if let Some(bytes) = self.modded(romfs.file_data_offset(&file), file.data_size) {
                    files.push((path, bytes));
                }
            }
            for (_, child) in romfs.subdirs(&dir) {
                pending.push((join(&path, &child.name), child));
            }
        }
        files
    }

    /// the size bytes at start in the image, when they are a file the mod
    /// put there.
    fn modded(&self, start: u64, size: u64) -> Option<Vec<u8>> {
        let piece = self.pieces.iter().find(|piece| piece.start == start && piece.size == size)?;
        let mut bytes = vec![0; size as usize];
        match &piece.source {
            Source::Image(_) => return None,
            Source::Memory(data) => bytes.copy_from_slice(data),
            Source::Host(path) => read_host(path, 0, &mut bytes),
            Source::Choice(paths) => read_host(chosen(paths).1, 0, &mut bytes), // tones
        }
        Some(bytes)
    }
}

/// a file of the tree being put together.
struct Leaf {
    name: String,
    size: u64,
    /// where its data is from the start of the game's file data, while it
    /// is the game's.
    game: Option<u64>,
    /// what the mod puts in its place.
    modded: Option<Source>,
}

/// the game's tree, with the mod's changes made to it.
struct Tree {
    dirs: Vec<Dir>,
    leaves: Vec<Leaf>,
    /// where the game's file data ends.
    game_data: u64,
}

impl Tree {
    /// mods: the tree of a folder whose files are listed, as the installer
    /// writes them.
    fn from_list(root: &Path, list: &[(String, u64)]) -> Tree {
        let mut tree = Tree { dirs: vec![dir(String::new(), 0)], leaves: Vec::new(), game_data: 0 };
        let mut dirs: std::collections::HashMap<(usize, String), usize> = std::collections::HashMap::new();
        for (path, size) in list {
            let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
            let Some(name) = parts.pop() else { continue };
            let mut at = 0;
            let mut host = root.to_path_buf();
            for part in parts {
                host.push(part);
                at = *dirs.entry((at, part.to_owned())).or_insert_with(|| {
                    let child = tree.dirs.len();
                    tree.dirs.push(dir(part.to_owned(), at));
                    tree.dirs[at].children.push(child);
                    child
                });
            }
            host.push(name);
            let leaf = tree.leaves.len();
            tree.leaves.push(Leaf { name: name.to_owned(), size: *size, game: None, modded: Some(Source::Host(host)) });
            tree.dirs[at].files.push(leaf);
        }
        tree
    }

    /// mods: a mod's content folder over the directory at: its romfs/
    /// files, then its romfs_ext/ patches and stubs.
    fn lay_mod(&mut self, dir: &Path, at: usize, game: Option<(&RomFs, &[u8])>, changes: &mut Changes) -> Result<(), FsError> {
        let files = dir.join("romfs");
        if files.is_dir() {
            self.lay(&files, at, changes)?;
        }
        let patches = dir.join("romfs_ext");
        if patches.is_dir() {
            self.patch(&patches, at, game, changes)?;
        }
        self.alternates(dir, at); // tones
        Ok(())
    }

    /// tones: a mod's `alt/<k>/romfs` folders (k = 1, 2...) hold alternates
    /// of files its romfs/ put in, each the same size as the file: the file
    /// becomes a Choice that ALTERNATE picks from. An alternate of another
    /// size, or of a file romfs/ didn't put in, is left out with a warning.
    fn alternates(&mut self, dir: &Path, at: usize) {
        let alt = dir.join("alt");
        if !alt.is_dir() {
            return;
        }
        for k in 1..8usize {
            let root = alt.join(k.to_string()).join("romfs");
            if !root.is_dir() {
                break;
            }
            let mut files = Vec::new();
            walk_files(&root, "", &mut files);
            for (rel, path, size) in files {
                let mut parts: Vec<&str> = rel.split('/').collect();
                let Some(name) = parts.pop() else { continue };
                let found = parts.iter().try_fold(at, |p, part| self.find_dir(p, part)).and_then(|p| self.find_file(p, name));
                let Some(leaf) = found else {
                    log::warn!("mods: the alternate {} is for a file the layer doesn't put in", path.display());
                    continue;
                };
                let leaf = &mut self.leaves[leaf];
                if leaf.size != size {
                    log::warn!("mods: the alternate {} is {size} bytes, its file {}", path.display(), leaf.size);
                    continue;
                }
                let paths = match leaf.modded.take() {
                    Some(Source::Host(base)) => vec![base],
                    Some(Source::Choice(paths)) => paths,
                    other => {
                        leaf.modded = other;
                        log::warn!("mods: the alternate {} is for a file the layer doesn't put in", path.display());
                        continue;
                    }
                };
                let mut paths = paths;
                while paths.len() < k {
                    let base = paths[0].clone();
                    paths.push(base);
                }
                paths.truncate(k);
                paths.push(path);
                leaf.modded = Some(Source::Choice(paths));
            }
        }
    }

    fn read(romfs: &RomFs) -> Result<Tree, FsError> {
        let root = romfs.root()?;
        let mut tree = Tree { dirs: vec![dir(root.name.clone(), 0)], leaves: Vec::new(), game_data: 0 };
        let mut pending = vec![(root, 0)];
        while let Some((entry, index)) = pending.pop() {
            for (_, file) in romfs.files(&entry) {
                tree.game_data = tree.game_data.max(file.data_offset + file.data_size);
                tree.leaves.push(Leaf { name: file.name, size: file.data_size, game: Some(file.data_offset), modded: None });
                tree.dirs[index].files.push(tree.leaves.len() - 1);
            }
            for (_, child) in romfs.subdirs(&entry) {
                tree.dirs.push(dir(child.name.clone(), index));
                let child_index = tree.dirs.len() - 1;
                tree.dirs[index].children.push(child_index);
                pending.push((child, child_index));
            }
        }
        Ok(tree)
    }

    /// the directory a mod's paths start from, the root, or the nameless
    /// one under it some images keep everything in.
    fn content(&self) -> usize {
        match self.dirs[0].children.as_slice() {
            &[only] if self.dirs[0].files.is_empty() && self.dirs[only].name.is_empty() => only,
            _ => 0,
        }
    }

    /// the directory named name in parent. names are matched exactly first,
    /// then ignoring case, as mods made on Windows can have them either way.
    fn find_dir(&self, parent: usize, name: &str) -> Option<usize> {
        let children = &self.dirs[parent].children;
        let by = |same: &dyn Fn(&str) -> bool| children.iter().copied().find(|&child| same(&self.dirs[child].name));
        by(&|other| other == name).or_else(|| by(&|other| other.eq_ignore_ascii_case(name)))
    }

    /// the file named name in parent, as a leaf.
    fn find_file(&self, parent: usize, name: &str) -> Option<usize> {
        let files = &self.dirs[parent].files;
        let by = |same: &dyn Fn(&str) -> bool| files.iter().copied().find(|&leaf| same(&self.leaves[leaf].name));
        by(&|other| other == name).or_else(|| by(&|other| other.eq_ignore_ascii_case(name)))
    }

    /// the mod's files in host, over the directory at.
    fn lay(&mut self, host: &Path, at: usize, changes: &mut Changes) -> Result<(), FsError> {
        // os: a directory with nothing in it yet has nothing to replace,
        // which skips the searches (a folder game lays 21,880 files)
        let fresh = self.dirs[at].files.is_empty() && self.dirs[at].children.is_empty();
        for (path, name, is_dir, size) in entries_with_kind(host)? {
            if is_dir {
                let found = if fresh { None } else { self.find_dir(at, &name) };
                let child = match found {
                    Some(child) => child,
                    None => {
                        let child = self.dirs.len();
                        self.dirs.push(dir(name, at));
                        self.dirs[at].children.push(child);
                        child
                    }
                };
                self.lay(&path, child, changes)?;
                continue;
            }
            let found = if fresh { None } else { self.find_file(at, &name) };
            match found {
                Some(leaf) => {
                    self.leaves[leaf].size = size;
                    self.leaves[leaf].modded = Some(Source::Host(path));
                    changes.replaced += 1;
                }
                None => {
                    let leaf = self.leaves.len();
                    self.leaves.push(Leaf { name, size, game: None, modded: Some(Source::Host(path)) });
                    self.dirs[at].files.push(leaf);
                    changes.added += 1;
                }
            }
        }
        Ok(())
    }

    /// the patches and stubs in host, for the files of the directory at.
    fn patch(&mut self, host: &Path, at: usize, game: Option<(&RomFs, &[u8])>, changes: &mut Changes) -> Result<(), FsError> {
        for (path, name) in entries(host)? {
            if path.is_dir() {
                match self.find_dir(at, &name) {
                    Some(child) => self.patch(&path, child, game, changes)?,
                    None => log::warn!("mods: {} is for a directory the game doesn't have", path.display()),
                }
                continue;
            }
            let Some((target, kind)) = name.rsplit_once('.') else {
                log::warn!("mods: {} is not a patch or a stub", path.display());
                continue;
            };
            let kind = kind.to_ascii_lowercase();
            if !["stub", "ips", "bps"].contains(&kind.as_str()) {
                log::warn!("mods: {} is not a patch or a stub", path.display());
                continue;
            }
            let Some(leaf) = self.find_file(at, target) else {
                log::warn!("mods: {} is for a file the game doesn't have", path.display());
                continue;
            };
            if kind == "stub" {
                self.dirs[at].files.retain(|&other| other != leaf);
                changes.removed += 1;
                continue;
            }
            let patch = std::fs::read(&path)?;
            let data = self.data(leaf, game)?;
            let patched = if kind == "ips" { patch::ips(&patch, &data) } else { patch::bps(&patch, &data) };
            match patched {
                Ok(patched) => {
                    self.leaves[leaf].size = patched.len() as u64;
                    self.leaves[leaf].modded = Some(Source::Memory(patched));
                    changes.patched += 1;
                }
                Err(error) => log::warn!("mods: {}: {error}", path.display()),
            }
        }
        Ok(())
    }

    /// a leaf's bytes as they are now, a patch goes over what romfs/ put
    /// there when it put something.
    fn data(&self, leaf: usize, game: Option<(&RomFs, &[u8])>) -> Result<Vec<u8>, FsError> {
        let leaf = &self.leaves[leaf];
        match (&leaf.modded, leaf.game, game) {
            (Some(Source::Host(path)), _, _) => Ok(std::fs::read(path)?),
            (Some(Source::Memory(data)), _, _) => Ok(data.clone()),
            (Some(Source::Choice(paths)), _, _) => Ok(std::fs::read(&paths[0])?), // tones
            (_, Some(at), Some((romfs, image))) => {
                let start = (romfs.base + romfs.header.file_data_offset as u64 + at) as usize;
                let bytes = image.get(start..start + leaf.size as usize);
                Ok(bytes.ok_or(FsError::BadRomFs("a file runs past the end of the image"))?.to_vec())
            }
            _ => Ok(Vec::new()),
        }
    }

    /// the image, the game's file data keeping its offsets and the mod's
    /// coming after it.
    fn layout(mut self, game_at: Option<u64>, changes: Changes) -> Layered {
        let mut end = self.game_data.next_multiple_of(16);
        let mut files = Vec::new();
        let mut modded = Vec::new();
        for index in 0..self.dirs.len() {
            for leaf in std::mem::take(&mut self.dirs[index].files) {
                let leaf = &mut self.leaves[leaf];
                let data_offset = match leaf.modded.take() {
                    Some(source) => {
                        let at = end;
                        end = (end + leaf.size).next_multiple_of(16);
                        modded.push(Piece { start: at, size: leaf.size, source });
                        at
                    }
                    None => leaf.game.unwrap_or(0),
                };
                self.dirs[index].files.push(files.len());
                files.push(File { name: std::mem::take(&mut leaf.name), parent: index, data_offset, data_size: leaf.size, offset: 0 });
            }
        }

        let meta = romfs_build::metadata(&mut self.dirs, &mut files);
        let base = meta.len() as u64;
        let mut pieces = Vec::new();
        if let Some(at) = game_at {
            pieces.push(Piece { start: base, size: self.game_data, source: Source::Image(at) });
        }
        pieces.extend(modded.into_iter().map(|piece| Piece { start: base + piece.start, ..piece }));
        Layered { meta, pieces, size: base + end, changes, open: std::sync::Mutex::new(Vec::new()) }
    }
}

fn dir(name: String, parent: usize) -> Dir {
    Dir { name, parent, children: Vec::new(), files: Vec::new(), offset: 0 }
}

/// the path of name in the directory at path. paths skip the nameless
/// directories, the root and the one some images keep everything in.
fn join(path: &str, name: &str) -> String {
    if path.is_empty() || name.is_empty() {
        format!("{path}{name}")
    } else {
        format!("{path}/{name}")
    }
}

/// a host directory's entries and their names, in order of name, so a
/// mod lays out the same way every time.
/// modsweep: what a mod's download carries that is never a file of the
/// game: a Windows shortcut (Fates+ 3.5 has "Terrain - Shortcut.lnk" in
/// GameData/Terrain), Explorer's and the Mac's folder notes (.DS_Store,
/// Thumbs.db, desktop.ini) and the Mac's zip copies (__MACOSX/, ._ files;
/// Abyssal Bloodline 1.2.1). Such a file is left out of the mod's RomFS.
pub fn is_junk(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".lnk") || lower.starts_with("._") || lower == "__macosx" || lower == ".ds_store" || lower == "thumbs.db" || lower == "desktop.ini"
}

/// os: entries() with whether each is a directory and its size, from the
/// directory listing itself, without opening each file.
fn entries_with_kind(host: &Path) -> Result<Vec<(PathBuf, String, bool, u64)>, FsError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(host)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        match path.file_name().and_then(|name| name.to_str()) {
            Some(name) if is_junk(name) => {} // modsweep
            Some(name) => entries.push((path.clone(), name.to_owned(), metadata.is_dir(), metadata.len())),
            None => log::warn!("mods: {} has a name a RomFS can't hold", path.display()),
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(entries)
}

fn entries(host: &Path) -> Result<Vec<(PathBuf, String)>, FsError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(host)? {
        let path = entry?.path();
        match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => entries.push((path.clone(), name.to_owned())),
            None => log::warn!("mods: {} has a name a RomFS can't hold", path.display()),
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(entries)
}

/// reads a mod's file from at into out, as much of it as there is.
/// jpvoice: with HAIKU_ROMFS_TRACE=1, says in the log the first time each
/// host file of the RomFS (the installed game's or a layer's) is opened, so
/// a test can tell which file a sound came from. Off, it costs one check of
/// a flag read once.
fn trace_open(path: &Path) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static SEEN: std::sync::Mutex<Option<std::collections::HashSet<PathBuf>>> = std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var("HAIKU_ROMFS_TRACE").is_ok_and(|value| value == "1")) {
        return;
    }
    if let Ok(mut seen) = SEEN.lock() {
        if seen.get_or_insert_with(Default::default).insert(path.to_path_buf()) {
            log::info!("romfs: opened {}", path.display());
        }
    }
}

fn read_host(path: &Path, at: u64, out: &mut [u8]) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        log::warn!("mods: {} can't be read", path.display());
        return;
    };
    if file.seek(SeekFrom::Start(at)).is_err() {
        return;
    }
    let mut filled = 0;
    while filled < out.len() {
        match file.read(&mut out[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::romfs_build::BuildFile;

    /// a folder for one test's mod, empty.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zakuro-mods-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, path: &str, data: &[u8]) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    /// a game's RomFS, its files at the root as games have them.
    fn game(files: &[(&str, &[u8])]) -> Vec<u8> {
        let files: Vec<BuildFile> = files.iter().map(|(path, data)| BuildFile { path: (*path).into(), data: data.to_vec() }).collect();
        romfs_build::assemble(&files, false)
    }

    /// the whole layered image, read the way a game reads it.
    fn whole(layered: &Layered, image: &[u8]) -> Vec<u8> {
        let mut out = vec![0xAA; layered.len() as usize];
        assert_eq!(layered.read(image, 0, &mut out), out.len());
        out
    }

    fn file(image: &[u8], path: &str) -> Option<Vec<u8>> {
        let romfs = RomFs::parse_level3(image, 0).unwrap();
        let entry = romfs.lookup(path).ok()?;
        let start = romfs.file_data_offset(&entry) as usize;
        Some(image[start..start + entry.data_size as usize].to_vec())
    }

    #[test]
    fn a_mod_replaces_adds_patches_and_removes_files() {
        let image = game(&[
            ("Data/model.bin", b"old model"),
            ("Data/text.bin", b"hello"),
            ("Data/unused.bin", b"gone soon"),
            ("Sound/bgm.bin", b"music"),
            ("top.bin", b"top"),
        ]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("all");
        put(&mods, "romfs/Data/model.bin", b"a new, bigger model");
        put(&mods, "romfs/Data/extra.bin", b"extra");
        put(&mods, "romfs/New/inside.bin", b"inside");
        let mut ips = b"PATCH".to_vec();
        ips.extend_from_slice(&[0, 0, 0, 0, 1, b'J']);
        ips.extend_from_slice(b"EOF");
        put(&mods, "romfs_ext/Data/text.bin.ips", &ips);
        put(&mods, "romfs_ext/Data/unused.bin.stub", b"");

        let layered = Layered::new(&romfs, &image, &mods).unwrap().expect("the mod changes the RomFS");
        assert_eq!(layered.changes, Changes { replaced: 1, added: 2, patched: 1, removed: 1 });
        let layered_image = whole(&layered, &image);
        assert_eq!(file(&layered_image, "Data/model.bin").unwrap(), b"a new, bigger model");
        assert_eq!(file(&layered_image, "Data/extra.bin").unwrap(), b"extra");
        assert_eq!(file(&layered_image, "New/inside.bin").unwrap(), b"inside");
        assert_eq!(file(&layered_image, "Data/text.bin").unwrap(), b"Jello");
        assert_eq!(file(&layered_image, "Data/unused.bin"), None);
        assert_eq!(file(&layered_image, "Sound/bgm.bin").unwrap(), b"music");
        assert_eq!(file(&layered_image, "top.bin").unwrap(), b"top");
        std::fs::remove_dir_all(mods).unwrap();
    }

    /// reads that start and end anywhere get the same bytes as reading the
    /// whole image, games read in chunks.
    #[test]
    fn reads_in_pieces_match_the_whole() {
        let image = game(&[("a.bin", &[1; 100]), ("b.bin", &[2; 37])]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("pieces");
        put(&mods, "romfs/b.bin", &[3; 50]);
        put(&mods, "romfs/c.bin", &[4; 20]);
        let layered = Layered::new(&romfs, &image, &mods).unwrap().unwrap();
        let all = whole(&layered, &image);
        for start in 0..all.len() {
            for len in [1, 7, 16, 64] {
                let mut out = vec![0xAA; len];
                let read = layered.read(&image, start as u64, &mut out);
                assert_eq!(&out[..read], &all[start..(start + len).min(all.len())]);
            }
        }
        std::fs::remove_dir_all(mods).unwrap();
    }

    /// tones: a file with alternates reads as the one ALTERNATE picks, its
    /// size and place in the image the same for each.
    #[test]
    fn alternates_are_picked_without_changing_the_tables() {
        let image = game(&[("m/a.bin", b"game a"), ("m/b.bin", b"game b")]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("alternates");
        put(&mods, "romfs/m/a.bin", b"style one");
        put(&mods, "romfs/m/b.bin", b"only base");
        put(&mods, "alt/1/romfs/m/a.bin", b"style two");
        put(&mods, "alt/2/romfs/m/a.bin", b"style 333");
        put(&mods, "alt/2/romfs/m/b.bin", b"too long for b"); // another size: left out
        let layered = Layered::new(&romfs, &image, &mods).unwrap().unwrap();
        let mut seen = Vec::new();
        for pick in [0, 1, 2, 7] {
            ALTERNATE.store(pick, std::sync::atomic::Ordering::Relaxed);
            let all = whole(&layered, &image);
            seen.push((file(&all, "m/a.bin").unwrap(), file(&all, "m/b.bin").unwrap(), all[..layered.meta.len()].to_vec()));
        }
        ALTERNATE.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(seen[0].0, b"style one");
        assert_eq!(seen[1].0, b"style two");
        assert_eq!(seen[2].0, b"style 333");
        assert_eq!(seen[3].0, b"style one", "past the last: the base");
        assert!(seen.iter().all(|(_, b, _)| b == b"only base"));
        assert!(seen.iter().all(|(_, _, meta)| *meta == seen[0].2), "the tables never change");
        std::fs::remove_dir_all(mods).unwrap();
    }

    #[test]
    fn names_match_without_case_and_keep_the_games() {
        let image = game(&[("Data/Model.bin", b"old")]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("case");
        put(&mods, "romfs/data/model.bin", b"new");
        let layered = Layered::new(&romfs, &image, &mods).unwrap().unwrap();
        assert_eq!(layered.changes, Changes { replaced: 1, ..Changes::default() });
        let layered_image = whole(&layered, &image);
        let romfs = RomFs::parse_level3(&layered_image, 0).unwrap();
        let root = romfs.root().unwrap();
        assert_eq!(romfs.subdirs(&root)[0].1.name, "Data");
        assert_eq!(file(&layered_image, "Data/Model.bin").unwrap(), b"new");
        std::fs::remove_dir_all(mods).unwrap();
    }

    /// images that keep everything in a nameless directory under the root
    /// take a mod's paths from there.
    #[test]
    fn a_nameless_content_directory_takes_the_mod() {
        let image = romfs_build::build(&[BuildFile { path: "US/country.bin".into(), data: vec![1] }]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("nameless");
        put(&mods, "romfs/US/country.bin", &[2, 2]);
        let layered = Layered::new(&romfs, &image, &mods).unwrap().unwrap();
        assert_eq!(layered.changes, Changes { replaced: 1, ..Changes::default() });
        assert_eq!(file(&whole(&layered, &image), "US/country.bin").unwrap(), [2, 2]);
        assert_eq!(layered.modded_files(|_| true), [("US/country.bin".to_owned(), vec![2, 2])]);
        std::fs::remove_dir_all(mods).unwrap();
    }

    /// the files a mod replaced, patched or added read back by their paths,
    /// in the game's spelling, and the game's own and those the mod took
    /// away do not.
    #[test]
    fn the_files_a_mod_put_in_read_back_by_path() {
        let image = game(&[
            ("cro/Battle.cro", b"old battle"),
            ("cro/Field.cro", b"field"),
            ("cro/Gone.cro", b"gone"),
            ("static.crs", b"crs"),
            ("Data/model.bin", b"model"),
        ]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("modded");
        put(&mods, "romfs/cro/battle.cro", b"new battle");
        put(&mods, "romfs/cro/Extra.cro", b"extra");
        put(&mods, "romfs/Data/model.bin", b"new model");
        let mut ips = b"PATCH".to_vec();
        ips.extend_from_slice(&[0, 0, 0, 0, 1, b'C']);
        ips.extend_from_slice(b"EOF");
        put(&mods, "romfs_ext/static.crs.ips", &ips);
        put(&mods, "romfs_ext/cro/Gone.cro.stub", b"");

        let layered = Layered::new(&romfs, &image, &mods).unwrap().unwrap();
        let mut modules = layered.modded_files(|path| path.ends_with(".cro") || path == "static.crs");
        modules.sort();
        let expected = [("cro/Battle.cro", &b"new battle"[..]), ("cro/Extra.cro", b"extra"), ("static.crs", b"Crs")];
        assert_eq!(modules, expected.map(|(path, bytes)| (path.to_owned(), bytes.to_vec())));
        assert_eq!(layered.modded_files(|_| true).len(), 4);
        std::fs::remove_dir_all(mods).unwrap();
    }

    /// mods: several mods laid in order, the later one's file winning, and
    /// a patch in a later mod going over an earlier mod's file.
    #[test]
    fn later_mods_win_over_earlier_ones() {
        let image = game(&[("Data/a.bin", b"game a"), ("Data/b.bin", b"game b"), ("c.bin", b"cat")]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let first = folder("first");
        let second = folder("second");
        put(&first, "romfs/Data/a.bin", b"first a");
        put(&first, "romfs/Data/b.bin", b"first b");
        put(&second, "romfs/data/A.bin", b"second a");
        let mut ips = b"PATCH".to_vec();
        ips.extend_from_slice(&[0, 0, 0, 0, 1, b'B']);
        ips.extend_from_slice(b"EOF");
        put(&second, "romfs_ext/Data/b.bin.ips", &ips);
        put(&second, "romfs_ext/c.bin.stub", b"");
        let layered = Layered::with_mods(&romfs, &image, &[&first, &second]).unwrap().unwrap();
        let all = whole(&layered, &image);
        assert_eq!(file(&all, "Data/a.bin").unwrap(), b"second a");
        assert_eq!(file(&all, "Data/b.bin").unwrap(), b"Birst b");
        assert_eq!(file(&all, "c.bin"), None);
        // the other way round, the first mod's file wins
        let layered = Layered::with_mods(&romfs, &image, &[&second, &first]).unwrap().unwrap();
        assert_eq!(file(&whole(&layered, &image), "Data/a.bin").unwrap(), b"first a");
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }

    /// mods: a game installed as a folder, listed, with mods over it.
    #[test]
    fn mods_go_over_a_listed_folder() {
        let base = folder("listed-base");
        put(&base, "Data/a.bin", b"game a");
        put(&base, "Data/b.bin", b"game b");
        put(&base, "top.bin", b"top");
        let list = vec![("Data/a.bin".to_owned(), 6), ("Data/b.bin".to_owned(), 6), ("top.bin".to_owned(), 3)];
        let first = folder("listed-first");
        put(&first, "romfs/data/a.bin", b"modded a, longer");
        put(&first, "romfs/New/n.bin", b"new");
        let mut ips = b"PATCH".to_vec();
        ips.extend_from_slice(&[0, 0, 0, 0, 1, b'T']);
        ips.extend_from_slice(b"EOF");
        put(&first, "romfs_ext/top.bin.ips", &ips);
        let layered = Layered::from_list_with_mods(&base, &list, &[&first]).unwrap();
        assert_eq!(layered.changes, Changes { replaced: 1, added: 1, patched: 1, removed: 0 });
        let all = whole(&layered, &[]);
        assert_eq!(file(&all, "Data/a.bin").unwrap(), b"modded a, longer");
        assert_eq!(file(&all, "Data/b.bin").unwrap(), b"game b");
        assert_eq!(file(&all, "New/n.bin").unwrap(), b"new");
        assert_eq!(file(&all, "top.bin").unwrap(), b"Top");
        // with no mods it reads as from_list does
        let plain = Layered::from_list_with_mods(&base, &list, &[]).unwrap();
        assert_eq!(whole(&plain, &[]), whole(&Layered::from_list(&base, &list), &[]));
        for dir in [base, first] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// mods: how long building the image and reading every file of an
    /// installed game takes, plain and with a mod over it. Run with
    /// FATES_BENCH_ROMFS=<data/romfs> and FATES_BENCH_MOD=<a mod's folder>:
    /// cargo test -p zakuro-fs --release romfs_read_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn romfs_read_cost() {
        let Some(romfs) = std::env::var_os("FATES_BENCH_ROMFS").map(PathBuf::from) else { return };
        let mods: Vec<PathBuf> = std::env::var_os("FATES_BENCH_MOD").map(PathBuf::from).into_iter().collect();
        let list = crate::datadir::folder_list(&romfs).unwrap();
        let read_all = |layered: &Layered| {
            let image = RomFs::parse_level3(&layered.meta, 0).unwrap();
            let mut files = Vec::new();
            let mut pending = vec![image.root().unwrap()];
            while let Some(dir) = pending.pop() {
                for (_, file) in image.files(&dir) {
                    files.push((image.file_data_offset(&file), file.data_size));
                }
                pending.extend(image.subdirs(&dir).into_iter().map(|(_, child)| child));
            }
            let started = std::time::Instant::now();
            let mut buffer = vec![0u8; 0x10000];
            let mut total = 0u64;
            for (at, size) in &files {
                let mut done = 0;
                while done < *size {
                    let chunk = (*size - done).min(buffer.len() as u64) as usize;
                    total += layered.read(&[], at + done, &mut buffer[..chunk]) as u64;
                    done += chunk as u64;
                }
            }
            (files.len(), total, started.elapsed())
        };
        for round in 0..3 {
            let started = std::time::Instant::now();
            let plain = Layered::from_list(&romfs, &list);
            let built = started.elapsed();
            let (count, bytes, took) = read_all(&plain);
            println!("round {round} plain: built in {built:?}, {count} files, {} MiB read in {took:?}", bytes >> 20);
            let refs: Vec<&Path> = mods.iter().map(PathBuf::as_path).collect();
            let started = std::time::Instant::now();
            let modded = Layered::from_list_with_mods(&romfs, &list, &refs).unwrap();
            let built = started.elapsed();
            let (count, bytes, took) = read_all(&modded);
            println!("round {round} with {} mods ({:?}): built in {built:?}, {count} files, {} MiB read in {took:?}", refs.len(), modded.changes, bytes >> 20);
        }
    }

    #[test]
    fn an_empty_mod_changes_nothing() {
        let image = game(&[("a.bin", b"a")]);
        let romfs = RomFs::parse_level3(&image, 0).unwrap();
        let mods = folder("empty");
        std::fs::create_dir_all(mods.join("romfs")).unwrap();
        assert!(Layered::new(&romfs, &image, &mods).unwrap().is_none());
        std::fs::remove_dir_all(mods).unwrap();
    }
}

/// tones: the files under dir, / separated, with their size.
fn walk_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, &format!("{prefix}{name}/"), out);
        } else if let Ok(meta) = entry.metadata() {
            out.push((format!("{prefix}{name}"), path, meta.len()));
        }
    }
}
