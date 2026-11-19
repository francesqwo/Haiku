//! fanpatch: mods made for another release of the game, and Haiku's own text
//! kept under any mod's. See dev/reports/fanpatch.md.
//!
//! **Fan translations of the Japanese game.** Team If's *Fire Emblem if*
//! translation (v4.3, a zip of `exe/ rom/ dlc/` and IfPatcher.exe) and
//! cris1010's "Uncensored Patch" (v1.8, Team If's files and more, a RAR
//! whose `rom` folder works as a LayeredFS mod) are made for the Japanese
//! game (000400000012DE00, DC00, DD00). Their message archives are in the
//! Japanese layout (`m/X.bin.lz`, `m/A/X.bin.lz`). IfPatcher puts them on a
//! USA game as `m/@E/X` (`m/@U/X` on an EUR one); Haiku does the same when it
//! reads the mod ([`inspect`]): the files go into a folder of the zips'
//! cache in the layout of the build running, which is what the mod system
//! lays over the game. Recognised: Team If's `rom/` (also inside the v1.8
//! `TeamIFPatcher` folder, or a `rom` folder put in as the mod itself), a
//! Japanese title id folder (`000400000012DE00/romfs`, `luma/titles/...`,
//! `load/mods/...`), a mod.toml `title`, and a `romfs/` whose `m/` holds the
//! Japanese layout.
//!
//! *Why convert when the mod is read rather than map the paths at load:*
//! the file layer (zakuro-fs `Layered`) is shared by the game, the DLC, the
//! voices and every mod, and takes plain content folders; a copy (hard
//! links where it can) in the cache leaves it as it is. The region is
//! known by then (`zakuro_core::region::set` runs before the first scan),
//! and is part of the folder's name, so one Haiku folder used by a USA and
//! an EUR build has both.
//!
//! **The region check for code.** A mod's `code.ips`/`code.bps`/`code.bin`/
//! `exheader.bin` is recompiled against this build's code. One made for
//! another title (by its title id folder, its exheader.bin's program id,
//! its mod.toml `title`, or a Japanese layout) is left out with a note; its
//! files are still used.
//!
//! **Haiku's own text under a mod's** ([`lay_text`]). Haiku's builtin layer
//! (`assets/builtin/romfs/m/@E`) adds lines to the game's message archives
//! (Kana's gender, the same-gender S-supports, End Turn, the Sunglasses). A
//! mod's archive used to replace Haiku's whole, so those features lost
//! their text while it was on. At each start, for every message archive a
//! mod gives: Haiku's edits (what its archive changes in the game's,
//! `zakuro_fs::msgarc::Edits`) are laid on the mod's, and every label the
//! game's archive has that the mod's lacks gets the game's line (so the US
//! code never looks one up in vain). The results go in one more layer, on
//! top, cached by what they were made from. With no mod giving a message
//! archive nothing is done: the plain game is as it was.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::haiku_mods::{ModInfo, ModOption, Note};

/// the Japanese Fire Emblem if: Special Edition, Birthright (Byakuya),
/// Conquest (Anya).
pub const JP_SE: u64 = 0x0004_0000_0012_DE00;
pub const JP_BIRTHRIGHT: u64 = 0x0004_0000_0012_DC00;
pub const JP_CONQUEST: u64 = 0x0004_0000_0012_DD00;

/// what Haiku makes of the folders, bumped when that changes.
const CONVERT_VERSION: u32 = 1;
const TEXT_VERSION: u32 = 3; // tones: 3, the speaking styles' alternates
/// the file that marks a folder Haiku laid out from a Japanese-layout mod:
/// its message archives are pruned to the game's labels (lay_text).
const JAPANESE_MARKER: &str = ".haiku-japanese-layout";

/// a title, as the notes name it.
pub fn title_name(title: u64) -> String {
    match title {
        JP_SE | JP_BIRTHRIGHT | JP_CONQUEST => "the Japanese game (Fire Emblem if)".to_owned(),
        zakuro_core::region::USA => "the USA Special Edition".to_owned(),
        zakuro_core::region::EUR => "the EUR Special Edition".to_owned(),
        other => format!("another game ({other:016X})"),
    }
}

pub fn is_japanese(title: u64) -> bool {
    matches!(title, JP_SE | JP_BIRTHRIGHT | JP_CONQUEST)
}

/// a title id folder's name: 16 hex digits of an application (00040000...).
fn title_id(name: &str) -> Option<u64> {
    if name.len() != 16 || !name.to_ascii_lowercase().starts_with("00040000") {
        return None;
    }
    u64::from_str_radix(name, 16).ok()
}

/// a mod.toml `title`: a title id, or usa / eur / jpn.
pub fn parse_title(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "usa" | "us" | "na" => Some(zakuro_core::region::USA),
        "eur" | "eu" | "pal" => Some(zakuro_core::region::EUR),
        "jpn" | "jp" | "japan" | "if" => Some(JP_SE),
        _ => u64::from_str_radix(text.trim_start_matches("0x"), 16).ok(),
    }
}

/// what Haiku made of a mod for another release, kept on its ModInfo.
#[derive(Debug, Clone, Default)]
pub struct FanPatch {
    /// the title it was made for, when known, and how that was found
    pub title: Option<u64>,
    pub how: String,
    /// its message archives were laid out for this build (Japanese layout)
    pub converted: bool,
    /// Team If's layout (rom/, exe/, dlc/)
    pub team_if: bool,
    /// its code files left out, by name
    pub code_left_out: Vec<String>,
    /// folders of its files that are only used when its options say so:
    /// (option id, content folder)
    pub extras: Vec<(String, PathBuf)>,
    /// what of it was not needed here (exe/, dlc/ copies, IfPatcher.exe)
    pub dropped: Vec<String>,
    /// how many message archives it has
    pub messages: usize,
}

impl FanPatch {
    /// made for the Japanese game.
    pub fn japanese(&self) -> bool {
        self.title.is_some_and(is_japanese)
    }
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    std::fs::read_dir(dir).ok()?.flatten().find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|entry| entry.path())
}

/// the files under dir, / separated, in their own case, with their size and
/// time.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            if prefix.matches('/').count() < 16 {
                walk(&path, &format!("{prefix}{name}/"), out);
            }
        } else {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// whether dir is the root of a RomFS whose text is in the Japanese layout:
/// m/ holds .bin.lz files itself (the USA and EUR games have only language
/// folders, @E, @U..., and route folders there).
fn japanese_romfs(dir: &Path) -> bool {
    let Some(m) = find_ci(dir, "m") else { return false };
    let Ok(entries) = std::fs::read_dir(&m) else { return false };
    entries.flatten().any(|entry| entry.path().is_file() && entry.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".bin.lz"))
}

/// whether haiku_mods stops looking into a folder that holds one folder:
/// a Japanese-layout RomFS root (Team If's rom/ put in as the mod), or a
/// folder holding a title id folder.
pub fn stops_unwrap(dir: &Path) -> bool {
    if japanese_romfs(dir) {
        return true;
    }
    std::fs::read_dir(dir).map(|entries| entries.flatten().any(|entry| entry.path().is_dir() && title_id(&entry.file_name().to_string_lossy()).is_some())).unwrap_or(false)
}

/// Team If's `rom` folder: in the mod's folder, a folder below it (v1.8's
/// TeamIFPatcher), or the mod's folder itself (a `rom` folder put in as it
/// is).
fn rom_folder(folder: &Path) -> Option<PathBuf> {
    let looks = |dir: &Path| japanese_romfs(dir);
    if let Some(rom) = find_ci(folder, "rom").filter(|rom| looks(rom)) {
        return Some(rom);
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            if let Some(rom) = find_ci(&dir, "rom").filter(|rom| looks(rom)) {
                return Some(rom);
            }
        }
    }
    None
}

/// a title id folder in a mod: luma/titles/<id>, titles/<id>,
/// load/mods/<id> or <id>, any title but the USA Special Edition's (which
/// haiku_mods finds itself).
pub fn title_folder(folder: &Path) -> Option<(PathBuf, u64)> {
    let mut places = vec![folder.to_path_buf()];
    places.extend(find_ci(folder, "luma").and_then(|luma| find_ci(&luma, "titles")));
    places.extend(find_ci(folder, "titles"));
    places.extend(find_ci(folder, "load").and_then(|load| find_ci(&load, "mods")));
    for place in places {
        let Ok(entries) = std::fs::read_dir(&place) else { continue };
        let mut found: Vec<(PathBuf, u64)> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| Some((entry.path(), title_id(&entry.file_name().to_string_lossy())?)))
            .filter(|(_, id)| *id != 0x0004_0000_0017_9800)
            .collect();
        found.sort();
        if let Some(first) = found.into_iter().next() {
            return Some(first);
        }
    }
    None
}

/// the program id an exheader.bin names (its ACI, at 0x200).
fn exheader_title(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let id = bytes.get(0x200..0x208)?;
    Some(u64::from_le_bytes(id.try_into().ok()?))
}

/// the language folder of the build running: m/@E on the USA game, m/@U
/// (EUR English) on the EUR one, as IfPatcher lays them.
pub fn language_folder() -> &'static str {
    // jpprep: the Japanese build reads m/X itself
    if zakuro_core::region::is_jpn() {
        return "";
    }
    if zakuro_core::region::is_eur() {
        "@U"
    } else {
        "@E"
    }
}

