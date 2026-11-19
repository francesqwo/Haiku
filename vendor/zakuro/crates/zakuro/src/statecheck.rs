//! statecheck: a save state remembers the file layer it was made with, and
//! is refused when loaded over another.
//!
//! A state holds the game's memory: its open RomFS files with their sizes
//! and offsets, file data it cached, message archives it already parsed.
//! Loaded over another file layer, the game reads files that no longer
//! match (eur2 measured hundreds of thousands of accesses to unmapped
//! addresses, and crashes; fanpatch saw crashes across turning Team If on
//! or off). So at each start Haiku writes down what the layer is made of
//! (a [`Setup`]) and every state saved from then on carries it:
//!
//! - in the `.hstate` slots' header (`states::Meta::setup`), so the Save
//!   States page can check a slot without reading its 25 MB;
//! - in the console's state itself, as a last section
//!   (`zakuro_core::state::set_tag`), so the raw states (FATES_SAVE_AT,
//!   FATES_LOAD, the old F5/F8, the bottom-check loader) have it too.
//!
//! What goes in, each part with a digest and the name the player knows it
//! by (`parts`, lowest layer first):
//!
//! - Haiku's own game files (`assets/builtin`);
//! - the Japanese voices, and the DLC's Japanese voices;
//! - each mod laid over the game: its id, name, and a digest of its files'
//!   paths and sizes (romfs/, romfs_ext/, dlc/), and of its code patches
//!   and plugin with their options when its code runs;
//! - the layer of Haiku's text merged into the mods' (fanpatch);
//! - the console language the game is told (Game Language on EUR).
//!
//! And `files`, the ground truth under them all: the layout of the RomFS
//! the console reads, for the game, its update and each DLC
//! (`zakuro_core::state::file_layout`: every file's name, offset and size).
//! Whatever the parts say, a state is refused when that differs; the parts
//! only say why.
//!
//! **Digests are cheap.** File sizes and paths, not contents: a size is
//! what moves the files after it, which is what breaks a state; a file
//! changed in place at the same size keeps every offset (the state may show
//! the old text until the game reads it again, nothing worse). Walking the
//! layers' folders takes a few milliseconds at start; the layout digest
//! hashes the RomFS metadata tables (about 1 MB) once. Nothing is hashed at
//! save time. Sizes rather than modification times, so a zip mod unpacked
//! again into the cache, or a Haiku folder copied elsewhere, still matches.
//!
//! **What never blocks** (`looks`, recorded for the log only):
//!
//! - texture packs and HD packs (a mod's `textures/`, Haiku's builtin
//!   `textures/`): the renderer swaps pictures as it uploads them, by the
//!   hash of the game's texture; nothing of them is in the game's memory
//!   or its files. A mod whose `romfs/` replaces pictures does change the
//!   files, and counts like any other mod;
//! - every setting that doesn't reach the game's files: render scale,
//!   aspect, frame rate, the bottom screen, controls, audio volumes,
//!   the cheats (their marks live in the units, in the state).
//!
//! **The code-mod build key is not a part.** It hashes the patched code
//! with Haiku's own recompiled code, so it changes with every Haiku build
//! even when the mods are the same; what the game's code is made of is the
//! mods' patch files, which are in their parts.
//!
//! A state with no setup (every state made before this) loads as before,
//! with a log line and a notice. `HAIKU_STATE_CHECK=warn` loads a state
//! that differs anyway, saying what differs (tests, and the bench's states
//! when the assets they were made with changed).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// what a setup holds changes from one format to the next.
pub const FORMAT: u32 = 1;

/// the notice for a state made before Haiku checked.
pub const UNCHECKED: &str = "Made before Haiku checked mods; if anything looks wrong, reload a regular save.";

/// what the console was started with: the file layer under the game.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Setup {
    pub format: u32,
    /// the layout of the RomFS the console reads: (game, update, dlc ...,
    /// a digest in hex)
    pub files: Vec<(String, String)>,
    /// what the layer is made of, lowest first
    pub parts: Vec<Part>,
    /// texture packs: recorded, never compared
    #[serde(default)]
    pub looks: Vec<Part>,
}

