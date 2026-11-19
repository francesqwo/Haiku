//! mods: Haiku's mod manager. Mods live in the `mods/` folder next to
//! Haiku.exe, each a folder or a .zip. Which are on, and their load order,
//! are kept in `userdata/haiku/mods.toml`. The mods turned on are handed to
//! the loader as content folders (`Config::mods`), laid over the game's
//! RomFS in order (later ones win), and their texture packs are merged
//! under the controller glyphs (see glyphs.rs).
//!
//! A mod's layout, any of:
//!
//! ```text
//! mods/<name>/mod.toml            id, name, version, author, description,
//!                                 dependencies, conflicts, gameplay (all optional)
//! mods/<name>/romfs/...           files that replace or add the game's
//! mods/<name>/romfs_ext/...       .ips/.bps patches and .stub removals
//! mods/<name>/dlc/<IIII>/romfs/   files for DLC content IIII (4 hex digits)
//! mods/<name>/textures/...        a texture pack (Citra's naming)
//! mods/<name>/luma/titles/0004000000179800/...   a Luma3DS mod as it is
//! mods/<name>.zip                 any of the above, zipped
//! ```
//!
//! codemods: code changes (code.ips, code.bps, code.bin, exheader.bin) and
//! native plugins (plugin.dll) are code mods, which haiku_codemods
//! recompiles and loads. Code modules in the RomFS (cro/*.cro, static.crs)
//! can't run: Fates has none and never loads one, so a mod with them stays
//! off.
//!
//! online2: online battles need the same gameplay mods on both PCs (the
//! battle is worked out on each from the same data and code). A mod is a
//! gameplay mod when its mod.toml says `gameplay = true`; with no word
//! there (`gameplay = false` says it is only for looks), when it changes
//! code (a code mod or a plugin), or game files that feed the game's rules:
//! anything but pictures, models, animations, effects, sounds, music,
//! movies, fonts, text and the interface's layouts (see `gameplay_file`).
//! Texture packs never count. `ModList::online_stamps` gives the mods in
//! use with a hash of what they change, for the online handshake.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// the game's title id, the name of its folder in a Luma3DS mod.
pub const PROGRAM_ID: &str = "0004000000179800";
/// eur2: the EUR Special Edition's title id.
pub const PROGRAM_ID_EUR: &str = "000400000017A800";
/// jpprep: the Japanese builds' title ids (Fire Emblem if: the Special
/// Edition, Byakuya Oukoku, Anya Oukoku).
pub const PROGRAM_IDS_JPN: [&str; 3] = ["000400000012DE00", "000400000012DC00", "000400000012DD00"];

/// eur2: the title ids a mod's folders may be named for, the running game's
/// first (a USA mod's files still serve the EUR game: its RomFS paths are
/// the same but for the language folders).
pub fn program_ids() -> [&'static str; 2] {
    // jpprep: the Japanese build's own title first
    if zakuro_core::region::is_jpn() {
        let running = zakuro_core::region::title();
        let own = PROGRAM_IDS_JPN.into_iter().find(|id| u64::from_str_radix(id, 16).ok() == Some(running)).unwrap_or(PROGRAM_IDS_JPN[0]);
        return [own, PROGRAM_ID];
    }
    if zakuro_core::region::is_eur() {
        [PROGRAM_ID_EUR, PROGRAM_ID]
    } else {
        [PROGRAM_ID, PROGRAM_ID_EUR]
    }
}

/// eur2: why a code mod can't run on this game: it was made for the other
/// build of Fates (its Luma folder's title id, or mod.toml's `title`), or,
/// on the EUR game, it doesn't say it was made for it. Code mods patch the
/// game's code at fixed places, and the two builds' code is laid out
/// differently, so one made for the USA build would break the EUR one.
///
/// fanpatch: the same check covers the Japanese game (Fire Emblem if,
/// 000400000012DE00/DC00/DD00, or `jpn`; haiku_fanpatch also finds it from
/// a Japanese title folder, exheader.bin or Team If's layout) and any other
/// title. A mod made for the Japanese game keeps its files: haiku_fanpatch
/// leaves only its code out, as soon as the mod is read.
pub fn code_region_problem(title: Option<&str>) -> Option<String> {
    let eur = zakuro_core::region::is_eur();
    // fanpatch: title ids, or usa / eur / jpn
    let parsed = title.and_then(crate::haiku_fanpatch::parse_title);
    // jpprep: on a Japanese build a code mod must be made for that very
    // build (the Uncensored Patch's endings code.ips is, for the Special
    // Edition); the USA and EUR builds answer as before
    if zakuro_core::region::is_jpn() {
        return jpn_code_problem(parsed);
    }
    if parsed.is_some_and(crate::haiku_fanpatch::is_japanese) {
        return Some(crate::haiku_fanpatch::JAPANESE_CODE.to_owned());
    }
    if let Some(other) = parsed.filter(|id| ![zakuro_core::region::USA, zakuro_core::region::EUR].contains(id)) {
        return Some(format!("Its code changes are made for another game ({other:016X}), so they can't be used."));
    }
    let made_for_eur = parsed == Some(zakuro_core::region::EUR);
    let made_for_usa = parsed == Some(zakuro_core::region::USA);
    if eur && !made_for_eur {
        Some(format!(
            "Its code changes are made for the USA version of Fire Emblem Fates ({}), and your game is the European Special Edition, whose code is laid out differently, so they can't be used. A code mod made for the European version says so with title = \"{PROGRAM_ID_EUR}\" in its mod.toml, or sits in luma/titles/{PROGRAM_ID_EUR}/.",
            if made_for_usa { "its folder says 0004000000179800" } else { "it doesn't say it is made for the European one" }
        ))
    } else if !eur && made_for_eur {
        Some(format!("Its code changes are made for the European Special Edition ({PROGRAM_ID_EUR}), and your game is the USA version, whose code is laid out differently, so they can't be used."))
    } else {
        None
    }
}
/// jpprep: code_region_problem on a Japanese build (Fire Emblem if).
fn jpn_code_problem(parsed: Option<u64>) -> Option<String> {
    let running = zakuro_core::region::title();
    match parsed {
        Some(title) if title == running => None,
        Some(title) if crate::haiku_fanpatch::is_japanese(title) => Some(format!(
            "Its code changes are made for another Japanese edition ({title:016X}), and your game is {running:016X}, whose code is laid out differently, so they can't be used."
        )),
        Some(zakuro_core::region::USA) | Some(zakuro_core::region::EUR) => Some(format!(
            "Its code changes are made for {}, and your game is the Japanese one (Fire Emblem if), whose code is laid out differently, so they can't be used.",
            crate::haiku_fanpatch::title_name(parsed.unwrap_or_default())
        )),
        Some(other) => Some(format!("Its code changes are made for another game ({other:016X}), so they can't be used.")),
        None => Some(format!(
            "It doesn't say which game its code changes are made for, and your game is the Japanese one (Fire Emblem if), so they can't be used. A code mod made for it says so with title = \"{running:016X}\" in its mod.toml, or sits in luma/titles/{running:016X}/."
        )),
    }
}

/// the list of mods turned on, beside settings.toml.
pub const CONFIG_FILE: &str = "mods.toml";