/// where a file of a Japanese-layout RomFS goes in this build's: m/X to
/// m/@E/X, m/A/X to m/A/@E/X (B, C too); everything else stays.
pub fn us_path(path: &str, lang: &str) -> String {
    // jpprep: on the Japanese build every file stays where it is
    if lang.is_empty() {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').collect();
    let is_m = parts.first().is_some_and(|first| first.eq_ignore_ascii_case("m"));
    if !is_m {
        return path.to_owned();
    }
    match parts.as_slice() {
        [m, file] => format!("{m}/{lang}/{file}"),
        [m, route, file] if route.len() == 1 && "abcABC".contains(*route) => format!("{m}/{route}/{lang}/{file}"),
        _ => path.to_owned(),
    }
}

/// which option a file of the mod is under (none: always used).
fn extra_of(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("font/") {
        return Some("fonts");
    }
    if (lower.starts_with("effect/") && lower.rsplit('/').next().is_some_and(|name| name.starts_with("tlp_title"))) || lower == "ui/titlelogo.bch.lz" {
        return Some("title");
    }
    None
}

/// the options a fan patch's files are under, with their defaults
/// (fanpatch.md: the Japanese fonts are left out by default; the title
/// logo is used).
pub fn default_option(id: &str) -> i64 {
    match id {
        // jpprep: on the Japanese build its fonts are the game's own kind
        "fonts" => i64::from(zakuro_core::region::is_jpn()),
        _ => 1,
    }
}

fn option_label(id: &str) -> &'static str {
    match id {
        "fonts" => "Use its fonts",
        _ => "Use its title logo",
    }
}

/// FNV-1a.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn add(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
        self.0 ^= 0xFF;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
    }

    fn add_stamp(&mut self, path: &Path) {
        let meta = std::fs::metadata(path).ok();
        let size = meta.as_ref().map_or(0, |meta| meta.len());
        let time = meta.and_then(|meta| meta.modified().ok()).and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |time| time.as_nanos() as u64);
        self.add(&size.to_le_bytes());
        self.add(&time.to_le_bytes());
    }
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// puts from at to, a hard link when it can be, else a copy.
fn place(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// reads what a mod is for, and, for a Japanese-layout one, makes the
/// folder of its files in this build's layout. `content` is the content
/// folder haiku_mods found (the folder, or, `titled`, its USA or EUR title
/// folder), `title` what haiku_mods knows of its title (that folder's, or
/// its mod.toml's). None: an ordinary mod of this build, as before. The
/// title found goes into ModInfo::title, which `haiku_mods::
/// code_region_problem` (the one region check) reads.
pub fn inspect(id: &str, folder: &Path, content: &Path, titled: bool, title: Option<&str>, cache: Option<&Path>) -> Option<(FanPatch, PathBuf)> {
    let mut fan = FanPatch::default();
    let mut content = content.to_path_buf();
    let mut japanese_root: Option<PathBuf> = None;
    // the title: mod.toml, a title folder (when haiku_mods found none of
    // the USA's), Team If's rom/, exheader.bin, a Japanese layout
    if let Some(title) = title.and_then(parse_title) {
        fan.title = Some(title);
        fan.how = if titled { format!("its {title:016X} folder") } else { "its mod.toml".to_owned() };
    }
    if !titled {
        if let Some((dir, title)) = title_folder(folder) {
            content = dir;
            if fan.title.is_none() {
                fan.title = Some(title);
                fan.how = format!("its {title:016X} folder");
            }
        }
    }
    if let Some(rom) = rom_folder(&content) {
        fan.team_if = true;
        japanese_root = Some(rom);
    } else if let Some(romfs) = find_ci(&content, "romfs").filter(|romfs| japanese_romfs(romfs)) {
        japanese_root = Some(romfs);
    } else if japanese_romfs(&content) && find_ci(&content, "romfs").is_none() {
        // the rom folder itself, or a RomFS root, put in as the mod
        fan.team_if = folder.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("rom"));
        japanese_root = Some(content.clone());
    }
    let exheader = find_ci(&content.join("exefs"), "exheader.bin").or_else(|| find_ci(&content, "exheader.bin")).filter(|path| path.is_file());
    if fan.title.is_none() {
        if let Some(title) = exheader.as_deref().and_then(exheader_title).filter(|title| title >> 32 == 0x0004_0000) {
            fan.title = Some(title);
            fan.how = "its exheader.bin".to_owned();
        }
    }
    if fan.title.is_none() && japanese_root.is_some() {
        fan.title = Some(JP_SE);
        fan.how = if fan.team_if { "Team If's layout".to_owned() } else { "its Japanese file layout".to_owned() };
    }
    // jpprep: on the Japanese build a Japanese mod is native: its files
    // keep their paths (no m/@E, no pruning), a romfs/ is used as it is,
    // and its code goes to the one region check like any code mod's
    let native = zakuro_core::region::is_jpn() && fan.title.is_some_and(is_japanese);
    let romfs_root = japanese_root.as_deref().and_then(Path::file_name).is_some_and(|name| name.eq_ignore_ascii_case("romfs"));
    // a Japanese layout on a Japanese title: converted; on a title said to
    // be another (a mod.toml), left as it is
    let convert = japanese_root.is_some() && fan.title.is_some_and(is_japanese) && !(native && romfs_root);
    if fan.title.is_none() {
        return None;
    }
    // nothing to do for a mod of this build
    if fan.title == Some(zakuro_core::region::title()) && !convert && !native {
        return None;
    }
    // the Japanese game's code is left out (finish), its files kept: the
    // code files, by name, for the note
    if fan.title.is_some_and(is_japanese) && !(native && fan.title == Some(zakuro_core::region::title())) {
        let mut names: Vec<String> = zakuro_core::mods::code_files(&content).iter().filter_map(|path| Some(path.file_name()?.to_string_lossy().into_owned())).collect();
        for dir in [folder, content.as_path()] {
            for name in ["code.ips", "code.bps", "code.bin", "exheader.bin"] {
                if let Some(path) = find_ci(dir, name).or_else(|| find_ci(&dir.join("exefs"), name)).filter(|path| path.is_file()) {
                    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                        names.push(name);
                    }
                }
            }
        }
        fan.code_left_out = names;
    }
    if !convert {
        return Some((fan, content));
    }
    let root = japanese_root.unwrap_or_else(|| content.clone());
    // what Team If's zip has besides rom/: not needed here
    let base = root.parent().map(Path::to_path_buf).unwrap_or_default();
    if fan.team_if {
        if find_ci(&base, "exe").is_some() {
            fan.dropped.push("exe/ (the home-menu banner)".to_owned());
        }
        if find_ci(&base, "dlc").is_some() && !native {
            fan.dropped.push("dlc/ (the Japanese DLC's copy of the route text, which this build has in the game)".to_owned());
        }
        if find_ci(&base, "IfPatcher.exe").is_some() || find_ci(folder, "IfPatcher.exe").is_some() {
            fan.dropped.push("IfPatcher.exe (never run: Haiku reads the files itself)".to_owned());
        }
    }
    let lang = language_folder();
    let mut files = Vec::new();
    walk(&root, "", &mut files);
    // jpprep: on the Japanese build Team If's dlc/<index>.<id>/ text is
    // the Japanese DLC's (its routes): laid over that content as
    // dlc/<index>/romfs/local/ (zakuro_core::mods::lay_dlc)
    let mut dlc_files = Vec::new();
    if native && fan.team_if {
        if let Some(dlc) = find_ci(&base, "dlc") {
            for entry in std::fs::read_dir(&dlc).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let index = name.split('.').next().unwrap_or("");
                if index.len() == 4 && u16::from_str_radix(index, 16).is_ok() && entry.path().is_dir() {
                    let mut inner = Vec::new();
                    walk(&entry.path(), "", &mut inner);
                    dlc_files.extend(inner.into_iter().map(|(path, from)| (format!("dlc/{}/romfs/local/{path}", index.to_ascii_uppercase()), from)));
                }
            }
        }
    }
    let mut stamp = Fnv::new();
    stamp.add(&CONVERT_VERSION.to_le_bytes());
    stamp.add(lang.as_bytes());
    for (path, from) in files.iter().chain(&dlc_files) {
        stamp.add(path.as_bytes());
        stamp.add_stamp(from);
    }
    let cache = cache.map(Path::to_path_buf).unwrap_or_else(|| std::env::temp_dir().join("haiku-mods")).join("fanpatch");
    let name = safe(id);
    let out = cache.join(format!("{name}-{:016x}", stamp.0));
    let done = out.join(".done");
    let extras_of = |out: &Path| -> Vec<(String, PathBuf)> {
        ["fonts", "title"].iter().map(|id| (id.to_string(), out.join(id))).filter(|(_, dir)| dir.join("romfs").is_dir()).collect()
    };
    fan.converted = true;
    fan.messages = files.iter().filter(|(path, _)| path.to_ascii_lowercase().starts_with("m/") && path.to_ascii_lowercase().ends_with(".bin.lz")).count();
    if done.is_file() {
        fan.extras = extras_of(&out);
        return Some((fan, out));
    }
    let started = Instant::now();
    let _ = std::fs::remove_dir_all(&out);
    let mut placed = 0;
    for (path, from) in &files {
        let lower = path.to_ascii_lowercase();
        // code and the patcher's own files are not game files
        if !lower.contains('/') && (lower.ends_with(".ips") || lower.ends_with(".bps") || lower.ends_with(".exe") || lower.ends_with(".bin") || lower.ends_with(".txt")) {
            continue;
        }
        let target = us_path(path, lang);
        let into = match extra_of(path) {
            Some(option) => out.join(option).join("romfs"),
            None => out.join("romfs"),
        };
        if let Err(error) = place(from, &into.join(&target)) {
            log::warn!("fanpatch: could not lay out {path} of {id}, {error}");
            return Some((FanPatch { converted: false, ..fan }, content));
        }
        placed += 1;
    }
    // jpprep: the Japanese build's DLC text, and no pruning on its build
    for (path, from) in &dlc_files {
        if place(from, &out.join(path)).is_ok() {
            placed += 1;
        }
    }
    if !native {
        let _ = std::fs::write(out.join(JAPANESE_MARKER), "made for the Japanese game; its message archives are pruned to the game's labels\n");
    }
    let _ = std::fs::write(&done, format!("{placed} files of {} in the {lang} layout\n", root.display()));
    log::info!("fanpatch: {id} is in the Japanese layout: {placed} files laid out for this build (m/{lang}) in {} ms, in {}", started.elapsed().as_millis(), out.display());
    // older layouts of the same mod go
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let other = entry.file_name().to_string_lossy().into_owned();
            if entry.path() != out && other.strip_prefix(&format!("{name}-")).is_some_and(|rest| rest.len() == 16 && u64::from_str_radix(rest, 16).is_ok()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    fan.extras = extras_of(&out);
    Some((fan, out))
}