/// a part of the layer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Part {
    /// builtin, voices, voices-dlc, mod, text, language, files
    pub kind: String,
    pub id: String,
    /// as the player knows it
    pub name: String,
    pub digest: String,
}

/// how a state's setup compares with the running one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// the same layer: it loads
    Same,
    /// a state made before Haiku checked: it loads, with a notice
    Unchecked,
    /// another layer: it is refused, saying what changed
    Differs(Vec<Change>),
}

/// one thing that differs, from the state's setup to the running one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// a mod (or a folder of files) on in the state, off now
    Off(String),
    /// off in the state, on now
    On(String),
    /// on in both, with other files
    Changed(String),
    /// the same mods, in another order
    Order,
    /// the voices: whether the state's were Japanese
    Voices(bool),
    /// another Japanese voice layer installed
    VoicesChanged,
    /// the DLC's Japanese voices: whether the state had them
    VoicesDlc(bool),
    /// Haiku's own files: whether the state had them at all
    Builtin(bool),
    /// Haiku's text merged into the mods'
    Text,
    /// the console language: the state's, now
    Language(String, String),
    /// the game's files themselves, by what changed (game, update, DLC)
    Files(Vec<String>),
    /// a setup this build can't read (a newer Haiku's)
    Unknown,
}

static CURRENT: Mutex<Option<Setup>> = Mutex::new(None);

/// the running game's setup, when one was worked out.
pub fn current() -> Option<Setup> {
    CURRENT.lock().ok().and_then(|current| current.clone())
}

fn hex(value: u64) -> String {
    format!("{value:016x}")
}

/// FNV-1a, with a separator after each field.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, bytes: &[u8]) {
        for byte in bytes.iter().copied().chain([0xFF]) {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

/// the files a content folder lays over the game, by path and size:
/// romfs/, romfs_ext/ and dlc/ (any case), all the way down.
fn folder_digest(dir: &Path, hash: &mut Fnv) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut tops: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| (entry.file_name().to_string_lossy().to_lowercase(), entry.path()))
        .filter(|(name, path)| matches!(name.as_str(), "romfs" | "romfs_ext" | "dlc") && path.is_dir())
        .collect();
    tops.sort();
    for (name, path) in tops {
        count += walk(&path, &name, hash);
    }
    count
}

fn walk(dir: &Path, prefix: &str, hash: &mut Fnv) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut list: Vec<(String, PathBuf, Option<std::fs::Metadata>)> =
        entries.flatten().map(|entry| (entry.file_name().to_string_lossy().to_lowercase(), entry.path(), entry.metadata().ok())).collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    let mut count = 0;
    for (name, path, meta) in list {
        let relative = format!("{prefix}/{name}");
        // a junction or link is followed as the file layer follows it
        let meta = match meta {
            Some(meta) if !meta.file_type().is_symlink() => Some(meta),
            _ => std::fs::metadata(&path).ok(),
        };
        let Some(meta) = meta else { continue };
        if meta.is_dir() {
            count += walk(&path, &relative, hash);
        } else {
            hash.add(relative.as_bytes());
            hash.add(&meta.len().to_le_bytes());
            count += 1;
        }
    }
    count
}

/// a code file's contents (patches are small), or a plugin's size.
fn code_digest(info: &crate::haiku_mods::ModInfo, options: &std::collections::BTreeMap<String, i64>, hash: &mut Fnv) {
    let Some(files) = &info.code_files else { return };
    let name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    for path in files.code.iter().chain(files.patches.iter()).chain(files.exheader.iter()) {
        hash.add(name(path).as_bytes());
        hash.add(&std::fs::read(path).unwrap_or_default());
    }
    if let Some(plugin) = &files.plugin {
        hash.add(name(plugin).as_bytes());
        hash.add(&std::fs::metadata(plugin).map_or(0, |meta| meta.len()).to_le_bytes());
        for (key, value) in options {
            hash.add(format!("{key}={value}").as_bytes());
        }
    }
}