/// what mod.toml says.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Manifest {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    author: Option<String>,
    description: Option<String>,
    dependencies: Vec<String>,
    conflicts: Vec<String>,
    /// codemods: the native plugin's DLL, in the mod's folder.
    plugin: Option<String>,
    /// codemods: the plugin's options, which the Mods screen sets.
    options: Vec<ModOption>,
    /// online2: whether it changes how the game plays (true) or only how it
    /// looks and sounds (false), for online battles; none: Haiku decides
    /// from what it changes.
    gameplay: Option<bool>,
    /// eur2: the build of the game its code changes are made for (a title
    /// id: 0004000000179800 USA, 000400000017A800 EUR); none: the USA one.
    /// fanpatch: also 000400000012DE00/DC00/DD00 (the Japanese game), or
    /// usa / eur / jpn.
    title: Option<String>,
}

/// codemods: an option a plugin reads (haiku_mod.h, api->option), declared
/// in its mod.toml: an on/off switch without min and max, a number with.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModOption {
    pub id: String,
    pub label: String,
    pub default: i64,
    pub min: Option<i64>,
    pub max: Option<i64>,
}

/// a note the Mods screen shows about a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// a warning, drawn in red, rather than a plain note.
    pub warning: bool,
    pub text: String,
}

/// a mod found in the mods folder.
#[derive(Debug, Clone, Default)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<String>,
    /// the folder or .zip in the mods folder.
    pub source: PathBuf,
    /// the folder its romfs/, textures/ and so on are in (a zip's is in the
    /// cache), none when it could not be read.
    pub content: Option<PathBuf>,
    /// the game's files it changes, lower case, / separated, with the
    /// DLC's under dlc/<IIII>/.
    pub files: Vec<String>,
    /// its texture pack, and how many pictures it has.
    pub textures: Option<PathBuf>,
    pub pictures: Vec<String>,
    /// the files that change the game's code, by name.
    pub code: Vec<String>,
    /// codemods: the code patches and the plugin, for haiku_codemods.
    pub code_files: Option<crate::haiku_codemods::CodeFiles>,
    /// codemods: it has code modules (.cro, .crs), which can't run.
    pub modules: bool,
    /// codemods: its plugin's options.
    pub options: Vec<ModOption>,
    /// online2: what its mod.toml says of gameplay, if anything.
    pub gameplay: Option<bool>,
    /// eur2: the title id it is made for: its Luma or Citra folder's, else
    /// its mod.toml's `title`.
    pub title: Option<String>,
    /// why it can't be used, when it can't.
    pub problem: Option<String>,
    /// hdpacks: a download still in its zips, unpacked when turned on
    /// (haiku_import.rs)
    pub download: Option<crate::haiku_import::Download>,
    /// fanpatch: a mod made for another release (Team If's layout, a
    /// Japanese or EUR title): what Haiku made of it
    pub fanpatch: Option<crate::haiku_fanpatch::FanPatch>,
}

impl ModInfo {
    /// whether it has anything Haiku can use.
    fn has_content(&self) -> bool {
        !self.files.is_empty() || !self.pictures.is_empty() || self.code_files.is_some() || self.download.is_some()
    }
}

/// an entry of mods.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    id: String,
    enabled: bool,
    /// use the mod's files even though it also changes the game's code,
    /// which is left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    files_only: bool,
    /// codemods: its plugin's options as the player set them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    options: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default, rename = "mod")]
    mods: Vec<Entry>,
}

/// the mods in the mods folder, in load order, and which are on.
#[derive(Default)]
pub struct ModList {
    /// the mods folder, the list file and the zips' cache.
    pub folder: PathBuf,
    config: Option<PathBuf>,
    cache: Option<PathBuf>,
    pub mods: Vec<ModInfo>,
    pub enabled: Vec<bool>,
    files_only: HashSet<String>,
    /// entries of the list whose mod is not in the folder now (taken out
    /// for a while, or a zip being replaced): kept, so they come back as
    /// they were.
    /// each after the id of the mod before it in the list (none: first).
    missing: Vec<(Option<String>, Entry)>,
    /// the notes each mod has, conflicts and warnings, by index.
    pub notes: Vec<Vec<Note>>,
    /// which were on, in order, when the running game started.
    pub booted: Option<Vec<String>>,
    /// a message for the player after a scan, a mod that could not be read.
    pub scanned: bool,
    /// codemods: notes about the code mods, by id, which the frontend works
    /// out (compiled or not, conflicts, Haiku's own functions they change).
    pub code_notes: HashMap<String, Vec<Note>>,
    /// codemods: mods left out of the running game whole, by id: their code
    /// could not be used, so their files are not either.
    pub left_out: HashSet<String>,
    /// codemods: the plugins' options, by mod id.
    options: HashMap<String, BTreeMap<String, i64>>,
}

/// the folder Haiku runs from: FATES_ROOT, or the program's.
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("FATES_ROOT") {
        return PathBuf::from(root);
    }
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

impl ModList {
    /// the mods in root/mods, with the list in data_dir and the zips
    /// unpacked under cache_dir.
    pub fn load(root: &Path, data_dir: Option<&Path>, cache_dir: Option<&Path>) -> ModList {
        let mut list = ModList {
            folder: root.join("mods"),
            config: data_dir.map(|dir| dir.join(CONFIG_FILE)),
            cache: cache_dir.map(|dir| dir.join("mods")),
            ..ModList::default()
        };
        list.scan();
        list
    }