/// what haiku_mods does with a mod for another release once it has read
/// it: its code left out (a foreign title), its options, its notes.
pub fn finish(info: &mut ModInfo) {
    let Some(fan) = info.fanpatch.clone() else { return };
    // the region check (haiku_mods::code_region_problem, eur2's, which
    // refuses a code mod for the other build when the code mods are
    // planned): a mod made for the Japanese game keeps its files (a
    // translation's text), only its code is left out, from the start
    if fan.title.is_some_and(is_japanese) && crate::haiku_mods::code_region_problem(info.title.as_deref()).is_some() {
        info.code.clear();
        info.code_files = None;
    }
    for (option, _) in &fan.extras {
        if !info.options.iter().any(|known| &known.id == option) {
            info.options.push(ModOption { id: option.clone(), label: option_label(option).to_owned(), default: default_option(option), min: None, max: None });
        }
    }
    // a text patch is for looks: its text, fonts, menus' pictures, title
    // logo and the name-entry keyboard
    if fan.converted && info.gameplay.is_none() {
        let looks = info.files.iter().all(|file| {
            let file = file.to_ascii_lowercase();
            ["m/", "font/", "ui/", "effect/"].iter().any(|top| file.starts_with(top)) || file == "gamedata/texteditor.bin.lz"
        });
        if looks {
            info.gameplay = Some(false);
        }
    }
    if info.description.is_empty() && fan.title.is_some_and(is_japanese) && !zakuro_core::region::is_jpn() {
        info.description = JAPANESE_NOTE.to_owned();
    }
    for note in notes(info, &BTreeMap::new()) {
        log::info!("fanpatch: {}: {}", info.name, note.text);
    }
    for dropped in &fan.dropped {
        log::info!("fanpatch: {}: not used: {dropped}", info.name);
    }
}

/// the content folders of a mod turned on: its own, then the extras its
/// options turn on (fanpatch).
pub fn content_dirs(info: &ModInfo, options: &BTreeMap<String, i64>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = info.content.iter().cloned().collect();
    if let Some(fan) = &info.fanpatch {
        for (option, dir) in &fan.extras {
            if options.get(option).copied().unwrap_or_else(|| default_option(option)) != 0 {
                dirs.push(dir.clone());
            }
        }
    }
    dirs
}

/// the Japanese game's note, the first thing the Mods screen says of a
/// mod made for it.
pub const JAPANESE_CODE: &str = "Its code patch is for the Japanese game (Fire Emblem if), so it's left out.";

/// the Japanese game's note, the first thing the Mods screen says of a
/// mod made for it.
pub const JAPANESE_NOTE: &str = "Made for the Japanese game (Fire Emblem if): the petting minigame and Japan-only DLC stay unavailable on this build.";

/// the Mods screen's notes on a mod for another release, the most
/// important first (the details box shows about four lines): its code left
/// out, the game it was made for, then what Haiku did with it. The whole
/// list is also in the log when the mod is read.
pub fn notes(info: &ModInfo, options: &BTreeMap<String, i64>) -> Vec<Note> {
    let Some(fan) = &info.fanpatch else { return Vec::new() };
    let mut notes = Vec::new();
    // jpprep: the Japanese build has the petting minigame and its DLC
    let japanese = fan.title.is_some_and(is_japanese) && !zakuro_core::region::is_jpn();
    // the region check's own words, with the files it leaves out
    if !fan.code_left_out.is_empty() {
        if let Some(problem) = crate::haiku_mods::code_region_problem(info.title.as_deref()) {
            let files = fan.code_left_out.join(", ");
            notes.push(Note { warning: true, text: format!("{problem} ({files})") });
        }
    }
    // the description says it when Haiku wrote it there (finish)
    if japanese && info.description != JAPANESE_NOTE {
        notes.push(Note { warning: false, text: JAPANESE_NOTE.to_owned() });
    }
    if fan.converted && fan.messages > 0 {
        notes.push(Note {
            warning: false,
            text: if zakuro_core::region::is_jpn() {
                // jpprep
                "Its files are used where the Japanese game has them, with Haiku's own lines added (Kana, same-gender S-supports, End Turn) and any line it lacks taken from the game.".to_owned()
            } else {
                format!("Its text is laid out for this build (m/{}), with Haiku's own lines added (Kana, same-gender S-supports, End Turn) and any line it lacks taken from the game.", language_folder())
            },
        });
        // the EUR game reads m/@U only in English (IfPatcher's EUR layout)
        if zakuro_core::region::is_eur() {
            notes.push(Note { warning: false, text: "It shows when the Game Language is English (Options > Interface).".to_owned() });
        }
    }
    let on = |option: &str| options.get(option).copied().unwrap_or_else(|| default_option(option)) != 0;
    let has = |option: &str| fan.extras.iter().any(|(id, _)| id == option);
    let mut parts = Vec::new();
    if has("title") {
        parts.push(if on("title") { "its title logo is used" } else { "its title logo is off" });
    }
    if has("fonts") {
        parts.push(if on("fonts") { "its Japanese fonts are on (the 3DS prompts may show)" } else { "its Japanese fonts are off: the game's have the letters it needs" });
    }
    if !parts.is_empty() {
        let mut text = parts.join("; ");
        text[..1].make_ascii_uppercase();
        notes.push(Note { warning: false, text: format!("{text} (mods.toml options: title, fonts).") });
    }
    if !fan.dropped.is_empty() {
        let names: Vec<&str> = fan.dropped.iter().map(|d| d.split(' ').next().unwrap_or(d)).collect();
        notes.push(Note { warning: false, text: format!("Not needed here, and never run: {}.", names.join(", ")) });
    }
    notes
}

/// the Mods page's row of Team If's zip and of the endings patch (its
/// Japanese code.ips left out), as the notes read, for FATES_UI_SNAPSHOT.
pub fn sample_rows() -> Vec<crate::fates_ui::mods::ModRow> {
    let team_if = ModInfo {
        name: "IfTranslationPatch_4-3".into(),
        files: vec!["m/@e/x002.bin.lz".into()],
        fanpatch: Some(FanPatch {
            title: Some(JP_SE),
            how: "Team If's layout".into(),
            converted: true,
            team_if: true,
            extras: vec![("fonts".into(), PathBuf::new()), ("title".into(), PathBuf::new())],
            dropped: vec!["exe/ (the home-menu banner)".into(), "dlc/ (the Japanese DLC's copy of the route text, which this build has in the game)".into(), "IfPatcher.exe (never run: Haiku reads the files itself)".into()],
            messages: 721,
            ..FanPatch::default()
        }),
        ..ModInfo::default()
    };
    let endings = ModInfo {
        name: "CharacterEndingsFixed".into(),
        files: vec!["m/@e/x001.bin.lz".into()],
        fanpatch: Some(FanPatch { title: Some(JP_SE), how: "its Japanese file layout".into(), converted: true, code_left_out: vec!["code.ips".into()], messages: 1, ..FanPatch::default() }),
        ..ModInfo::default()
    };
    let mut rows = Vec::new();
    for mut info in [team_if, endings] {
        finish(&mut info);
        rows.push(crate::fates_ui::mods::ModRow {
            name: info.name.clone(),
            description: info.description.clone(),
            enabled: true,
            can_enable: true,
            notes: notes(&info, &BTreeMap::new()),
            ..Default::default()
        });
    }
    rows
}

/// a RAR or 7z archive in the mods folder: a row that says to extract it
/// (none when it was extracted beside it already, or it's another file).
pub fn archive(path: &Path) -> Option<ModInfo> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let kind = match ext.as_str() {
        "rar" => "a RAR archive",
        "7z" => "a 7z archive",
        _ => return None,
    };
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let parent = path.parent()?;
    if parent.join(&stem).is_dir() {
        return None;
    }
    Some(ModInfo {
        id: stem.clone(),
        name: stem,
        source: path.to_path_buf(),
        problem: Some(format!(
            "It's {kind}, which Haiku can't open. Extract it with 7-Zip (if it asks for a password, the patch's own page gives it), then put the extracted folder in mods/."
        )),
        ..ModInfo::default()
    })
}

// -- Haiku's text under the mods' ---------------------------------------------

/// what lay_text did, for the log and the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextMerge {
    pub archives: usize,
    pub written: usize,
    pub filled: usize,
    pub added: usize,
    pub changed: usize,
    pub kept: usize,
    pub pruned: usize,
    /// lines with a letter the fonts lack, written with the nearest one
    pub letters: usize,
    /// tones: archives laid in the Japanese game's three speaking styles
    pub tones: usize,
}

/// the letters the game draws its own text with: those its fonts have (the
/// talk font, and the menus' System and System_L) and that its own text
/// uses. A font can hold a letter the game never shows whose picture isn't
/// that letter: the US talk font's ≪ and ≫ draw as "?" (seen in Team If's
/// dragon lines), so a letter counts only when the game's own lines use it
/// (or it is plain ASCII).
struct Fonts {
    talk: std::collections::HashSet<u32>,
    menus: std::collections::HashSet<u32>,
    used: std::collections::HashSet<u32>,
}