/// works out the running game's setup, after the loader laid its files,
/// and has every state saved from now on carry it. `layers` are the
/// content folders handed to the loader (Config::mods), lowest first.
pub fn started(system: &zakuro_core::System, layers: &[PathBuf], mods: &crate::haiku_mods::ModList, game: &Path, language: crate::settings::Language) {
    let started = std::time::Instant::now();
    let builtin = crate::haiku_builtin::content_dirs();
    let voices = crate::haiku_voices::installed(Some(game));
    let voices_dlc = voices.as_deref().and_then(crate::haiku_voices::installed_dlc);
    // each mod on, with the folders it lays (its own and its extras)
    let on: Vec<usize> = (0..mods.mods.len()).filter(|&index| mods.enabled.get(index).copied().unwrap_or(false) && mods.can_enable(index) && !mods.left_out.contains(&mods.mods[index].id)).collect();
    let owner = |dir: &PathBuf| on.iter().copied().find(|&index| crate::haiku_fanpatch::content_dirs(&mods.mods[index], &mods.options_of(index)).contains(dir));
    let mut parts: Vec<Part> = Vec::new();
    let mut hashes: Vec<Fnv> = Vec::new();
    let mut files = 0;
    let add = |parts: &mut Vec<Part>, hashes: &mut Vec<Fnv>, kind: &str, id: &str, name: &str| -> usize {
        if let Some(at) = parts.iter().position(|part| part.kind == kind && part.id == id) {
            return at;
        }
        parts.push(Part { kind: kind.to_owned(), id: id.to_owned(), name: name.to_owned(), digest: String::new() });
        hashes.push(Fnv::new());
        parts.len() - 1
    };
    for dir in layers {
        let at = if builtin.contains(dir) {
            add(&mut parts, &mut hashes, "builtin", "builtin", "Haiku's own game files")
        } else if voices.as_ref() == Some(dir) {
            add(&mut parts, &mut hashes, "voices", "ja", "Japanese voices")
        } else if voices_dlc.as_ref() == Some(dir) {
            add(&mut parts, &mut hashes, "voices-dlc", "ja-dlc", "the DLC's Japanese voices")
        } else if dir.join("merged.txt").is_file() {
            // fanpatch: Haiku's text merged into the mods' (its cache folder)
            add(&mut parts, &mut hashes, "text", "merged", "Haiku's text in the mods")
        } else if let Some(index) = owner(dir) {
            let info = &mods.mods[index];
            add(&mut parts, &mut hashes, "mod", &info.id.to_lowercase(), &info.name)
        } else {
            let name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            add(&mut parts, &mut hashes, "files", &name.to_lowercase(), &name)
        };
        files += folder_digest(dir, &mut hashes[at]);
    }
    // code mods: their patches and plugins, also those with no files
    for &index in &on {
        if !mods.uses_code(index) {
            continue;
        }
        let info = &mods.mods[index];
        let at = add(&mut parts, &mut hashes, "mod", &info.id.to_lowercase(), &info.name);
        code_digest(info, &mods.options_of(index), &mut hashes[at]);
    }
    for (part, hash) in parts.iter_mut().zip(&hashes) {
        part.digest = hex(hash.0);
    }
    // versions2: the language only where it is a choice, the European game;
    // the US game's text is English and the Japanese game is told Japanese
    // whatever the setting said (a state from before each version had its
    // own language carries the shared one, which a part left out here never
    // compares against)
    if zakuro_core::region::is_eur() {
        let mut language_hash = Fnv::new();
        language_hash.add(&[language.code()]);
        parts.push(Part { kind: "language".into(), id: "language".into(), name: language.name().to_owned(), digest: hex(language_hash.0) });
    }
    // what never blocks: the texture packs
    let looks: Vec<Part> = on
        .iter()
        .filter_map(|&index| {
            let info = &mods.mods[index];
            info.textures.as_ref().map(|_| Part { kind: "textures".into(), id: info.id.to_lowercase(), name: info.name.clone(), digest: String::new() })
        })
        .collect();
    let layout: Vec<(String, String)> = zakuro_core::state::file_layout(system).into_iter().map(|(what, digest)| (what, hex(digest))).collect();
    let setup = Setup { format: FORMAT, files: layout, parts, looks };
    let tag = serde_json::to_vec(&setup).unwrap_or_default();
    zakuro_core::state::set_tag(tag);
    log::info!(
        "statecheck: states made now carry this setup ({} files walked in {} ms): {}",
        files,
        started.elapsed().as_millis(),
        describe(&setup)
    );
    if let Ok(mut current) = CURRENT.lock() {
        *current = Some(setup);
    }
    // pulse: Divine Pulse's moments are of the console before this start
    // (a restart in place: the Mods screen's Apply in a headless run, or
    // HAIKU_RESTART=in-process; a window starts a new process instead)
    let kept = crate::pulse::moments_kept();
    if kept > 0 {
        log::info!("statecheck: a new start: Divine Pulse's {kept} moments of the console before are let go");
        println!("statecheck: a new start: Divine Pulse's {kept} moments of the console before are let go");
    }
    crate::pulse::state_loaded();
}