    /// Haiku's list: root() / mods, the list in APPDATA/haiku, the cache in
    /// LOCALAPPDATA/haiku (Haiku points both into its own folder).
    pub fn haiku(data_dir: Option<&Path>) -> ModList {
        let cache = std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("haiku"));
        ModList::load(&root(), data_dir, cache.as_deref())
    }

    /// reads the mods folder and the list again.
    pub fn scan(&mut self) {
        let started = std::time::Instant::now();
        let config = self.read_config();
        let mut found = Vec::new();
        // hdpacks: large zips, zips of zips and split downloads are
        // downloads, unpacked on their own thread when turned on, never
        // here; the small zips are unpacked into the cache as before
        let (downloads, small) = crate::haiku_import::find(&self.folder);
        if let Ok(entries) = std::fs::read_dir(&self.folder) {
            let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
            paths.sort();
            for path in paths {
                let zip = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
                if !path.is_dir() && !zip {
                    // fanpatch: a RAR or 7z says to extract it first
                    found.extend(crate::haiku_fanpatch::archive(&path));
                    continue;
                }
                // hdpacks: the downloads put away once unpacked, and the
                // folders being unpacked into
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                if path.is_dir() && (name.eq_ignore_ascii_case(crate::haiku_import::DOWNLOADS) || name.starts_with('.')) {
                    continue;
                }
                if zip && !small.contains(&path) {
                    continue;
                }
                found.push(read_mod(&path, self.cache.as_deref()));
            }
        }
        for download in downloads {
            found.push(ModInfo {
                id: download.id.clone(),
                name: download.name.clone(),
                description: download_description(&download),
                source: download.parts[0].clone(),
                download: Some(download),
                ..ModInfo::default()
            });
        }
        if let Some(cache) = &self.cache {
            prune_cache(cache, &small);
        }
        // ids are unique: a second mod with one is told apart by its file
        let mut seen = HashSet::new();
        for info in &mut found {
            if !seen.insert(info.id.to_lowercase()) {
                let file = info.source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                info.id = format!("{}@{file}", info.id);
            }
            seen.insert(info.id.to_lowercase());
        }
        // the list's order first, the mods it doesn't know after, off
        let mut ordered = Vec::new();
        let mut enabled = Vec::new();
        for entry in &config.mods {
            if let Some(at) = found.iter().position(|info| info.id.eq_ignore_ascii_case(&entry.id)) {
                ordered.push(found.remove(at));
                enabled.push(entry.enabled);
            }
        }
        for info in found {
            log::info!("mods: found {} ({}), off until it is turned on", info.name, info.source.display());
            ordered.push(info);
            enabled.push(false);
        }
        self.missing.clear();
        let mut before: Option<String> = None;
        for entry in &config.mods {
            if ordered.iter().any(|info| info.id.eq_ignore_ascii_case(&entry.id)) {
                before = Some(entry.id.clone());
            } else {
                self.missing.push((before.clone(), entry.clone()));
            }
        }
        self.files_only = config.mods.iter().filter(|entry| entry.files_only).map(|entry| entry.id.to_lowercase()).collect();
        self.options = config.mods.iter().filter(|entry| !entry.options.is_empty()).map(|entry| (entry.id.to_lowercase(), entry.options.clone())).collect();
        self.mods = ordered;
        self.enabled = enabled;
        for index in 0..self.mods.len() {
            if self.enabled[index] && !self.can_enable(index) {
                let info = &self.mods[index];
                let why = info.problem.clone().unwrap_or_else(|| if info.modules { format!("it has code modules ({}), which Fates never loads", info.code.join(", ")) } else { "it has no files for the game".into() });
                log::warn!("mods: {} stays off: {why}", info.name);
                self.enabled[index] = false;
            }
        }
        // hdpacks: a download turned on is unpacked (also one turned on
        // when Haiku was closed before it was done)
        for index in 0..self.mods.len() {
            if self.enabled[index] {
                if let Some(download) = &self.mods[index].download {
                    crate::haiku_import::start(download, &self.folder);
                }
            }
        }
        self.scanned = true;
        self.check();
        log::info!(
            "mods: {} in {}, {} on, read in {} ms",
            self.mods.len(),
            self.folder.display(),
            self.enabled.iter().filter(|on| **on).count(),
            started.elapsed().as_millis()
        );
        self.save();
    }

    fn read_config(&self) -> Config {
        let Some(path) = &self.config else { return Config::default() };
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|error| {
                log::warn!("mods: {} can't be read, {error}; every mod is off", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// writes the list, in load order.
    pub fn save(&self) {
        let Some(path) = &self.config else { return };
        // the mods here in order, each missing one back after the mod it
        // followed
        let missing_after = |id: Option<&str>| -> Vec<Entry> {
            let id = id.map(str::to_lowercase);
            self.missing.iter().filter(|(before, _)| before.as_deref().map(str::to_lowercase) == id).map(|(_, entry)| entry.clone()).collect()
        };
        let mut mods: Vec<Entry> = missing_after(None);
        for (info, enabled) in self.mods.iter().zip(&self.enabled) {
            let options = self.options.get(&info.id.to_lowercase()).cloned().unwrap_or_default();
            mods.push(Entry { id: info.id.clone(), enabled: *enabled, files_only: self.files_only.contains(&info.id.to_lowercase()), options });
            mods.extend(missing_after(Some(&info.id)));
        }
        // and those whose mod before them is gone too, at the end
        for (before, entry) in &self.missing {
            if !mods.iter().any(|known| known.id.eq_ignore_ascii_case(&entry.id)) {
                let _ = before;
                mods.push(entry.clone());
            }
        }
        let config = Config { mods };
        let mut text = String::from("# Haiku's mods, in load order: a mod's files win over those of the mods above it.\n# The Mods screen (Esc > Mods, or Mods on the title screen) changes this.\n\n");
        text += &toml::to_string(&config).unwrap_or_default();
        if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(path, text) {
            log::warn!("mods: could not write {}, {error}", path.display());
        }
    }

    /// whether the mod can be turned on: it has something Haiku can use,
    /// and no code modules (unless its entry says files_only). codemods:
    /// code patches and plugins are things Haiku can use.
    pub fn can_enable(&self, index: usize) -> bool {
        let Some(info) = self.mods.get(index) else { return false };
        info.problem.is_none() && info.has_content() && (!info.modules || self.files_only.contains(&info.id.to_lowercase()))
    }

    /// codemods: whether the mod's code is used: it is on, has code, and
    /// no files_only.
    pub fn uses_code(&self, index: usize) -> bool {
        self.enabled.get(index).copied().unwrap_or(false) && self.can_enable(index) && self.mods[index].code_files.is_some() && !self.files_only.contains(&self.mods[index].id.to_lowercase())
    }

    /// codemods: a plugin's options: what the player set, else the
    /// defaults its mod.toml gives.
    pub fn options_of(&self, index: usize) -> BTreeMap<String, i64> {
        let Some(info) = self.mods.get(index) else { return BTreeMap::new() };
        let mut options: BTreeMap<String, i64> = info.options.iter().map(|option| (option.id.clone(), option.default)).collect();
        if let Some(set) = self.options.get(&info.id.to_lowercase()) {
            options.extend(set.iter().map(|(id, value)| (id.clone(), *value)));
        }
        options
    }

    /// codemods: sets a plugin's option, kept in mods.toml.
    pub fn set_option(&mut self, index: usize, id: &str, value: i64) {
        let Some(info) = self.mods.get(index) else { return };
        self.options.entry(info.id.to_lowercase()).or_default().insert(id.to_owned(), value);
        self.save();
    }

    /// turns a mod on or off, and saves the list.
    pub fn set_enabled(&mut self, index: usize, on: bool) -> bool {
        if index >= self.mods.len() || (on && !self.can_enable(index)) {
            return false;
        }
        self.enabled[index] = on;
        log::info!("mods: {} turned {}", self.mods[index].name, if on { "on" } else { "off" });
        // hdpacks: a download is unpacked once it is turned on
        if on {
            if let Some(download) = &self.mods[index].download {
                crate::haiku_import::start(download, &self.folder);
            }
        }
        self.check();
        self.save();
        true
    }

    /// moves a mod up (-1, loaded earlier) or down (+1, later), and saves.
    pub fn shift(&mut self, index: usize, by: i32) -> Option<usize> {
        let to = index as i32 + by;
        if index >= self.mods.len() || to < 0 || to as usize >= self.mods.len() {
            return None;
        }
        let to = to as usize;
        self.mods.swap(index, to);
        self.enabled.swap(index, to);
        self.check();
        self.save();
        Some(to)
    }

    /// the ids of the mods on, in order.
    pub fn active(&self) -> Vec<String> {
        self.mods.iter().zip(&self.enabled).filter(|(_, on)| **on).map(|(info, _)| info.id.clone()).collect()
    }

    /// whether the list differs from the one the running game started with,
    /// so a restart would change something.
    pub fn pending(&self) -> bool {
        self.booted.as_ref().is_some_and(|booted| *booted != self.active())
    }

    /// the content folders of the mods on, in load order, for Config::mods.
    pub fn content_dirs(&self) -> Vec<PathBuf> {
        self.mods
            .iter()
            .enumerate()
            .filter(|(index, info)| self.enabled[*index] && self.can_enable(*index) && !info.files.is_empty() && !self.left_out.contains(&info.id))
            .flat_map(|(index, info)| crate::haiku_fanpatch::content_dirs(info, &self.options_of(index))) // fanpatch: and its extras
            .collect()
    }

    /// online2: the mods in use that change gameplay, with a hash of what
    /// they change (files, code, the plugin and its options), sorted by
    /// id. Visual mods and texture packs are left out.
    pub fn online_stamps(&self) -> Vec<zakuro_core::netplay::ModStamp> {
        let mut stamps = Vec::new();
        for (index, info) in self.mods.iter().enumerate() {
            if !self.enabled[index] || !self.can_enable(index) || self.left_out.contains(&info.id) || info.download.is_some() {
                continue;
            }
            let code = self.uses_code(index);
            let mut hash = Fnv::default();
            hash.add(info.id.to_lowercase().as_bytes());
            let mut feeds = code;
            if let Some(content) = &info.content {
                for (path, relative) in game_files(content) {
                    if gameplay_file(&relative) || info.gameplay == Some(true) {
                        feeds |= gameplay_file(&relative);
                        hash.add(relative.as_bytes());
                        hash.add_file(&path);
                    }
                }
            }
            if code {
                if let Some(files) = &info.code_files {
                    for path in files.code.iter().chain(files.patches.iter()).chain(files.exheader.iter()).chain(files.plugin.iter()) {
                        hash.add(path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default().as_bytes());
                        hash.add_file(path);
                    }
                }
                for (key, value) in self.options_of(index) {
                    hash.add(format!("{key}={value}").as_bytes());
                }
            }
            let gameplay = info.gameplay.unwrap_or(feeds);
            log::info!("mods: {} is a {} mod for online battles", info.name, if gameplay { "gameplay" } else { "visual" });
            if gameplay {
                stamps.push(zakuro_core::netplay::ModStamp { id: info.id.clone(), name: info.name.clone(), hash: hash.0 });
            }
        }
        stamps.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
        stamps
    }

    /// the texture packs of the mods on, in load order.
    pub fn texture_dirs(&self) -> Vec<PathBuf> {
        self.mods.iter().enumerate().filter(|(index, info)| self.enabled[*index] && self.can_enable(*index) && !self.left_out.contains(&info.id)).filter_map(|(_, info)| info.textures.clone()).collect()
    }

    /// works out each mod's notes: what stops it, the code it changes, the
    /// files it shares with other mods on, and what it needs or clashes with.
    pub fn check(&mut self) {
        let mut notes: Vec<Vec<Note>> = vec![Vec::new(); self.mods.len()];
        // who has each file, among the mods on, in order
        let mut owners: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, info) in self.mods.iter().enumerate() {
            if !self.enabled[index] {
                continue;
            }
            for file in info.files.iter().chain(info.pictures.iter()) {
                owners.entry(file.as_str()).or_default().push(index);
            }
        }
        let name = |index: usize| self.mods[index].name.clone();
        for (index, info) in self.mods.iter().enumerate() {
            let on = self.enabled[index];
            let note = &mut notes[index];
            if let Some(problem) = &info.problem {
                note.push(Note { warning: true, text: problem.clone() });
            } else if let Some(download) = &info.download {
                // hdpacks: still in its zips
                let size = gigabytes(download.packed);
                let parts = download.parts.len();
                let zips = if parts == 1 { format!("a {size} zip") } else { format!("{parts} zips, {size}") };
                let text = if crate::haiku_import::is_running(&info.id) {
                    format!("Unpacking {zips} into mods/{}...", info.id)
                } else if on {
                    format!("Still in {zips}: Haiku unpacks it into mods/{} now.", info.id)
                } else {
                    format!("Still in {zips}. Turn it on and Haiku unpacks it once, into mods/{}.", info.id)
                };
                note.push(Note { warning: false, text });
            } else if !info.has_content() && info.code.is_empty() {
                note.push(Note { warning: true, text: "It has no files for the game (romfs/ or textures/), so it does nothing.".into() });
            }
            if !info.code.is_empty() {
                let files = info.code.join(", ");
                if self.files_only.contains(&info.id.to_lowercase()) {
                    note.push(Note { warning: true, text: format!("Its code ({files}) is left out (files_only), and only its files are used.") });
                } else if info.modules {
                    note.push(Note { warning: true, text: format!("It has code modules ({files}). Fates has none and never loads one, so they can't run, and it stays off.") });
                } else {
                    note.push(Note { warning: false, text: format!("Code mod ({files}): Haiku recompiles the code it changes.") });
                }
            }
            if let Some(code_notes) = self.code_notes.get(&info.id) {
                note.extend(code_notes.iter().cloned());
            }
            note.extend(crate::haiku_fanpatch::notes(info, &self.options_of(index))); // fanpatch
            if !on {
                continue;
            }
            // shared files: those it takes from mods above, and those taken
            // from it by mods below
            let mut over: BTreeMap<usize, usize> = BTreeMap::new();
            let mut under: BTreeMap<usize, usize> = BTreeMap::new();
            for file in info.files.iter().chain(info.pictures.iter()) {
                if let Some(list) = owners.get(file.as_str()) {
                    for &other in list {
                        if other < index {
                            *over.entry(other).or_default() += 1;
                        } else if other > index {
                            *under.entry(other).or_default() += 1;
                        }
                    }
                }
            }
            for (other, count) in over {
                note.push(Note { warning: false, text: format!("Replaces {count} {} of {}, which loads before it.", plural(count, "file", "files"), name(other)) });
            }
            for (other, count) in under {
                note.push(Note { warning: true, text: format!("{count} of its files {} replaced by {}, which loads after it.", plural(count, "is", "are"), name(other)) });
            }
            for wanted in &info.dependencies {
                match self.mods.iter().position(|other| other.id.eq_ignore_ascii_case(wanted)) {
                    None => note.push(Note { warning: true, text: format!("It needs {wanted}, which isn't installed.") }),
                    Some(other) if !self.enabled[other] => note.push(Note { warning: true, text: format!("It needs {}, which is off.", name(other)) }),
                    Some(other) if other > index => note.push(Note { warning: true, text: format!("It needs {} to load before it: move it down.", name(other)) }),
                    _ => {}
                }
            }
            for clash in &info.conflicts {
                if let Some(other) = self.mods.iter().position(|other| other.id.eq_ignore_ascii_case(clash)).filter(|other| self.enabled[*other]) {
                    note.push(Note { warning: true, text: format!("It doesn't work with {}, which is on.", name(other)) });
                }
            }
        }
        // a mod others need, turned off, says so too
        for (index, info) in self.mods.iter().enumerate() {
            if !self.enabled[index] {
                continue;
            }
            for (other, them) in self.mods.iter().enumerate() {
                if other != index && !self.enabled[other] && info.dependencies.iter().any(|wanted| wanted.eq_ignore_ascii_case(&them.id)) {
                    notes[other].push(Note { warning: true, text: format!("{} needs it.", info.name) });
                }
            }
        }
        self.notes = notes;
    }
}

/// online2: FNV-1a over what a mod changes; both players' Haiku compute it
/// the same way.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn add(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
        // a separator, so ("ab", "c") and ("a", "bc") differ
        self.0 ^= 0xFF;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
    }

    fn add_file(&mut self, path: &Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.add(&bytes),
            Err(_) => self.add(b"(unreadable)"),
        }
    }
}