/// a bfnt's letters (glyph records of 16 bytes from 0x30, their count at
/// 0x22, each starting with its code point).
fn bfnt_letters(lz: &[u8]) -> std::collections::HashSet<u32> {
    let Ok(data) = zakuro_fs::lz11::decompress(lz) else { return Default::default() };
    let count = data.get(0x22..0x24).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    (0..count).filter_map(|i| data.get(0x30 + i * 16..0x34 + i * 16)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

impl Fonts {
    /// the fonts in use (the topmost layer's that has one, else the game's),
    /// and the letters the game's own lines use.
    fn of(mods: &[PathBuf], game_file: &dyn Fn(&str) -> Option<Vec<u8>>, used: std::collections::HashSet<u32>) -> Fonts {
        let font = |name: &str| -> std::collections::HashSet<u32> {
            let path = format!("font/{name}.bfnt.lz");
            let from_mod = mods.iter().rev().filter_map(|dir| find_ci(dir, "romfs")).find_map(|romfs| std::fs::read(romfs.join(&path)).ok());
            from_mod.or_else(|| game_file(&path)).map(|bytes| bfnt_letters(&bytes)).unwrap_or_default()
        };
        let mut menus = font("System");
        menus.extend(font("System_L"));
        Fonts { talk: font("Talk"), menus, used }
    }

    /// text with each letter the game can't show where it is (the talk font
    /// for a line with talk codes, the menus' otherwise) written as the
    /// nearest one it can; none when nothing changes or no font was read.
    fn fallback(&self, text: &[u16]) -> Option<Vec<u16>> {
        if self.talk.is_empty() || self.menus.is_empty() {
            return None;
        }
        let talk = text.windows(2).any(|w| w[0] == '$' as u16 && (w[1] == 'W' as u16 || w[1] == 't' as u16));
        let has = |c: u32| {
            let font = if talk { &self.talk } else { &self.menus };
            font.contains(&c) && ((0x20..0x7F).contains(&c) || self.used.contains(&c))
        };
        let mut out = Vec::with_capacity(text.len());
        let mut changed = false;
        // the table's letters never appear in a $ code (codes are ASCII,
        // and names in kana)
        for &unit in text {
            let c = unit as u32;
            if c < 0x7F || has(c) {
                out.push(unit);
                continue;
            }
            match nearest_letters(c).filter(|near| near.chars().all(|n| has(n as u32))) {
                Some(near) => {
                    out.extend(near.encode_utf16());
                    changed = true;
                }
                None => out.push(unit),
            }
        }
        changed.then_some(out)
    }
}

/// the nearest plain letters for one the game may not show.
fn nearest_letters(c: u32) -> Option<String> {
    // full-width ASCII (！ to ～) as the ASCII letter
    if (0xFF01..=0xFF5E).contains(&c) {
        return char::from_u32(c - 0xFEE0).map(String::from);
    }
    Some(
        match char::from_u32(c)? {
            'ō' | 'ó' | 'ò' | 'ô' | 'ö' => "o",
            'Ō' => "O",
            'ū' | 'ú' | 'ù' | 'û' | 'ü' => "u",
            'Ū' => "U",
            'ā' | 'á' | 'à' | 'â' | 'ä' => "a",
            'ē' | 'é' | 'è' | 'ê' | 'ë' => "e",
            'ī' | 'í' | 'ì' | 'î' | 'ï' => "i",
            'ç' => "c",
            'ß' => "ss",
            '÷' => "/",
            '≪' | '≫' | '“' | '”' | '«' | '»' => "\"",
            '―' | '–' => "—",
            '’' | '‘' => "'",
            '…' => "...",
            '★' | '☆' => "*",
            '・' => "-",
            '[' => "(",
            ']' => ")",
            _ => return None,
        }
        .to_owned(),
    )
}

/// a message archive in a layer: its path in the RomFS as the layer has it,
/// and the file.
fn message_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    if let Some(romfs) = find_ci(dir, "romfs") {
        if let Some(m) = find_ci(&romfs, "m") {
            let name = m.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "m".into());
            walk(&m, &format!("{name}/"), &mut files);
        }
    }
    files.retain(|(path, _)| path.to_ascii_lowercase().ends_with(".bin.lz"));
    files
}

/// the m/ files a layer changes through romfs_ext (patches and stubs).
fn patched_message_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Some(ext) = find_ci(dir, "romfs_ext") {
        if let Some(m) = find_ci(&ext, "m") {
            walk(&m, "m/", &mut files);
        }
    }
    files
        .into_iter()
        .filter_map(|(path, _)| {
            let lower = path.to_ascii_lowercase();
            ["ips", "bps", "stub"].iter().find_map(|kind| lower.strip_suffix(&format!(".{kind}")).map(str::to_owned))
        })
        .collect()
}

fn read_lz(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    zakuro_fs::lz11::decompress(&bytes)
}

/// where the merged archives are kept.
fn text_cache() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("haiku")).unwrap_or_else(|| crate::haiku_mods::root().join("cache")).join("text")
}

/// Haiku's own text under the mods' (see the module's notes): for the
/// layers in `mods` (the voices, Haiku's builtin layer, then the player's
/// mods, lowest first), the mods' message archives get Haiku's lines and
/// the game's missing ones, in one more layer on top. `game` is the game's
/// data, for the game's own archives. HAIKU_TEXT_MERGE=0 leaves it out
/// (for comparing); =nofill only lays Haiku's lines.
pub fn lay_text(mods: &mut Vec<PathBuf>, game: &Path) -> Option<TextMerge> {
    zakuro_core::tones::set_available(None); // tones: until a layer with speaking styles is laid
    crate::jpn_english::set_active(false); // jpn2: until an English translation is found on the Japanese game
    let mode = std::env::var("HAIKU_TEXT_MERGE").unwrap_or_default();
    if mode == "0" {
        return None;
    }
    lay_text_with(mods, game, &crate::haiku_builtin::content_dirs(), &text_cache(), mode != "nofill")
}

/// the game's own file at a RomFS path: from an installed folder's romfs/
/// as it is on disk, else through its RomFS.
fn game_files(game: &Path) -> Box<dyn Fn(&str) -> Option<Vec<u8>>> {
    let folder = game.join("romfs");
    if folder.is_dir() {
        return Box::new(move |path: &str| std::fs::read(folder.join(path)).ok());
    }
    let rom = zakuro_fs::Title::load(game).ok().and_then(|title| zakuro_core::randomizer::rom::Rom::of(&title));
    if rom.is_none() {
        log::warn!("fanpatch: the game's text can't be read from {}; the mods' text gets Haiku's lines only", game.display());
    }
    Box::new(move |path: &str| rom.as_ref()?.read(path))
}