/// a setup in one line, for the log.
pub fn describe(setup: &Setup) -> String {
    let parts: Vec<String> = setup.parts.iter().map(|part| format!("{} {} [{}]", part.kind, part.name, &part.digest[part.digest.len().saturating_sub(8)..])).collect();
    let files: Vec<String> = setup.files.iter().map(|(what, digest)| format!("{what} {}", &digest[digest.len().saturating_sub(8)..])).collect();
    let looks: Vec<String> = setup.looks.iter().map(|part| part.name.clone()).collect();
    format!("files: {}; layer: {}; texture packs (never checked): {}", files.join(", "), parts.join(", "), if looks.is_empty() { "none".to_owned() } else { looks.join(", ") })
}

/// whether a state that differs is loaded anyway (HAIKU_STATE_CHECK=warn
/// or off: tests, and states made with assets that have changed since).
pub fn warn_only() -> bool {
    std::env::var("HAIKU_STATE_CHECK").is_ok_and(|value| matches!(value.as_str(), "warn" | "off" | "0"))
}

/// a state's setup against the running one. With no running setup (no
/// game started through the app) everything is the same.
pub fn verdict(saved: Option<&Setup>) -> Verdict {
    let Some(now) = current() else { return Verdict::Same };
    match saved {
        None => Verdict::Unchecked,
        Some(saved) if saved.format > FORMAT => Verdict::Differs(vec![Change::Unknown]),
        Some(saved) => {
            let changes = changes(saved, &now);
            if changes.is_empty() {
                Verdict::Same
            } else {
                Verdict::Differs(changes)
            }
        }
    }
}

/// a raw state's setup: from its tag section.
pub fn verdict_of_state(bytes: &[u8]) -> Verdict {
    match zakuro_core::state::tag(bytes) {
        Ok(None) => verdict(None),
        Ok(Some(tag)) => match serde_json::from_slice::<Setup>(&tag) {
            Ok(setup) => verdict(Some(&setup)),
            Err(_) if current().is_none() => Verdict::Same,
            Err(_) => Verdict::Differs(vec![Change::Unknown]),
        },
        // not a state, or damaged: load_state says so
        Err(_) => Verdict::Same,
    }
}