/// online2: a mod's files for the game, each with the RomFS path it
/// changes (lower case, / separated, a patch's or stub's target; the DLC's
/// under dlc/<IIII>/), sorted by that path.
fn game_files(content: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut walk = |dir: Option<PathBuf>, prefix: &str, patches: bool| {
        let Some(dir) = dir else { return };
        let mut files = Vec::new();
        list_files(&dir, "", &mut files);
        for file in files {
            let target = if patches {
                match file.rsplit_once('.') {
                    Some((target, kind)) if ["ips", "bps", "stub"].contains(&kind) => target.to_owned(),
                    _ => continue,
                }
            } else {
                file.clone()
            };
            out.push((dir.join(&file), format!("{prefix}{}", target.to_lowercase())));
        }
    };
    walk(find_ci(content, "romfs"), "", false);
    walk(find_ci(content, "romfs_ext"), "", true);
    if let Some(dlc) = find_ci(content, "dlc") {
        if let Ok(entries) = std::fs::read_dir(&dlc) {
            let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
            entries.sort();
            for entry in entries {
                let name = entry.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
                if name.len() == 4 && u16::from_str_radix(&name, 16).is_ok() {
                    walk(find_ci(&entry, "romfs"), &format!("dlc/{name}/"), false);
                }
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// online2: whether a game file feeds the game's rules (Haiku's safe rule
/// when a mod.toml doesn't say). Only what is plainly for looks and sound
/// is left out: the folders of models, animations, faces, effects, maps'
/// scenery, skies, sounds, movies, fonts, icons, the interface, the text
/// (m/), tutorials' pictures; and models, layouts and pictures anywhere.
/// The rest counts: GameData (characters, classes, items, skills, maps'
/// units and terrain), Scripts, castle and map data, params, and anything
/// unknown.
pub fn gameplay_file(path: &str) -> bool {
    const LOOKS: [&str; 24] = [
        "sound", "movie", "face", "live2d", "unit", "ba", "bu", "bx", "bp", "bs", "effect", "field", "sky", "font", "icon", "ui", "m", "scene", "shaders", "common", "sample", "debug", "talk", "tut",
    ];
    const LOOK_FILES: [&str; 13] = [".bch", ".bch.lz", ".arc", ".arc.lz", ".bcfnt", ".bffnt", ".bclim", ".bflim", ".ctpk", ".png", ".bcstm", ".bcwav", ".moflex"];
    let path = path.to_lowercase();
    // the DLC's files are laid out as the game's
    let path = match path.strip_prefix("dlc/") {
        Some(rest) => rest.split_once('/').map_or(rest, |(_, file)| file).to_owned(),
        None => path,
    };
    let top = path.split('/').next().unwrap_or("");
    if path.contains('/') && LOOKS.contains(&top) {
        return false;
    }
    if LOOK_FILES.iter().any(|ext| path.ends_with(ext)) || path == "texturecoordinate.bin" {
        return false;
    }
    true
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        one
    } else {
        many
    }
}

/// reads a mod in the mods folder, a folder or a zip.
fn read_mod(source: &Path, cache: Option<&Path>) -> ModInfo {
    let stem = source.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let mut info = ModInfo { id: stem.clone(), name: stem, source: source.to_path_buf(), ..ModInfo::default() };
    let folder = if source.is_dir() {
        source.to_path_buf()
    } else {
        match unpack(source, cache) {
            Ok(folder) => folder,
            Err(problem) => {
                log::warn!("mods: {} can't be read, {problem}", source.display());
                info.problem = Some(problem);
                return info;
            }
        }
    };
    let folder = wrapped(&folder);
    let mut plugin: Option<String> = None; // codemods
    if let Some(path) = find_ci(&folder, "mod.toml") {
        match std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|text| toml::from_str::<Manifest>(&text).map_err(|error| error.to_string())) {
            Ok(manifest) => {
                if let Some(id) = manifest.id.filter(|id| !id.trim().is_empty()) {
                    info.id = id.trim().to_owned();
                }
                if let Some(name) = manifest.name.filter(|name| !name.trim().is_empty()) {
                    info.name = name.trim().to_owned();
                }
                info.version = manifest.version.unwrap_or_default();
                info.author = manifest.author.unwrap_or_default();
                info.description = manifest.description.unwrap_or_default().trim().to_owned();
                info.dependencies = manifest.dependencies;
                info.conflicts = manifest.conflicts;
                plugin = manifest.plugin;
                info.options = manifest.options;
                info.gameplay = manifest.gameplay;
                info.title = manifest.title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
            }
            Err(error) => {
                log::warn!("mods: {} can't be read, {error}", path.display());
                info.problem = Some("Its mod.toml can't be read.".into());
            }
        }
    }
    let (content, titled) = content_folder(&folder);
    if titled.is_some() {
        info.title = titled.map(str::to_owned); // eur2
    }
    // fanpatch: a mod for another release: its title (a Japanese title
    // folder, exheader.bin, a Japanese layout), for code_region_problem, and
    // a Japanese layout laid out for this build in the cache
    let fanpatch = crate::haiku_fanpatch::inspect(&info.id, &folder, &content, titled.is_some(), info.title.as_deref(), cache);
    let content = match &fanpatch {
        Some((fan, content)) => {
            if let Some(title) = fan.title {
                info.title = Some(format!("{title:016X}"));
            }
            content.clone()
        }
        None => content,
    };
    info.fanpatch = fanpatch.map(|(fan, _)| fan);
    // the game's files: romfs/, romfs_ext/ (a patch or stub stands for the
    // file it changes) and dlc/<IIII>/ for the DLC
    let mut files = Vec::new();
    if let Some(romfs) = find_ci(&content, "romfs") {
        list_files(&romfs, "", &mut files);
    }
    if let Some(ext) = find_ci(&content, "romfs_ext") {
        let mut patched = Vec::new();
        list_files(&ext, "", &mut patched);
        files.extend(patched.into_iter().filter_map(|file| {
            let (target, kind) = file.rsplit_once('.')?;
            ["ips", "bps", "stub"].contains(&kind).then(|| target.to_owned())
        }));
    }
    if let Some(dlc) = find_ci(&content, "dlc") {
        if let Ok(entries) = std::fs::read_dir(&dlc) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.len() == 4 && u16::from_str_radix(&name, 16).is_ok() {
                    if let Some(romfs) = find_ci(&entry.path(), "romfs") {
                        list_files(&romfs, &format!("dlc/{name}/"), &mut files);
                    }
                }
            }
        }
    }
    files.sort();
    files.dedup();
    info.files = files;
    // the texture pack: textures/, or Citra's textures/<title id>/
    if let Some(textures) = find_ci(&content, "textures") {
        // hdpacks: the title id folder when its pictures are there, not
        // only what was left beside the zips (a readme, an ALT- picture)
        let named = |file: &String| {
            let name = file.rsplit('/').next().unwrap_or(file);
            name.starts_with("tex1_") && (name.ends_with(".png") || name.ends_with(".dds"))
        };
        let titled = program_ids().into_iter().find_map(|id| find_ci(&textures, id)).filter(|titled| {
            let mut pictures = Vec::new();
            list_files(titled, "", &mut pictures);
            pictures.iter().any(named)
        });
        let textures = titled.unwrap_or(textures);
        let mut pictures = Vec::new();
        list_files(&textures, "", &mut pictures);
        pictures.retain(named);
        // two packs share a picture when they replace the same texture
        info.pictures = pictures.iter().map(|file| format!("textures:{}", file.rsplit('/').next().unwrap_or(file))).collect();
        if !info.pictures.is_empty() {
            info.textures = Some(textures);
        }
    }
    // hdpacks: a Citra texture pack put in as it is (the pictures in the
    // folder, or under its title id folder, with no textures/ around them)
    if info.textures.is_none() && info.files.is_empty() {
        let mut pictures = Vec::new();
        list_files(&folder, "", &mut pictures);
        pictures.retain(|file| {
            let name = file.rsplit('/').next().unwrap_or(file);
            name.starts_with("tex1_") && name.ends_with(".png")
        });
        if !pictures.is_empty() {
            info.pictures = pictures.iter().map(|file| format!("textures:{}", file.rsplit('/').next().unwrap_or(file))).collect();
            info.textures = Some(folder.clone());
        }
    }
    info.code = zakuro_core::mods::code_files(&content).iter().filter_map(|path| Some(path.file_name()?.to_string_lossy().into_owned())).collect();
    // codemods: the patches and the plugin, which haiku_codemods uses; a
    // plugin's DLL sits in the mod's folder (a Luma layout's content folder
    // or the mod's own)
    let mut files = crate::haiku_codemods::code_files(&content, plugin.as_deref());
    if files.plugin.is_none() && content != folder {
        files.plugin = crate::haiku_codemods::code_files(&folder, plugin.as_deref()).plugin;
    }
    if let Some(dll) = &files.plugin {
        info.code.push(dll.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
    }
    if files.changes_code() || files.plugin.is_some() {
        info.code_files = Some(files);
    }
    // code modules in the RomFS (cro/*.cro, static.crs): Fates has none and
    // never loads one (3dsrecomp finds 0 modules in it), so they can't run
    for file in info.files.iter().filter(|file| !file.starts_with("dlc/") && (file.ends_with(".cro") || file.ends_with(".crs"))) {
        info.code.push(file.rsplit('/').next().unwrap_or(file).to_owned());
        info.modules = true;
    }
    info.content = Some(content);
    crate::haiku_fanpatch::finish(&mut info); // fanpatch: foreign code left out, options
    info
}

/// hdpacks: what the Mods screen says about a download.
fn download_description(download: &crate::haiku_import::Download) -> String {
    let what = if download.texture_pack { "A texture pack" } else { "A mod" };
    let holds = if download.nested > 0 { format!(", {} zips inside", download.nested) } else { String::new() };
    format!("{what} for Citra, downloaded in {} {}{holds}.", download.parts.len(), if download.parts.len() == 1 { "zip" } else { "parts" })
}

/// hdpacks: a size in GB (or MB), as the Mods screen shows it.
fn gigabytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", (bytes >> 20).max(1))
    }
}