/// lay_text, with Haiku's builtin layers and the cache given.
pub fn lay_text_with(mods: &mut Vec<PathBuf>, game: &Path, builtin: &[PathBuf], cache: &Path, fill: bool) -> Option<TextMerge> {
    // the topmost mod file of each message archive, and Haiku's own
    // (its path, the file, whether its mod was made for the Japanese game)
    let mut winner: HashMap<String, (String, PathBuf, bool)> = HashMap::new();
    let mut ours: HashMap<String, PathBuf> = HashMap::new();
    let mut patched: Vec<String> = Vec::new();
    for dir in mods.iter() {
        let is_builtin = builtin.iter().any(|b| b == dir);
        let japanese = dir.join(JAPANESE_MARKER).is_file();
        for (path, file) in message_files(dir) {
            let key = path.to_lowercase();
            if is_builtin {
                ours.insert(key.clone(), file);
                winner.remove(&key);
            } else {
                winner.insert(key, (path, file, japanese));
            }
        }
        if !is_builtin {
            for path in patched_message_files(dir) {
                winner.remove(&path);
                patched.push(path);
            }
        }
    }
    if winner.is_empty() {
        return None;
    }
    // jpn2: on the Japanese game, a mod whose script is English (Team If,
    // the Uncensored Patch): Haiku's added lines and menus in English too
    let english = zakuro_core::region::is_jpn() && english_script(&winner);
    crate::jpn_english::set_active(english);
    if english {
        log::info!("jpn2: the mods put the Japanese game's script in English: Haiku's lines and menus are English too");
    }
    let started = Instant::now();
    let mut keys: Vec<&String> = winner.keys().collect();
    // tones: the creator's text last, once the speaking styles are known
    keys.sort_by_key(|key| (is_creator_text(key), (*key).clone()));
    let mut stamp = Fnv::new();
    stamp.add(&TEXT_VERSION.to_le_bytes());
    stamp.add(&[fill as u8]);
    stamp.add(&[english as u8, JPN_ENGLISH_VERSION]); // jpn2
    stamp.add(std::env::var("HAIKU_TEXT_NOFILL").unwrap_or_default().as_bytes());
    stamp.add(game.to_string_lossy().as_bytes());
    stamp.add_stamp(game);
    for key in &keys {
        let (_, file, _) = &winner[*key];
        stamp.add(key.as_bytes());
        stamp.add(file.to_string_lossy().as_bytes());
        stamp.add_stamp(file);
        if let Some(builtin) = ours.get(*key) {
            stamp.add_stamp(builtin);
        }
    }
    let out = cache.join(format!("{:016x}", stamp.0));
    let done = out.join("merged.txt");
    if let Ok(summary) = std::fs::read_to_string(&done) {
        log::info!("fanpatch: Haiku's text over the mods' {} message files, as made before: {}", winner.len(), summary.trim());
        if out.join("romfs").is_dir() {
            tones_from(&out); // tones
            mods.push(out);
        }
        return None;
    }
    let game_file = game_files(game);
    let mut total = TextMerge { archives: winner.len(), ..TextMerge::default() };
    // the game's own archives, and the letters its lines use (for a
    // Japanese-layout mod's text, with the fonts in use)
    let mut game_archives: HashMap<&String, Option<Vec<zakuro_fs::msgarc::Message>>> = HashMap::new();
    let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for key in &keys {
        let (path, _, _) = &winner[*key];
        let messages = game_file(path).and_then(|bytes| zakuro_fs::lz11::decompress(&bytes).ok()).and_then(|raw| zakuro_fs::msgarc::parse(&raw).ok());
        for message in messages.iter().flatten() {
            used.extend(message.text.iter().map(|&unit| unit as u32));
        }
        game_archives.insert(*key, messages);
    }
    let fonts = winner.values().any(|(_, _, japanese)| *japanese).then(|| Fonts::of(mods, &*game_file, used));
    // jpn2: Haiku's lines in English, made from the translation's (by RomFS
    // path, lower case), and the Japanese lines they stand for
    let mut english_lines: HashMap<String, Vec<zakuro_fs::msgarc::Message>> = HashMap::new();
    let mut english_made = crate::jpn_english::Made::default();
    if english {
        let mut read_cache: HashMap<String, Option<std::sync::Arc<Vec<zakuro_fs::msgarc::Message>>>> = HashMap::new();
        let mut read = |path: &str| -> Option<std::sync::Arc<Vec<zakuro_fs::msgarc::Message>>> {
            let key = path.to_lowercase();
            read_cache
                .entry(key.clone())
                .or_insert_with(|| {
                    let raw = match winner.get(&key) {
                        Some((_, file, _)) => read_lz(file).ok(),
                        None => game_file(path).and_then(|bytes| zakuro_fs::lz11::decompress(&bytes).ok()),
                    };
                    raw.and_then(|raw| zakuro_fs::msgarc::parse(&raw).ok()).map(std::sync::Arc::new)
                })
                .clone()
        };
        let (lines, made) = crate::jpn_english::make(&mut read);
        english_made = made;
        for (path, messages) in lines {
            english_lines.entry(path.to_lowercase()).or_default().extend(messages);
        }
        // Haiku's own archives the mods don't have (the new support files):
        // made here too, so the English ones lie over the Japanese builtin
        for key in ours.keys() {
            english_lines.entry(key.clone()).or_default();
        }
    }
    let work = cache.join(format!("{:016x}.new", stamp.0));
    let _ = std::fs::remove_dir_all(&work);
    // tones: the styles' names, the largest growth of an archive in another
    // style, and the archives' total bytes in each style
    let mut tone_names = zakuro_core::tones::default_names();
    let mut tone_growth = 0i64;
    let mut tone_bytes = [0usize; 3];
    for key in keys {
        let (path, file, japanese) = &winner[key];
        let theirs = match read_lz(file) {
            Ok(raw) => raw,
            Err(error) => {
                log::warn!("fanpatch: {path} of a mod can't be read ({error}); left as it is");
                continue;
            }
        };
        let game_messages = game_archives.remove(key).flatten();
        let haiku = ours.get(key).and_then(|file| read_lz(file).ok()).and_then(|raw| zakuro_fs::msgarc::parse(&raw).ok());
        let edits = match &haiku {
            // jpn2: Haiku's English lines (and its Japanese ones where none
            // was made), and End Turn over the translation's End
            _ if english => english_edits(english_lines.remove(key).unwrap_or_default(), haiku.as_deref(), game_messages.as_deref(), &theirs),
            Some(haiku) => zakuro_fs::msgarc::Edits::between(game_messages.as_deref(), haiku),
            None => zakuro_fs::msgarc::Edits::default(),
        };
        // dev: HAIKU_TEXT_NOFILL=<archive names> leaves those unfilled (bisecting)
        let fill_this = fill && !std::env::var("HAIKU_TEXT_NOFILL").is_ok_and(|names| names.split(',').any(|name| !name.is_empty() && path.ends_with(name)));
        // a Japanese-layout mod's archives keep only the labels the game
        // (or Haiku) has: the Japanese game's three talk tones and its own
        // scenes made them too big for the US game's memory (fanpatch.md)
        match zakuro_fs::msgarc::merge(&theirs, game_messages.as_deref(), &edits, fill_this, *japanese) {
            Ok((merged, counts)) => {
                // a letter no font in use has (the translation's ō, say) is
                // written as the nearest one they have
                let (merged, letters) = match (&fonts, *japanese) {
                    (Some(fonts), true) => zakuro_fs::msgarc::map_texts(&merged, |text| fonts.fallback(text)).unwrap_or((merged, 0)),
                    _ => (merged, 0),
                };
                // tones: a Japanese-layout mod's archive rebuilt compactly:
                // the lines rewritten above (fill, letters) left their old
                // text in the data, unread (A010: 178 KB for 93 KB of
                // messages), and these archives are what the US game's
                // memory is short of (fanpatch.md)
                let merged = if *japanese { compact(merged) } else { merged };
                // tones: the archive in the Japanese game's other speaking
                // styles, the style's lines under the labels the game reads
                let alternates = if *japanese { tone_alternates(&merged, &theirs, fonts.as_ref()) } else { Vec::new() };
                // tones: the creator's Voice step names the styles its voices pick
                let creator = *japanese && total.tones > 0 && is_creator_text(key);
                let merged = if creator { name_voices(merged, &mut tone_names, &theirs) } else { merged };
                total.letters += letters;
                total.filled += counts.filled;
                total.added += counts.added;
                total.changed += counts.changed;
                total.kept += counts.kept;
                total.pruned += counts.pruned;
                if counts.any() || letters > 0 || !alternates.is_empty() || creator {
                    let to = work.join("romfs").join(path);
                    if let Some(parent) = to.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    // tones: every style the same size, so the RomFS tables
                    // hold for all three: the shorter ones padded inside
                    // their stream (zeros after the archive's data), as a
                    // long run of bytes after the stream broke the game's
                    // loading of it (A002, 1 KB of zeros: faults)
                    // jpn2: on the Japanese game the archive ends in zeros (jpn_tail)
                    let merged = if zakuro_core::region::is_jpn() { jpn_tail(merged) } else { merged };
                    let mut packed = vec![zakuro_fs::lz11::compress_fast(&merged)];
                    packed.extend(alternates.iter().map(|alt| alt.as_deref().map(zakuro_fs::lz11::compress_fast).unwrap_or_default()));
                    let size = packed.iter().map(Vec::len).max().unwrap_or(0);
                    let mut failed = false;
                    for (k, data) in packed.iter_mut().enumerate() {
                        if k > 0 && data.is_empty() {
                            continue;
                        }
                        if data.len() < size {
                            *data = zakuro_fs::lz11::pad_to(data, size).unwrap_or_else(|| {
                                let mut data = data.clone();
                                data.resize(size, 0);
                                data
                            });
                        }
                        let to = if k == 0 { to.clone() } else { work.join("alt").join(k.to_string()).join("romfs").join(path) };
                        if let Some(parent) = to.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(error) = std::fs::write(&to, &data) {
                            log::warn!("fanpatch: could not write {}, {error}", to.display());
                            failed |= k == 0;
                        }
                    }
                    if failed {
                        continue;
                    }
                    if alternates.iter().any(Option::is_some) {
                        total.tones += 1;
                        let base = merged.len() as i64;
                        let grown = alternates.iter().flatten().map(|alt| alt.len() as i64 - base).max().unwrap_or(0);
                        tone_growth = tone_growth.max(grown);
                        tone_bytes[0] += merged.len();
                        for (k, alt) in alternates.iter().enumerate() {
                            tone_bytes[k + 1] += alt.as_ref().map_or(merged.len(), Vec::len);
                        }
                    }
                    total.written += 1;
                    if trace() {
                        log::info!("fanpatch: {path}: Haiku's lines {} added, {} rewritten, {} kept; {} of the game's lines filled in; {} it has the game doesn't left out", counts.added, counts.changed, counts.kept, counts.filled, counts.pruned);
                    }
                }
            }
            Err(error) => log::warn!("fanpatch: {path} of a mod isn't a message archive Haiku knows ({error}); left as it is"),
        }
    }
    // jpn2: Haiku's archives the mods don't give (its new support files),
    // over the game's own or Haiku's Japanese one, with the English lines
    if english {
        let mut rest: Vec<(String, Vec<zakuro_fs::msgarc::Message>)> = english_lines.drain().collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, lines) in rest {
            if lines.is_empty() {
                continue; // Haiku's Japanese archive stays as it is
            }
            let builtin_raw = ours.get(&key).and_then(|file| read_lz(file).ok());
            let haiku = builtin_raw.as_deref().and_then(|raw| zakuro_fs::msgarc::parse(raw).ok());
            // the path as Haiku's layer spells it (RomFS names keep their case)
            let path = ours
                .get(&key)
                .and_then(|file| file.to_str().map(|f| f.replace('\\', "/")))
                .and_then(|file| file.rfind("/romfs/").map(|at| file[at + 7..].to_string()))
                .unwrap_or_else(|| key.clone());
            let game_raw = game_file(&path).and_then(|bytes| zakuro_fs::lz11::decompress(&bytes).ok());
            let game_messages = game_raw.as_deref().and_then(|raw| zakuro_fs::msgarc::parse(raw).ok());
            // the base: the game's archive, else an empty one named as Haiku's
            let base = match (&game_raw, &builtin_raw) {
                (Some(raw), _) => raw.clone(),
                (None, Some(raw)) => match zakuro_fs::msgarc::name(raw) {
                    Ok(name) => zakuro_fs::msgarc::new(&name, &[]),
                    Err(_) => continue,
                },
                (None, None) => continue,
            };
            let edits = english_edits(lines, haiku.as_deref(), game_messages.as_deref(), &base);
            if edits.is_empty() {
                continue;
            }
            match zakuro_fs::msgarc::merge(&base, game_messages.as_deref(), &edits, false, false) {
                Ok((merged, counts)) => {
                    let to = work.join("romfs").join(&path);
                    if let Some(parent) = to.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::write(&to, zakuro_fs::lz11::compress_fast(&jpn_tail(merged))).is_ok() {
                        total.written += 1;
                        total.added += counts.added;
                    }
                }
                Err(error) => log::warn!("jpn2: Haiku's English lines for {key} couldn't be laid ({error})"),
            }
        }
        log::info!(
            "jpn2: Haiku's lines in English: {} made from the translation ({} as their source reads, {} reviewed edits not met), {} without a source line",
            english_made.lines,
            english_made.same,
            english_made.edits_missed,
            english_made.no_source
        );
    }
    let summary = format!(
        "{} archives: {} rebuilt; Haiku's lines {} added, {} rewritten, {} left in the mod's own words; {} of the game's lines the mods lack filled in; {} lines the game never asks for left out; {} lines given letters the fonts have{}",
        total.archives,
        total.written,
        total.added,
        total.changed,
        total.kept,
        total.filled,
        total.pruned,
        total.letters,
        if patched.is_empty() { String::new() } else { format!("; {} patched by romfs_ext left as they are", patched.len()) }
    );
    let summary = if total.tones > 0 {
        format!(
            "{summary}; {} archives in the Japanese game's three speaking styles ({} / {} / {} bytes, an archive at most {tone_growth} bytes more than in the first)",
            total.tones, tone_bytes[0], tone_bytes[1], tone_bytes[2]
        )
    } else {
        summary
    };
    let _ = std::fs::create_dir_all(&work);
    let _ = std::fs::write(work.join("merged.txt"), format!("{summary}\n"));
    if total.tones > 0 {
        let _ = std::fs::write(work.join(TONES_FILE), format!("male={}\nfemale={}\narchives={}\n", tone_names[0].join("|"), tone_names[1].join("|"), total.tones));
    }
    let _ = std::fs::remove_dir_all(&out);
    if let Err(error) = std::fs::rename(&work, &out) {
        log::warn!("fanpatch: could not keep the merged text in {}, {error}", out.display());
        return Some(total);
    }
    // the merges of other sets of mods go
    if let Ok(entries) = std::fs::read_dir(cache) {
        for entry in entries.flatten() {
            if entry.path() != out {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    log::info!("fanpatch: Haiku's text over the mods' {summary}, in {} ms ({})", started.elapsed().as_millis(), out.display());
    if total.written > 0 {
        tones_from(&out); // tones
        mods.push(out);
    }
    Some(total)
}

// -- tones: Corrin's speaking styles ------------------------------------------

/// jpn2: the plan's version, in the merged text's stamp (3: jpn_tail)
const JPN_ENGLISH_VERSION: u8 = 3;

/// jpn2: an archive Haiku writes for the Japanese game ends in 32 zero
/// bytes (its size word counting them). The Japanese game read the last
/// bytes of such a file short: the archive's last label (the English
/// "Leave it to fate" of Kana's question, the last message Haiku adds to
/// X002) was never found, and the Sunglasses' name in the creator data
/// likewise (dev/region/jpn2/sunglasses_jpn.py). Zeros there cost nothing.
fn jpn_tail(mut raw: Vec<u8>) -> Vec<u8> {
    let len = raw.len();
    raw.resize(len + 32, 0);
    if raw.len() >= 4 && u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize == len {
        raw[..4].copy_from_slice(&(len as u32 + 32).to_le_bytes());
    }
    raw
}

/// jpn2: whether the mods' message archives are in English (on the Japanese
/// game): the topmost mod's archives, a few of them, read for their letters.
fn english_script(winner: &HashMap<String, (String, PathBuf, bool)>) -> bool {
    let mut keys: Vec<&String> = winner.keys().collect();
    keys.sort();
    let mut votes = (0, 0);
    for key in keys.iter().filter(|k| k.ends_with("gamedata.bin.lz") || k.ends_with("x002.bin.lz") || k.ends_with("residentmenu.bin.lz") || k.contains("/a0")).take(12) {
        let (_, file, _) = &winner[*key];
        if let Some(messages) = read_lz(file).ok().and_then(|raw| zakuro_fs::msgarc::parse(&raw).ok()) {
            match crate::jpn_english::is_english(&messages) {
                Some(true) => votes.0 += 1,
                Some(false) => votes.1 += 1,
                None => {}
            }
        }
    }
    votes.0 > votes.1
}

/// jpn2: the edits for one archive under an English translation on the
/// Japanese game: Haiku's English lines; its Japanese lines where no English
/// one was made (none is left missing); End Turn where the translation's
/// End reads "End" (as qol5 does on the USA game).
fn english_edits(lines: Vec<zakuro_fs::msgarc::Message>, haiku: Option<&[zakuro_fs::msgarc::Message]>, game: Option<&[zakuro_fs::msgarc::Message]>, theirs: &[u8]) -> zakuro_fs::msgarc::Edits {
    let mut edits = zakuro_fs::msgarc::Edits { added: lines, changed: Vec::new() };
    if let Some(haiku) = haiku {
        let japanese = zakuro_fs::msgarc::Edits::between(game, haiku);
        let made: std::collections::HashSet<Vec<u8>> = edits.added.iter().map(|m| m.label.clone()).collect();
        let end = crate::jpn_english::label_of(crate::jpn_english::END_LABEL);
        for message in japanese.added {
            if !made.contains(&message.label) {
                edits.added.push(message);
            }
        }
        for (message, _) in japanese.changed {
            if message.label == end {
                let theirs = zakuro_fs::msgarc::parse(theirs).unwrap_or_default();
                if let Some(text) = theirs.iter().find(|m| m.label == end).map(|m| m.text.clone()) {
                    if String::from_utf16_lossy(&text).trim() == "End" {
                        edits.changed.push((zakuro_fs::msgarc::Message { label: end.clone(), text: "End Turn".encode_utf16().collect() }, text));
                    }
                }
            }
        }
    }
    edits
}

/// the file a text layer with speaking styles keeps their names in.
const TONES_FILE: &str = "tones.txt";

/// the avatar creator's text (its voices' names).
fn is_creator_text(key: &str) -> bool {
    key.to_ascii_lowercase().ends_with("/myunitedit.bin.lz")
}

/// tells the game a text layer has speaking styles (its tones.txt).
fn tones_from(out: &Path) {
    // jpn: the Japanese game picks the talk tone itself (TalkUtil::GetTalkTone)
    if zakuro_core::region::is_jpn() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(out.join(TONES_FILE)) else { return };
    let mut names = zakuro_core::tones::default_names();
    for line in text.lines() {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        let gender = match key {
            "male" => 0,
            "female" => 1,
            _ => continue,
        };
        let parts: Vec<&str> = value.split('|').collect();
        if parts.len() == 3 && parts.iter().all(|part| !part.is_empty()) {
            names[gender] = [parts[0].to_owned(), parts[1].to_owned(), parts[2].to_owned()];
        }
    }
    log::info!("tones: the translation's text has the Japanese game's speaking styles: {} (male), {} (female)", names[0].join(", "), names[1].join(", "));
    zakuro_core::tones::set_available(Some(names));
}

/// an archive with only the texts its labels point at.
fn compact(raw: Vec<u8>) -> Vec<u8> {
    match (zakuro_fs::msgarc::name(&raw), zakuro_fs::msgarc::parse(&raw)) {
        (Ok(name), Ok(messages)) => {
            let rebuilt = zakuro_fs::msgarc::new(&name, &messages);
            if rebuilt.len() < raw.len() { rebuilt } else { raw }
        }
        _ => raw,
    }
}

/// a merged archive in speaking styles 2 and 3 (none where a style changes
/// no line), from the translation's own archive with all three, given the
/// same letters the first got. Empty when neither changes a line.
fn tone_alternates(merged: &[u8], theirs: &[u8], fonts: Option<&Fonts>) -> Vec<Option<Vec<u8>>> {
    let Ok(messages) = zakuro_fs::msgarc::parse(theirs) else { return Vec::new() };
    let alternates: Vec<Option<Vec<u8>>> = (2..=3u8)
        .map(|tone| {
            let alt = zakuro_fs::msgarc::tone_variant(merged, &messages, tone).ok().flatten()?;
            let alt = match fonts {
                Some(fonts) => compact(zakuro_fs::msgarc::map_texts(&alt, |text| fonts.fallback(text)).map(|(alt, _)| alt).unwrap_or(alt)),
                None => alt,
            };
            // the same lines once the letters are the fonts' (a "…" the
            // first style had as "..."): no alternate
            let same = zakuro_fs::msgarc::parse(&alt).ok() == zakuro_fs::msgarc::parse(merged).ok();
            (!same).then_some(alt)
        })
        .collect();
    if alternates.iter().all(Option::is_none) {
        return Vec::new();
    }
    alternates
}

/// the voices' names in the creator: in the Japanese game each voice is a
/// speaking style, and Haiku makes the US creator's three voices pick the
/// three styles (overrides/tones.c), so they are named by the styles, in
/// the translation's words (its five Japanese voices, "Spirited 1",
/// "Spirited 2", "Mature 1", "Mature 2", "Gentle": styles 1, 1, 2, 2, 3).
fn name_voices(merged: Vec<u8>, names: &mut [[String; 3]; 2], theirs: &[u8]) -> Vec<u8> {
    let own: HashMap<Vec<u8>, String> = zakuro_fs::msgarc::parse(theirs).unwrap_or_default().into_iter().map(|m| (m.label, String::from_utf16_lossy(&m.text))).collect();
    let mut set = Vec::new();
    for (gender, word) in ["MAN", "WOMAN"].iter().enumerate() {
        for (style, voice) in [0, 2, 4].iter().enumerate() {
            let label = format!("MID_MYUNIT_VOICE_{word}_0{voice}");
            if let Some(text) = own.get(label.as_bytes()) {
                let text = text.trim();
                let text = text.strip_suffix(" 1").or_else(|| text.strip_suffix(" 2")).unwrap_or(text).trim();
                if !text.is_empty() && text.chars().count() <= 16 {
                    names[gender][style] = text.to_owned();
                }
            }
        }
        for style in 0..3 {
            set.push(zakuro_fs::msgarc::Message::new(format!("MID_MYUNIT_VOICE_{word}_0{style}").as_bytes(), &names[gender][style]));
        }
    }
    zakuro_fs::msgarc::set(&merged, &set).unwrap_or(merged)
}

fn trace() -> bool {
    std::env::var_os("HAIKU_MSG_TRACE").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zakuro_fs::msgarc::{self, Message};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("haiku-fanpatch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, path: &str, data: &[u8]) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    fn archive(name: &str, messages: &[(&str, &str)]) -> Vec<u8> {
        let messages: Vec<Message> = messages.iter().map(|(label, text)| Message::new(label.as_bytes(), text)).collect();
        zakuro_fs::lz11::compress(&msgarc::new(name.as_bytes(), &messages))
    }

    #[test]
    fn letters_the_fonts_lack_are_written_as_near_ones() {
        let all = |s: &str| s.chars().map(|c| c as u32).collect::<std::collections::HashSet<u32>>();
        // the talk font has ≪ (as the US one does) and ’, the game's lines
        // use ’ but never ≪
        let fonts = Fonts { talk: all("Hello o'.\"\u{226A}\u{226B}\u{2019}"), menus: all("LV<>\"%o"), used: all("\u{2019}") };
        let text = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        // a talk line: the curly quote stays, ō and ≪≫ go
        assert_eq!(fonts.fallback(&text("$Wsx|Hōll’o ≪o≫")), Some(text("$Wsx|Holl’o \"o\"")));
        // a menu line: ≪ and ％ as the menus can show them
        assert_eq!(fonts.fallback(&text("≪LV≫ ％")), Some(text("\"LV\" %")));
        assert_eq!(fonts.fallback(&text("Hello")), None);
    }

    #[test]
    fn japanese_paths_go_to_the_language_folder() {
        // jpprep: on the Japanese build (no language folder) every path stays
        assert_eq!(us_path("m/X002.bin.lz", ""), "m/X002.bin.lz");
        assert_eq!(us_path("m/A/A007.bin.lz", ""), "m/A/A007.bin.lz");
        assert_eq!(us_path("m/X002.bin.lz", "@E"), "m/@E/X002.bin.lz");
        assert_eq!(us_path("m/A/A007.bin.lz", "@E"), "m/A/@E/A007.bin.lz");
        assert_eq!(us_path("m/C/C010.bin.lz", "@U"), "m/C/@U/C010.bin.lz");
        assert_eq!(us_path("m/@E/X002.bin.lz", "@E"), "m/@E/X002.bin.lz");
        assert_eq!(us_path("font/System.bfnt.lz", "@E"), "font/System.bfnt.lz");
        assert_eq!(parse_title("000400000012DE00"), Some(JP_SE));
        assert_eq!(parse_title("eur"), Some(zakuro_core::region::EUR));
        assert_eq!(title_id("0004000000179800"), Some(0x0004_0000_0017_9800));
        assert_eq!(title_id("rom"), None);
    }

    /// the region check: code made for another title is never recompiled
    /// against this build's; the mod's files are still used.
    #[test]
    fn code_for_another_title_is_left_out() {
        let root = folder("region");
        let mods = root.join("mods");
        // a Japanese title folder (Luma) with a code patch and text
        put(&mods, "jp-luma/luma/titles/000400000012DE00/code.ips", b"PATCHEOF");
        put(&mods, "jp-luma/luma/titles/000400000012DE00/romfs/m/GMap.bin.lz", b"x");
        // the endings patch's shape: romfs/ in the Japanese layout, code.ips
        put(&mods, "endings/code.ips", b"PATCHEOF");
        put(&mods, "endings/romfs/m/X001.bin.lz", b"x");
        put(&mods, "endings/UI/tex1_8x8_0000000000000000_13.png", b"png");
        // an EUR code mod: its exheader says so
        let mut exheader = vec![0u8; 0x400];
        exheader[0x200..0x208].copy_from_slice(&zakuro_core::region::EUR.to_le_bytes());
        put(&mods, "eur-code/exheader.bin", &exheader);
        put(&mods, "eur-code/code.ips", b"PATCHEOF");
        put(&mods, "eur-code/romfs/GameData/GameData.bin.lz", b"g");
        // a mod.toml says which title
        put(&mods, "said/mod.toml", b"title = \"000400000012DE00\"\n");
        put(&mods, "said/code.ips", b"PATCHEOF");
        put(&mods, "said/romfs/a.bin", b"a");
        // a USA code mod, as before
        put(&mods, "usa/luma/titles/0004000000179800/code.ips", b"PATCHEOF");
        put(&mods, "usa/luma/titles/0004000000179800/romfs/a.bin", b"a");
        let list = crate::haiku_mods::ModList::load(&root, Some(&root.join("userdata")), Some(&root.join("cache")));
        let by = |id: &str| list.mods.iter().position(|info| info.id == id).unwrap();
        for id in ["jp-luma", "endings", "said"] {
            let info = &list.mods[by(id)];
            assert!(info.code_files.is_none(), "{id}: its code is left out");
            assert!(info.code.is_empty(), "{id}");
            assert!(list.can_enable(by(id)), "{id}: its files are still used");
            let notes = &list.notes[by(id)];
            assert!(notes.iter().any(|note| note.text.contains("so it's left out")), "{id}: {notes:?}");
        }
        let endings = &list.notes[by("endings")];
        assert!(endings.iter().any(|note| note.text == "Its code patch is for the Japanese game (Fire Emblem if), so it's left out. (code.ips)"), "{endings:?}");
        assert_eq!(list.mods[by("endings")].description, JAPANESE_NOTE);
        assert_eq!(list.mods[by("endings")].files, ["m/@e/x001.bin.lz"], "its text is laid out for this build");
        assert_eq!(list.mods[by("jp-luma")].files, ["m/@e/gmap.bin.lz"]);
        // an EUR code mod found by its exheader: the one region check
        // (eur2's, when the code mods are planned) refuses it on this build
        let eur = &list.mods[by("eur-code")];
        assert_eq!(eur.title.as_deref(), Some("000400000017A800"));
        assert!(eur.code_files.is_some());
        assert!(crate::haiku_mods::code_region_problem(eur.title.as_deref()).is_some_and(|p| p.contains("European Special Edition")));
        assert_eq!(list.mods[by("said")].title.as_deref(), Some("000400000012DE00"));
        assert_eq!(crate::haiku_mods::code_region_problem(Some("jpn")).as_deref(), Some(JAPANESE_CODE));
        assert_eq!(crate::haiku_mods::code_region_problem(Some("0004000000179800")), None);
        let usa = &list.mods[by("usa")];
        assert!(usa.code_files.is_some(), "a USA code mod is a code mod as before");
        assert!(usa.fanpatch.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Team If's zip as it is (rom/ exe/ dlc/ IfPatcher.exe), v1.8's rom
    /// folder in a folder of its own, and a rom folder put in as the mod.
    #[test]
    fn team_if_layouts_are_read() {
        let root = folder("teamif");
        let mods = root.join("mods");
        let text = archive("X002", &[("MID_X002_OP1_1", "Hi")]);
        let zip_files: Vec<(&str, &[u8])> = vec![
            ("IfPatcher.exe", b"MZ"),
            ("exe/banner.bnr", b"b"),
            ("dlc/0001.00000017/m/A/A007.bin.lz", b"d"),
            ("rom/m/X002.bin.lz", &text),
            ("rom/m/A/A007.bin.lz", &text),
            ("rom/font/System.bfnt.lz", b"f"),
            ("rom/effect/Tlp_Title_A.arc.lz", b"t"),
            ("rom/ui/TitleLogo.bch.lz", b"t"),
            ("rom/ui/Battle.bch.lz", b"u"),
            ("rom/GameData/TextEditor.bin.lz", b"k"),
        ];
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("IfTranslationPatch_4-3.zip"), crate::haiku_mods::tests_zip(&zip_files)).unwrap();
        put(&mods, "Uncensored/TeamIFPatcher/rom/m/X002.bin.lz", &text);
        put(&mods, "Uncensored/TeamIFPatcher/rom/m/C/C010.bin.lz", &text);
        put(&mods, "rom/m/X002.bin.lz", &text);
        std::fs::write(mods.join("FE_Fates Uncensored Patch 1.8.rar"), b"Rar!").unwrap();
        let mut list = crate::haiku_mods::ModList::load(&root, Some(&root.join("userdata")), Some(&root.join("cache")));
        let by = |list: &crate::haiku_mods::ModList, id: &str| list.mods.iter().position(|info| info.id == id).unwrap();
        let zip = by(&list, "IfTranslationPatch_4-3");
        let info = &list.mods[zip];
        assert_eq!(info.files, ["gamedata/texteditor.bin.lz", "m/@e/x002.bin.lz", "m/a/@e/a007.bin.lz", "ui/battle.bch.lz"], "fonts and the logo are extras");
        assert_eq!(info.gameplay, Some(false), "a text patch is for looks");
        let fan = info.fanpatch.as_ref().unwrap();
        assert!(fan.team_if && fan.converted && fan.title == Some(JP_SE));
        assert_eq!(fan.extras.len(), 2);
        assert!(list.can_enable(zip));
        assert!(list.set_enabled(zip, true));
        // the logo by default, the fonts not
        let dirs = list.content_dirs();
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert!(dirs[1].ends_with("title"));
        list.set_option(zip, "fonts", 1);
        assert_eq!(list.content_dirs().len(), 3);
        let notes: Vec<&str> = list.notes[zip].iter().map(|note| note.text.as_str()).collect();
        assert_eq!(list.mods[zip].description, JAPANESE_NOTE);
        assert!(notes.iter().any(|note| note.starts_with("Not needed here, and never run: exe/, dlc/, IfPatcher.exe")), "{notes:?}");
        assert!(notes.iter().any(|note| note.starts_with("Its title logo is used; its Japanese fonts are off")), "{notes:?}");
        let rom = list.mods[by(&list, "Uncensored")].clone();
        assert_eq!(rom.files, ["m/@e/x002.bin.lz", "m/c/@e/c010.bin.lz"]);
        let bare = list.mods[by(&list, "rom")].clone();
        assert_eq!(bare.files, ["m/@e/x002.bin.lz"]);
        assert!(bare.fanpatch.as_ref().unwrap().team_if);
        let rar = by(&list, "FE_Fates Uncensored Patch 1.8");
        assert!(!list.can_enable(rar));
        assert!(list.mods[rar].problem.as_ref().unwrap().contains("Extract it with 7-Zip"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Haiku's lines go into a mod's archive at start, the mod's own
    /// wording of a line Haiku rewrites stays, the game's lines it lacks are
    /// filled in; a second start uses what was made; no mod text, nothing.
    /// tones: a Japanese-layout mod's talk tones become alternates of the
    /// archives the game reads, the same size in each style, the style's
    /// lines under the tone-1 labels; the creator's voices are named by the
    /// styles in the translation's own words.
    #[test]
    fn speaking_styles_become_alternates() {
        let root = folder("tones");
        let game = root.join("game");
        put(&game, "romfs/m/@E/A001.bin.lz", &archive("A001", &[("MID_A_PCM1", "Hey."), ("MID_A_PCF1", "Hello."), ("MID_B", "plain")]));
        put(&game, "romfs/m/@E/MyUnitEdit.bin.lz", &archive("MyUnitEdit", &[("MID_MYUNIT_VOICE_MAN_00", "M1"), ("MID_MYUNIT_VOICE_WOMAN_00", "F1")]));
        put(&game, "romfs/m/@E/Plain.bin.lz", &archive("Plain", &[("MID_P", "p")]));
        let theirs = root.join("mod");
        put(&theirs, "romfs/m/@E/A001.bin.lz", &archive("A001", &[
            ("MID_A_PCM1", "Yo."), ("MID_A_PCM2", "Hm."), ("MID_A_PCM3", "Good day, everyone."),
            ("MID_A_PCF1", "Hi."), ("MID_A_PCF2", "Hi."), ("MID_A_PCF3", "Hi!"), ("MID_B", "plain"),
        ]));
        put(&theirs, "romfs/m/@E/MyUnitEdit.bin.lz", &archive("MyUnitEdit", &[
            ("MID_MYUNIT_VOICE_MAN_00", "Spirited 1"), ("MID_MYUNIT_VOICE_MAN_01", "Spirited 2"), ("MID_MYUNIT_VOICE_MAN_02", "Mature 1"),
            ("MID_MYUNIT_VOICE_MAN_03", "Mature 2"), ("MID_MYUNIT_VOICE_MAN_04", "Gentle"),
            ("MID_MYUNIT_VOICE_WOMAN_00", "Polite 1"), ("MID_MYUNIT_VOICE_WOMAN_02", "Youthful 1"), ("MID_MYUNIT_VOICE_WOMAN_04", "Proud"),
        ]));
        put(&theirs, "romfs/m/@E/Plain.bin.lz", &archive("Plain", &[("MID_P", "pee")]));
        std::fs::write(theirs.join(JAPANESE_MARKER), "").unwrap();
        let cache = root.join("cache");
        let mut dirs = vec![theirs.clone()];
        let made = lay_text_with(&mut dirs, &game, &[], &cache, true).unwrap();
        assert_eq!(made.tones, 1);
        let layer = dirs.last().unwrap().clone();
        let base = std::fs::read(layer.join("romfs/m/@E/A001.bin.lz")).unwrap();
        let two = std::fs::read(layer.join("alt/1/romfs/m/@E/A001.bin.lz")).unwrap();
        let three = std::fs::read(layer.join("alt/2/romfs/m/@E/A001.bin.lz")).unwrap();
        assert!(base.len() == two.len() && two.len() == three.len(), "every style the same size");
        let text = |raw: &[u8], label: &str| {
            let messages = msgarc::parse(&zakuro_fs::lz11::decompress(raw).unwrap()).unwrap();
            messages.iter().find(|m| m.label == label.as_bytes()).map(|m| String::from_utf16_lossy(&m.text))
        };
        assert_eq!(text(&base, "MID_A_PCM1").as_deref(), Some("Yo."));
        assert_eq!(text(&two, "MID_A_PCM1").as_deref(), Some("Hm."));
        assert_eq!(text(&three, "MID_A_PCM1").as_deref(), Some("Good day, everyone."));
        assert_eq!(text(&two, "MID_A_PCF1").as_deref(), Some("Hi."));
        assert_eq!(text(&three, "MID_A_PCF1").as_deref(), Some("Hi!"));
        assert_eq!(text(&three, "MID_A_PCM3"), None, "the other tones stay pruned");
        assert!(!layer.join("alt/1/romfs/m/@E/Plain.bin.lz").exists(), "an archive without tones has no alternates");
        let creator = std::fs::read(layer.join("romfs/m/@E/MyUnitEdit.bin.lz")).unwrap();
        assert_eq!(text(&creator, "MID_MYUNIT_VOICE_MAN_00").as_deref(), Some("Spirited"));
        assert_eq!(text(&creator, "MID_MYUNIT_VOICE_MAN_01").as_deref(), Some("Mature"));
        assert_eq!(text(&creator, "MID_MYUNIT_VOICE_MAN_02").as_deref(), Some("Gentle"));
        assert_eq!(text(&creator, "MID_MYUNIT_VOICE_WOMAN_01").as_deref(), Some("Youthful"));
        let names = std::fs::read_to_string(layer.join(TONES_FILE)).unwrap();
        assert!(names.contains("male=Spirited|Mature|Gentle") && names.contains("female=Polite|Youthful|Proud"), "{names}");
        // the Layered file system reads the style ALTERNATE picks
        let romfs_dir = root.join("romfs-game");
        put(&romfs_dir, "m/@E/A001.bin.lz", &archive("A001", &[("MID_A_PCM1", "Hey.")]));
        let list = vec![("m/@E/A001.bin.lz".to_owned(), std::fs::metadata(romfs_dir.join("m/@E/A001.bin.lz")).unwrap().len())];
        let layered = zakuro_fs::layered::Layered::from_list_with_mods(&romfs_dir, &list, &[layer.as_path()]).unwrap();
        let mut whole = vec![0; layered.len() as usize];
        let read_style = |pick: usize, whole: &mut Vec<u8>| {
            zakuro_fs::layered::ALTERNATE.store(pick, std::sync::atomic::Ordering::Relaxed);
            layered.read(&[], 0, whole);
            whole.windows(three.len()).any(|w| w == if pick == 2 { &three[..] } else if pick == 1 { &two[..] } else { &base[..] })
        };
        assert!(read_style(2, &mut whole) && read_style(1, &mut whole) && read_style(0, &mut whole));
        zakuro_fs::layered::ALTERNATE.store(0, std::sync::atomic::Ordering::Relaxed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn haiku_text_is_laid_over_a_mods() {
        let root = folder("text");
        let game = root.join("game");
        put(&game, "romfs/m/@E/ResidentMenu.bin.lz", &archive("ResidentMenu", &[("MID_MENU_終了", "End"), ("MID_US", "US only")]));
        put(&game, "romfs/m/@E/Other.bin.lz", &archive("Other", &[("MID_O", "o")]));
        let builtin = root.join("builtin");
        put(&builtin, "romfs/m/@E/ResidentMenu.bin.lz", &archive("ResidentMenu", &[("MID_MENU_終了", "End Turn"), ("MID_US", "US only"), ("MID_HAIKU", "Haiku")]));
        put(&builtin, "romfs/m/@E/Haiku_Only.bin.lz", &archive("Haiku_Only", &[("MID_H", "h")]));
        let theirs = root.join("mod");
        put(&theirs, "romfs/m/@E/ResidentMenu.bin.lz", &archive("ResidentMenu", &[("MID_MENU_終了", "End"), ("MID_JP", "JP")]));
        put(&theirs, "romfs/m/@E/Other.bin.lz", &archive("Other", &[("MID_O", "oh")]));
        let cache = root.join("cache");
        // no mod: nothing at all
        let mut plain = vec![builtin.clone()];
        assert_eq!(lay_text_with(&mut plain, &game, &[builtin.clone()], &cache, true), None);
        assert_eq!(plain, [builtin.clone()]);
        let mut dirs = vec![builtin.clone(), theirs.clone()];
        let made = lay_text_with(&mut dirs, &game, &[builtin.clone()], &cache, true).unwrap();
        assert_eq!(made, TextMerge { archives: 2, written: 1, filled: 1, added: 1, changed: 1, kept: 0, pruned: 0, letters: 0, tones: 0 });
        assert_eq!(dirs.len(), 3, "one more layer, on top");
        let merged = msgarc::parse(&read_lz(&dirs[2].join("romfs/m/@E/ResidentMenu.bin.lz")).unwrap()).unwrap();
        let text = |label: &str| merged.iter().find(|m| m.label == label.as_bytes()).map(|m| String::from_utf16_lossy(&m.text));
        assert_eq!(text("MID_MENU_終了").as_deref(), Some("End Turn"));
        assert_eq!(text("MID_HAIKU").as_deref(), Some("Haiku"));
        assert_eq!(text("MID_US").as_deref(), Some("US only"));
        assert_eq!(text("MID_JP").as_deref(), Some("JP"));
        assert!(!dirs[2].join("romfs/m/@E/Other.bin.lz").exists(), "nothing to add: the mod's file as it is");
        // the next start: the same layer, not made again
        let mut again = vec![builtin.clone(), theirs.clone()];
        assert_eq!(lay_text_with(&mut again, &game, &[builtin.clone()], &cache, true), None);
        assert_eq!(again, dirs);
        std::fs::remove_dir_all(root).unwrap();
    }
}