/// a raw state (FATES_LOAD, the old F5/F8, the bottom-check loader) loaded
/// after its setup is checked: refused with the message when it differs
/// (unless HAIKU_STATE_CHECK=warn), loaded with a log line when it has
/// none. Whether it had none comes back.
pub fn load_raw(system: &mut zakuro_core::System, bytes: &[u8], what: &str) -> Result<bool, String> {
    let verdict = verdict_of_state(bytes);
    match &verdict {
        Verdict::Differs(changes) => {
            let text = message(changes);
            if !warn_only() {
                log::warn!("statecheck: {what} refused: {text}");
                return Err(text);
            }
            log::warn!("statecheck: {what} differs, loaded anyway (HAIKU_STATE_CHECK): {text}");
            println!("statecheck: {what} differs, loaded anyway (HAIKU_STATE_CHECK): {text}");
        }
        Verdict::Unchecked => {
            log::info!("statecheck: {what} was made before Haiku checked the file layer (it has no setup); loaded as before");
            println!("statecheck: {what} was made before Haiku checked the file layer (it has no setup); loaded as before");
        }
        Verdict::Same => {}
    }
    system.load_state(bytes)?;
    crate::states::verify_loaded(system, bytes, what); // pulsefix: tests
    Ok(verdict == Verdict::Unchecked)
}

/// what differs from saved to now, the most telling first.
pub fn changes(saved: &Setup, now: &Setup) -> Vec<Change> {
    let mut out = Vec::new();
    let find = |setup: &Setup, kind: &str, id: &str| setup.parts.iter().find(|part| part.kind == kind && part.id == id).cloned();
    let of_kind = |setup: &Setup, kind: &str| -> Vec<Part> { setup.parts.iter().filter(|part| part.kind == kind).cloned().collect() };
    // the voices
    match (find(saved, "voices", "ja"), find(now, "voices", "ja")) {
        (Some(_), None) => out.push(Change::Voices(true)),
        (None, Some(_)) => out.push(Change::Voices(false)),
        (Some(a), Some(b)) if a.digest != b.digest => out.push(Change::VoicesChanged),
        _ => {}
    }
    let voices_both = find(saved, "voices", "ja").is_some() && find(now, "voices", "ja").is_some();
    match (find(saved, "voices-dlc", "ja-dlc"), find(now, "voices-dlc", "ja-dlc")) {
        (Some(_), None) if voices_both => out.push(Change::VoicesDlc(true)),
        (None, Some(_)) if voices_both => out.push(Change::VoicesDlc(false)),
        (Some(a), Some(b)) if a.digest != b.digest => out.push(Change::VoicesChanged),
        _ => {}
    }
    // the mods (and folders of files), by id
    for kind in ["mod", "files"] {
        let (before, after) = (of_kind(saved, kind), of_kind(now, kind));
        for part in &before {
            match after.iter().find(|other| other.id == part.id) {
                None => out.push(Change::Off(part.name.clone())),
                Some(other) if other.digest != part.digest => out.push(Change::Changed(other.name.clone())),
                _ => {}
            }
        }
        for part in &after {
            if !before.iter().any(|other| other.id == part.id) {
                out.push(Change::On(part.name.clone()));
            }
        }
    }
    // the language
    match (find(saved, "language", "language"), find(now, "language", "language")) {
        (Some(a), Some(b)) if a.digest != b.digest => out.push(Change::Language(a.name, b.name)),
        _ => {}
    }
    // Haiku's own files
    match (find(saved, "builtin", "builtin"), find(now, "builtin", "builtin")) {
        (Some(_), None) => out.push(Change::Builtin(true)),
        (None, Some(_)) => out.push(Change::Builtin(false)),
        (Some(a), Some(b)) if a.digest != b.digest => out.push(Change::Builtin(true)),
        _ => {}
    }
    if !out.is_empty() {
        return out;
    }
    // the same parts: in another order?
    let order = |setup: &Setup| -> Vec<(String, String)> { setup.parts.iter().filter(|part| part.kind == "mod" || part.kind == "files").map(|part| (part.kind.clone(), part.id.clone())).collect() };
    if order(saved) != order(now) {
        return vec![Change::Order];
    }
    // the merged text follows from the rest; it differs alone when Haiku's
    // way of merging changed
    if find(saved, "text", "merged").map(|part| part.digest) != find(now, "text", "merged").map(|part| part.digest) {
        return vec![Change::Text];
    }
    // and the files as the console reads them, the ground truth
    if saved.files != now.files {
        let mut which: Vec<String> = Vec::new();
        for (what, digest) in &saved.files {
            if now.files.iter().find(|(other, _)| other == what).is_none_or(|(_, other)| other != digest) {
                which.push(what.clone());
            }
        }
        for (what, _) in &now.files {
            if !saved.files.iter().any(|(other, _)| other == what) {
                which.push(what.clone());
            }
        }
        return vec![Change::Files(which)];
    }
    Vec::new()
}