/// hdpacks: the cache's unpackings of zips that are not small zip mods any
/// more (taken out, or large ones now unpacked as downloads), which the
/// mod manager used to make at start, gone.
fn prune_cache(cache: &Path, small: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(cache) else { return };
    let live: Vec<String> = small
        .iter()
        .filter_map(|zip| zip.file_stem())
        .map(|stem| stem.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect())
        .collect();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // only the cache's own, <zip>-<size>-<time> with files/ in it
        let mut fields = name.rsplitn(3, '-');
        let (Some(time), Some(size), Some(stem)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let ours = u64::from_str_radix(time, 16).is_ok() && u64::from_str_radix(size, 16).is_ok() && entry.path().join("files").is_dir();
        if ours && !live.iter().any(|live| live == stem) {
            log::info!("mods: the cache's unpacking of {stem} goes, its zip is not a small mod in the folder now");
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// a folder holding only one folder and maybe a readme, as zips often are,
/// stands for that folder.
fn wrapped(folder: &Path) -> PathBuf {
    let mut current = folder.to_path_buf();
    for _ in 0..4 {
        // hdpacks: textures/ too, a texture pack's own folder
        if find_ci(&current, "mod.toml").is_some() || find_ci(&current, "romfs").is_some() || find_ci(&current, "luma").is_some() || find_ci(&current, "textures").is_some() || crate::haiku_fanpatch::stops_unwrap(&current) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else { break };
        let dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        match dirs.as_slice() {
            [only] => current = only.clone(),
            _ => break,
        }
    }
    current
}

/// where a mod's romfs/ and the rest are: a Luma3DS layout's
/// luma/titles/<title id>/, Citra's <title id>/, or the folder itself; and
/// the title id when it was one (eur2: the running game's, else the other
/// build's).
fn content_folder(folder: &Path) -> (PathBuf, Option<&'static str>) {
    for id in program_ids() {
        let luma = find_ci(folder, "luma").and_then(|luma| find_ci(&luma, "titles")).and_then(|titles| find_ci(&titles, id));
        if let Some(luma) = luma {
            return (luma, Some(id));
        }
        if let Some(titles) = find_ci(folder, "titles").and_then(|titles| find_ci(&titles, id)) {
            return (titles, Some(id));
        }
        if let Some(citra) = find_ci(folder, id) {
            return (citra, Some(id));
        }
    }
    (folder.to_path_buf(), None)
}

/// the entry of dir named name, in any case.
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    std::fs::read_dir(dir).ok()?.flatten().find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|entry| entry.path())
}

/// the files under dir, lower case, / separated, after prefix.
fn list_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let path = format!("{prefix}{name}");
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => list_files(&entry.path(), &format!("{path}/"), out),
            // hdpacks: a link or junction to a folder (a pack linked in
            // rather than copied) is followed, not so deep that a link
            // back up goes round for ever
            Ok(kind) if kind.is_symlink() && entry.path().is_dir() => {
                if prefix.matches('/').count() < 16 {
                    list_files(&entry.path(), &format!("{path}/"), out);
                }
            }
            Ok(_) => out.push(path),
            Err(_) => {}
        }
    }
}

/// a zip's files, unpacked once into the cache: a folder named for the zip
/// and its size and time, so a changed zip is unpacked again.
fn unpack(zip: &Path, cache: Option<&Path>) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(zip).map_err(|error| error.to_string())?;
    let modified = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |time| time.as_secs());
    let stem = zip.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let safe: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let cache = cache.map(Path::to_path_buf).unwrap_or_else(|| std::env::temp_dir().join("haiku-mods"));
    let folder = cache.join(format!("{safe}-{:x}-{modified:x}", meta.len()));
    let done = folder.join(".unpacked");
    if done.is_file() {
        return Ok(folder.join("files"));
    }
    let _ = std::fs::remove_dir_all(&folder);
    let bytes = std::fs::read(zip).map_err(|error| error.to_string())?;
    let files = folder.join("files");
    let count = zip_extract(&bytes, &files)?;
    std::fs::write(&done, format!("{count} files from {}\n", zip.display())).map_err(|error| error.to_string())?;
    log::info!("mods: unpacked {} ({count} files) into {}", zip.display(), folder.display());
    // the older unpackings of the same zip go
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path() != folder && name.starts_with(&format!("{safe}-")) && name[safe.len() + 1..].split('-').count() == 2 {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(files)
}

/// a little-endian number in bytes at at, none past the end.
fn le(bytes: &[u8], at: usize, size: usize) -> Option<u64> {
    let slice = bytes.get(at..at + size)?;
    Some(slice.iter().rev().fold(0u64, |value, byte| value << 8 | *byte as u64))
}