fn language_row() -> &'static str {
    if zakuro_core::region::is_eur() {
        "Game Language"
    } else {
        "Language"
    }
}

fn files_what(which: &[String]) -> String {
    let game = which.iter().any(|what| what == "game");
    let update = which.iter().any(|what| what == "update");
    let dlc = which.iter().any(|what| what.starts_with("dlc"));
    let list: Vec<&str> = [(game, "the game's"), (update, "the update's"), (dlc, "the DLC's")].into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
    match list.as_slice() {
        [] => "the game's files were different".to_owned(),
        [one] => format!("{one} files were different"),
        [first @ .., last] => format!("{} and {last} files were different", first.join(", ")),
    }
}

impl Change {
    /// a few words, for a list of several and the slot's card.
    pub fn phrase(&self) -> String {
        match self {
            Change::Off(name) => format!("{name} was on, now off"),
            Change::On(name) => format!("{name} was off, now on"),
            Change::Changed(name) => format!("{name} has changed"),
            Change::Order => "the mods were in another order".to_owned(),
            Change::Voices(true) => "the voices were Japanese, now English".to_owned(),
            Change::Voices(false) => "the voices were English, now Japanese".to_owned(),
            Change::VoicesChanged => "other Japanese voices were installed".to_owned(),
            Change::VoicesDlc(true) => "the DLC's Japanese voices were in, now out".to_owned(),
            Change::VoicesDlc(false) => "the DLC's Japanese voices were out, now in".to_owned(),
            Change::Builtin(_) => "Haiku's own game files were different".to_owned(),
            Change::Text => "Haiku's text was merged into the mods' differently".to_owned(),
            Change::Language(was, now) => format!("the game was in {was}, now {now}"),
            Change::Files(which) => files_what(which),
            Change::Unknown => "it was made by a newer Haiku".to_owned(),
        }
    }