/// unpacks a zip's files (stored or deflated) into out, refusing names that
/// would leave it. how many files back.
pub fn zip_extract(bytes: &[u8], out: &Path) -> Result<usize, String> {
    // the end of central directory record, in the last 64 KiB
    let from = bytes.len().saturating_sub(0x10000 + 22);
    let end = (from..bytes.len().saturating_sub(21)).rev().find(|&at| bytes[at..at + 4] == [0x50, 0x4B, 0x05, 0x06]).ok_or("it is not a zip")?;
    let count = le(bytes, end + 10, 2).ok_or("it is not a zip")? as usize;
    let mut at = le(bytes, end + 16, 4).ok_or("it is not a zip")? as usize;
    if count == 0xFFFF || at == 0xFFFF_FFFF {
        return Err("it is a ZIP64 zip, which can't be read; zip it again without ZIP64".into());
    }
    let mut written = 0;
    for _ in 0..count {
        if le(bytes, at, 4) != Some(0x0201_4B50) {
            return Err("its file list is damaged".into());
        }
        let flags = le(bytes, at + 8, 2).unwrap_or(0);
        let method = le(bytes, at + 10, 2).unwrap_or(0);
        let packed = le(bytes, at + 20, 4).unwrap_or(0) as usize;
        let size = le(bytes, at + 24, 4).unwrap_or(0) as usize;
        let name_len = le(bytes, at + 28, 2).unwrap_or(0) as usize;
        let extra_len = le(bytes, at + 30, 2).unwrap_or(0) as usize;
        let comment_len = le(bytes, at + 32, 2).unwrap_or(0) as usize;
        let local = le(bytes, at + 42, 4).unwrap_or(0) as usize;
        let raw_name = bytes.get(at + 46..at + 46 + name_len).ok_or("its file list is damaged")?;
        // bit 11: UTF-8 names; others are code page 437, read as Latin-1
        let name: String = if flags & 0x800 != 0 { String::from_utf8_lossy(raw_name).into_owned() } else { raw_name.iter().map(|&b| b as char).collect() };
        at += 46 + name_len + extra_len + comment_len;
        let name = name.replace('\\', "/");
        if name.ends_with('/') {
            continue;
        }
        let parts: Vec<&str> = name.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
        if parts.is_empty() || parts.iter().any(|part| *part == ".." || part.contains(':')) || name.starts_with('/') {
            return Err(format!("it holds a file named {name}, outside its own folder"));
        }
        if flags & 1 != 0 {
            return Err("it is encrypted".into());
        }
        if le(bytes, local, 4) != Some(0x0403_4B50) {
            return Err("its files are damaged".into());
        }
        let data_at = local + 30 + le(bytes, local + 26, 2).unwrap_or(0) as usize + le(bytes, local + 28, 2).unwrap_or(0) as usize;
        let data = bytes.get(data_at..data_at + packed).ok_or("its files are damaged")?;
        let data = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec(data).map_err(|_| format!("{name} is damaged"))?,
            _ => return Err(format!("{name} is packed in a way that can't be read (method {method}); zip it again with Deflate")),
        };
        if data.len() != size {
            return Err(format!("{name} is damaged"));
        }
        let mut path = out.to_path_buf();
        path.extend(&parts);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&path, data).map_err(|error| error.to_string())?;
        written += 1;
    }
    Ok(written)
}

/// the game's title menu asked for the Mods screen (overrides/mods.c).
static OPEN_ASKED: AtomicBool = AtomicBool::new(false);
/// how many times the title menu got its Mods entry, for the log.
static ENTRIES: AtomicU32 = AtomicU32::new(0);

/// overrides/mods.c's hook: 1 the Mods entry was chosen, 2 it was added to
/// the Main Menu.
///
/// # Safety
///
/// called by the recompiled code with its context, which is not used.
pub unsafe extern "C" fn title_hook(_ctx: *mut std::ffi::c_void, what: u32) {
    match what {
        1 => {
            log::info!("mods: the Main Menu's Mods entry was chosen");
            OPEN_ASKED.store(true, Ordering::Relaxed);
        }
        2 => {
            if ENTRIES.fetch_add(1, Ordering::Relaxed) == 0 {
                log::info!("mods: the Main Menu has its Mods entry");
            }
        }
        _ => {}
    }
}

/// whether the title menu asked for the Mods screen since the last call.
pub fn take_open_request() -> bool {
    OPEN_ASKED.swap(false, Ordering::Relaxed)
}

/// fanpatch: the tests' zips, for haiku_fanpatch's tests.
/// jpprep: the Japanese build's answers (the running title is the USA one
/// in tests: the same title accepted, another Japanese edition, no title,
/// another game refused)
#[cfg(test)]
#[test]
fn a_japanese_build_takes_code_made_for_it() {
    assert_eq!(jpn_code_problem(Some(zakuro_core::region::USA)), None);
    assert!(jpn_code_problem(Some(zakuro_core::region::JPN)).unwrap().contains("another Japanese edition"));
    assert!(jpn_code_problem(None).unwrap().contains("doesn't say"));
    assert!(jpn_code_problem(Some(0x0004_0000_0000_1000)).unwrap().contains("another game"));
}

#[cfg(test)]
pub(crate) fn tests_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    tests::zip(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("haiku-mods-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, path: &str, data: &[u8]) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    /// a zip of stored files, as `zip -0` makes.
    pub(crate) fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
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
                h.extend_from_slice(&0u32.to_le_bytes()); // crc, not checked
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
            // comment length, disk, internal and external attributes
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

    #[test]
    fn mods_are_found_ordered_and_checked() {
        let root = folder("list");
        let mods = root.join("mods");
        put(&mods, "b-text/mod.toml", b"id = \"text\"\nname = \"Better Text\"\nversion = \"1.2\"\nauthor = \"Someone\"\ndescription = \"Words.\"\n");
        put(&mods, "b-text/romfs/m/@E/Title.bin.lz", b"x");
        put(&mods, "a-luma/luma/titles/0004000000179800/romfs/m/@e/title.bin.lz", b"y");
        put(&mods, "a-luma/luma/titles/0004000000179800/romfs/face/x.bin", b"y");
        put(&mods, "c-code/romfs/a.bin", b"z");
        put(&mods, "c-code/code.ips", b"PATCHEOF");
        put(&mods, "f-cro/romfs/cro/Battle.cro", b"cro");
        put(&mods, "d-tex/mod.toml", b"name = \"Textures\"\ndependencies = [\"text\"]\n");
        put(&mods, "d-tex/textures/tex1_32x32_0123456789ABCDEF_4_mip0.png", b"png");
        std::fs::write(mods.join("e-zip.zip"), zip(&[("Wrapped/romfs/m/@E/Title.bin.lz", b"z"), ("Wrapped/readme.txt", b"hi")])).unwrap();
        let data = root.join("userdata");
        let mut list = ModList::load(&root, Some(&data), Some(&root.join("cache")));
        let ids: Vec<&str> = list.mods.iter().map(|info| info.id.as_str()).collect();
        assert_eq!(ids, ["a-luma", "text", "c-code", "d-tex", "e-zip", "f-cro"]);
        assert_eq!(list.mods[5].code, ["battle.cro"], "a code module in the RomFS is code");
        assert!(!list.can_enable(5));
        assert!(list.enabled.iter().all(|on| !on), "new mods start off");
        assert_eq!(list.mods[0].files, ["face/x.bin", "m/@e/title.bin.lz"]);
        assert_eq!(list.mods[1].name, "Better Text");
        assert_eq!(list.mods[1].version, "1.2");
        assert_eq!(list.mods[2].code, ["code.ips"]);
        assert!(list.can_enable(2), "codemods: a mod that changes the code is a code mod");
        assert!(list.notes[2][0].text.starts_with("Code mod"));
        assert_eq!(list.mods[4].files, ["m/@e/title.bin.lz"], "a zip's wrapping folder is looked through");
        assert!(list.mods[4].content.as_ref().unwrap().starts_with(root.join("cache")));

        for index in [0, 1, 3, 4] {
            assert!(list.set_enabled(index, true));
        }
        assert!(!list.set_enabled(5, true), "code modules can't run");
        // the three that share Title.bin.lz
        assert_eq!(list.notes[1][0].text, "Replaces 1 file of a-luma, which loads before it.");
        assert!(list.notes[1].iter().any(|note| note.text == "1 of its files is replaced by e-zip, which loads after it."));
        // d-tex needs text, which loads before it: fine
        assert!(list.notes[3].is_empty(), "{:?}", list.notes[3]);
        list.shift(3, -2);
        assert_eq!(list.mods[1].id, "d-tex");
        assert!(list.notes[1].iter().any(|note| note.text.contains("to load before it")));
        assert_eq!(list.content_dirs().len(), 3);
        assert_eq!(list.texture_dirs().len(), 1);

        // a mod taken out of the folder for a while keeps its place
        std::fs::rename(mods.join("b-text"), root.join("b-text-away")).unwrap();
        let away = ModList::load(&root, Some(&data), Some(&root.join("cache")));
        assert!(!away.active().contains(&"text".to_owned()));
        std::fs::rename(root.join("b-text-away"), mods.join("b-text")).unwrap();
        // the list is kept, in order, and read back
        let again = ModList::load(&root, Some(&data), Some(&root.join("cache")));
        assert_eq!(again.active(), list.active());
        assert_eq!(again.active(), ["a-luma", "d-tex", "text", "e-zip"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// hdpacks: texture packs as Citra's players have them: the pictures
    /// under the title id folder, in a folder of their own, or in
    /// textures/ beside a title id folder holding only the readme; and the
    /// downloads put away are not mods.
    #[test]
    fn citra_texture_packs_are_found_as_they_are() {
        let root = folder("citra");
        let mods = root.join("mods");
        put(&mods, "a-citra/0004000000179800/UI/tex1_8x8_00000000000000AA_13.png", b"png");
        put(&mods, "b-floors/Map Floors/tex1_8x8_00000000000000BB_12.png", b"png");
        put(&mods, "c-tex/textures/0004000000179800/README - C.txt", b"hi");
        put(&mods, "c-tex/textures/0004000000179800/ALT-tex1_8x8_00000000000000CC_13.png", b"png");
        put(&mods, "c-tex/textures/UI/tex1_8x8_00000000000000CC_13.png", b"png");
        put(&mods, "_downloads/old/romfs/a.bin", b"x");
        put(&mods, ".unpacking-x/textures/tex1_8x8_00000000000000DD_13.png", b"png");
        let list = ModList::load(&root, Some(&root.join("userdata")), Some(&root.join("cache")));
        let ids: Vec<&str> = list.mods.iter().map(|info| info.id.as_str()).collect();
        assert_eq!(ids, ["a-citra", "b-floors", "c-tex"]);
        for index in 0..3 {
            assert!(list.can_enable(index), "{}", list.mods[index].id);
            assert_eq!(list.mods[index].pictures.len(), 1, "{}", list.mods[index].id);
        }
        assert!(list.mods[2].textures.as_ref().unwrap().ends_with("textures"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zips_that_leave_their_folder_are_refused() {
        let out = folder("slip");
        assert!(zip_extract(&zip(&[("../evil.txt", b"no")]), &out).is_err());
        assert!(zip_extract(&zip(&[("ok/../../evil.txt", b"no")]), &out).is_err());
        assert_eq!(zip_extract(&zip(&[("ok/fine.txt", b"yes")]), &out), Ok(1));
        assert_eq!(std::fs::read(out.join("ok/fine.txt")).unwrap(), b"yes");
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn online_gameplay_files_are_told_from_looks() {
        for path in ["gamedata/gamedata.bin.lz", "gamedata/person/a001.bin.lz", "gamedata/dispos/a001.bin.lz", "scripts/a001.cmb", "castle/castle_join.bin.lz", "params.bin.lz", "dlc/0001/gamedata/person/x.bin.lz", "something/new.bin"] {
            assert!(gameplay_file(path), "{path}");
        }
        for path in ["sound/a.bcstm", "face/a.arc", "m/@e/gamedata.bin.lz", "unit/head/a.bch.lz", "ui/x.arc.lz", "font/a.bcfnt", "castle/obj.bch.lz", "texturecoordinate.bin", "dlc/0001/face/x.arc"] {
            assert!(!gameplay_file(path), "{path}");
        }
    }

    #[test]
    fn online_stamps_list_gameplay_mods_only() {
        let root = folder("online");
        let data = root.join("userdata");
        put(&root, "mods/rules/mod.toml", b"name = \"Rules\"
");
        put(&root, "mods/rules/romfs_ext/GameData/GameData.bin.lz.ips", b"PATCHEOF");
        put(&root, "mods/looks/mod.toml", b"name = \"Looks\"
");
        put(&root, "mods/looks/romfs/face/a.arc", b"x");
        put(&root, "mods/said/mod.toml", b"name = \"Said\"
gameplay = true
");
        put(&root, "mods/said/romfs/sound/a.bcstm", b"y");
        put(&root, "mods/pack/textures/tex1_4x4_0000000000000000_0_mip0.png", b"z");
        put(&data, "mods.toml", b"[[mod]]
id = \"rules\"
enabled = true
[[mod]]
id = \"looks\"
enabled = true
[[mod]]
id = \"said\"
enabled = true
[[mod]]
id = \"pack\"
enabled = true
");
        let list = ModList::load(&root, Some(&data), Some(&root.join("cache")));
        let stamps = list.online_stamps();
        let ids: Vec<&str> = stamps.iter().map(|stamp| stamp.id.as_str()).collect();
        assert_eq!(ids, ["rules", "said"]);
        // another version of a file is another hash
        put(&root, "mods/rules/romfs_ext/GameData/GameData.bin.lz.ips", b"PATCH     EOF");
        let again = ModList::load(&root, Some(&data), Some(&root.join("cache"))).online_stamps();
        assert_ne!(again[0].hash, stamps[0].hash);
        assert_eq!(again[1].hash, stamps[1].hash);
        let diff = zakuro_core::netplay::ModsDiff::of(&stamps, &again[1..]);
        assert_eq!(diff.ours, ["Rules"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