    /// the whole message, when it is the only change.
    pub fn sentence(&self) -> String {
        match self {
            Change::Off(name) => format!("This state was saved with different mods: {name} was on, now off. Turn it back on to load it."),
            Change::On(name) => format!("This state was saved with different mods: {name} was off, now on. Turn it off to load it."),
            Change::Changed(name) => format!("This state was saved with another version of {name}. Put that version back to load it."),
            Change::Order => "This state was saved with the mods in another order. Put them back in that order to load it.".to_owned(),
            Change::Voices(true) => "This state was saved with Japanese voices. Set Voices to Japanese (Options > Audio) to load it.".to_owned(),
            Change::Voices(false) => "This state was saved with English voices. Set Voices to English (Options > Audio) to load it.".to_owned(),
            Change::VoicesChanged => "This state was saved with other Japanese voices installed, so it can't be loaded with these.".to_owned(),
            Change::VoicesDlc(true) => "This state was saved with the DLC's Japanese voices. Add them again with Haiku Setup to load it.".to_owned(),
            Change::VoicesDlc(false) => "This state was saved before the DLC's Japanese voices were added, so it can't be loaded with them.".to_owned(),
            Change::Builtin(_) => "This state was saved by another version of Haiku, whose own game files were different, so this one can't load it.".to_owned(),
            Change::Text => "This state was saved by another version of Haiku, which merged its text into the mods' differently, so this one can't load it.".to_owned(),
            Change::Language(was, _) => format!("This state was saved with the game in {was}. Set {} to {was} to load it.", language_row()),
            Change::Files(which) => {
                let what = files_what(which);
                format!("This state was saved when {what}, so it can't be loaded safely.")
            }
            Change::Unknown => "This state was saved by a newer version of Haiku. Load it with that version.".to_owned(),
        }
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// why a state is refused, naming up to three changes.
pub fn message(changes: &[Change]) -> String {
    match changes {
        [] => String::new(),
        [one] => one.sentence(),
        _ => {
            let shown: Vec<String> = changes.iter().take(3).map(Change::phrase).collect();
            let more = changes.len().saturating_sub(3);
            let list = match shown.as_slice() {
                [a, b] => format!("{a}, and {b}"),
                [a, b, c] => format!("{a}; {b}; {c}"),
                _ => shown.join("; "),
            };
            let more = if more > 0 { format!(" ({more} more)") } else { String::new() };
            format!("This state was saved with a different setup: {list}{more}. Put them back as they were to load it.")
        }
    }
}

/// the slot card's short reason.
pub fn short(changes: &[Change]) -> String {
    let first = changes.first().map(|change| capital(&change.phrase())).unwrap_or_default();
    match changes.len() {
        0 | 1 => first,
        n => format!("{first} (+{} more)", n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(kind: &str, id: &str, name: &str, digest: &str) -> Part {
        Part { kind: kind.into(), id: id.into(), name: name.into(), digest: digest.into() }
    }

    fn setup(parts: Vec<Part>) -> Setup {
        Setup { format: FORMAT, files: vec![("game".into(), "1".into())], parts, looks: Vec::new() }
    }

    #[test]
    fn the_same_setup_matches_and_texture_packs_never_count() {
        let a = setup(vec![part("builtin", "builtin", "Haiku's own game files", "b"), part("mod", "teamif", "Team If", "t"), part("language", "language", "English", "e")]);
        let mut b = a.clone();
        b.looks.push(part("textures", "hd", "HD Pack", ""));
        assert!(changes(&a, &b).is_empty());
    }

    #[test]
    fn a_mod_turned_off_or_on_is_named() {
        let with = setup(vec![part("builtin", "builtin", "B", "b"), part("mod", "teamif", "Team If translation", "t")]);
        let without = setup(vec![part("builtin", "builtin", "B", "b")]);
        let off = changes(&with, &without);
        assert_eq!(off, vec![Change::Off("Team If translation".into())]);
        assert_eq!(message(&off), "This state was saved with different mods: Team If translation was on, now off. Turn it back on to load it.");
        assert_eq!(changes(&without, &with), vec![Change::On("Team If translation".into())]);
    }

    #[test]
    fn voices_and_several_changes() {
        let ja = setup(vec![part("voices", "ja", "Japanese voices", "v"), part("mod", "a", "A", "1"), part("language", "language", "French", "f")]);
        let en = setup(vec![part("mod", "b", "B", "2"), part("language", "language", "German", "g")]);
        assert_eq!(message(&changes(&ja, &setup(vec![part("mod", "a", "A", "1"), part("language", "language", "French", "f")]))), "This state was saved with Japanese voices. Set Voices to Japanese (Options > Audio) to load it.");
        let all = changes(&ja, &en);
        assert_eq!(all.len(), 4);
        let text = message(&all);
        assert!(text.starts_with("This state was saved with a different setup: the voices were Japanese, now English; A was on, now off; B was off, now on (1 more)."), "{text}");
        assert_eq!(short(&all), "The voices were Japanese, now English (+3 more)");
    }

    #[test]
    fn order_text_and_files_only_when_nothing_else() {
        let a = setup(vec![part("mod", "a", "A", "1"), part("mod", "b", "B", "2")]);
        let b = setup(vec![part("mod", "b", "B", "2"), part("mod", "a", "A", "1")]);
        assert_eq!(changes(&a, &b), vec![Change::Order]);
        let mut c = a.clone();
        c.files = vec![("game".into(), "2".into()), ("dlc 0004008C00179800".into(), "3".into())];
        assert_eq!(changes(&a, &c), vec![Change::Files(vec!["game".into(), "dlc 0004008C00179800".into()])]);
        assert_eq!(message(&changes(&a, &c)), "This state was saved when the game's and the DLC's files were different, so it can't be loaded safely.");
    }
}
