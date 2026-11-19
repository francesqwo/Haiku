//! jpvoice: the Japanese voices, a layer of the game's own sound files
//! from a Japanese dump, laid over the RomFS when the player picks them.
//!
//! Haiku Setup extracts the layer from a dump with Japanese voices (an EUR
//! "undub" or a Japanese Fire Emblem if) into `data/voices-ja/`, in the mod
//! layout: `romfs/sound/IRON15_sound.bcsar` (the Japanese sound archive),
//! `romfs/sound/IndirectSound.bin.lz` (its indirect-sound table, with the
//! aliases that give Kana the Japanese lines), `romfs/sound/stream/*.aac`
//! (the voice clips) and `*.bcstm` (movie soundtracks, Azura's song), and
//! `romfs/movie/**/*_bgm.bin` (the movies' sound cues), plus `voices.txt`,
//! what it was made from. Nothing else of the dump. The installed game is
//! never written, so English is the game as it is and switching back is
//! only leaving the layer out at the next start.
//!
//! The archive is used whole, with its own table: the game finds every
//! sound by its label, never by number, and the two archives number their
//! sounds differently (dev/reports/jpvoice-survey.md).
//!
//! `PcSettings::voices` picks the language; `HAIKU_VOICES=ja|en` overrides
//! it (tests), `HAIKU_VOICES_DIR` names another layer folder.
//!
//! jpdlc: the DLC's Japanese voices are a second layer beside the first,
//! `data/voices-ja-dlc/`, which Haiku Setup takes from the Japanese DLC: in
//! the mod layout for DLC, `dlc/<content index>/romfs/local/sound/...` (each
//! map's sound archives and voice, narration and song streams) and the
//! route contents' movie cue tables, with its own `voices.txt` and
//! `files.tsv`. It goes with the first layer (Voices: Japanese, a layer
//! installed), right after it in the mod list; the mod system lays its
//! folders over the DLC's contents (mods::lay_dlc), whether the DLC is
//! installed as folders or kept as a CIA. Everything else of the DLC (maps,
//! scripts, text) stays the installed DLC's. `HAIKU_VOICES_DLC_DIR` names
//! another folder.

use std::path::{Path, PathBuf};

use crate::settings::{PcSettings, Voices};

/// the file that marks a complete layer: Haiku Setup writes it last.
pub const MANIFEST: &str = "voices.txt";
pub const FOLDER: &str = "voices-ja";
/// jpdlc: the DLC's Japanese voices, beside FOLDER.
pub const DLC_FOLDER: &str = "voices-ja-dlc";

/// the voice layer for the game at game (its data/ folder, or a ROM image
/// beside one), when one is installed and complete.
pub fn installed(game: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("HAIKU_VOICES_DIR").map(PathBuf::from).into_iter().collect();
    if let Some(game) = game {
        if game.is_dir() {
            candidates.push(game.join(FOLDER));
        } else if let Some(parent) = game.parent() {
            candidates.push(parent.join("data").join(FOLDER));
        }
    }
    let root = std::env::var_os("FATES_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned)));
    candidates.extend(root.map(|root| root.join("data").join(FOLDER)));
    candidates.into_iter().find(|dir| dir.join("romfs").join("sound").is_dir() && dir.join(MANIFEST).is_file())
}

/// jpdlc: the DLC voice layer that goes with the voice layer at layer, when
/// one is installed and complete (beside it, or HAIKU_VOICES_DLC_DIR).
pub fn installed_dlc(layer: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("HAIKU_VOICES_DLC_DIR").map(PathBuf::from).into_iter().collect();
    candidates.extend(layer.parent().map(|data| data.join(DLC_FOLDER)));
    candidates.into_iter().find(|dir| dir.join("dlc").is_dir() && dir.join(MANIFEST).is_file())
}

/// the source line of a layer's manifest.
fn source(layer: &Path) -> Option<String> {
    let text = std::fs::read_to_string(layer.join(MANIFEST)).ok()?;
    text.lines().find_map(|line| line.strip_prefix("source ").map(str::to_owned))
}

/// the language asked for: the settings, or HAIKU_VOICES.
pub fn wanted(pc: &PcSettings) -> Voices {
    match std::env::var("HAIKU_VOICES").ok().as_deref() {
        Some("ja" | "japanese" | "jp") => Voices::Japanese,
        Some("en" | "english") => Voices::English,
        _ => pc.voices,
    }
}

/// puts the voice layer under the mods (any mod's file wins) when the
/// Japanese voices are asked for and installed; says in the log which.
pub fn lay(mods: &mut Vec<PathBuf>, pc: &PcSettings, game: &Path) {
    // jpn: the Japanese game's voices are its own: no layer, no option
    if zakuro_core::region::is_jpn() {
        set(&RUNNING, Voices::English);
        set(&CHOSEN, Voices::English);
        log::info!("voices: the Japanese game's own (the Voices option is for the USA and EUR games)");
        return;
    }
    let layer = installed(Some(game));
    let wanted = wanted(pc);
    set(&RUNNING, if layer.is_some() { wanted } else { Voices::English });
    set(&CHOSEN, pc.voices);
    match (wanted, layer) {
        (Voices::English, _) => log::info!("voices: English (the game's own)"),
        (Voices::Japanese, Some(layer)) => {
            log::info!("voices: Japanese, the layer in {} (from {})", layer.display(), source(&layer).as_deref().unwrap_or("?"));
            // jpdlc: the DLC's voices right after it, when installed
            match installed_dlc(&layer) {
                Some(dlc) => {
                    log::info!("voices: Japanese for the DLC too, the layer in {} (from {})", dlc.display(), source(&dlc).as_deref().unwrap_or("?"));
                    mods.insert(0, dlc);
                }
                None => log::info!("voices: no DLC voice layer is installed; the DLC's voices stay English"),
            }
            mods.insert(0, layer);
        }
        (Voices::Japanese, None) => log::warn!("voices: Japanese asked for, but no voice layer is installed (Haiku Setup > Japanese voices); English"),
    }
}

/// what the in-game option shows: whether there is a layer to choose.
pub fn available() -> bool {
    if zakuro_core::region::is_jpn() {
        return false; // jpn: the game's own voices are Japanese
    }
    let game = LAST_GAME.lock().ok().and_then(|game| game.clone());
    installed(game.as_deref()).is_some()
}

/// jpdlc: whether the DLC's Japanese voices are installed too (the help line).
pub fn dlc_available() -> bool {
    let game = LAST_GAME.lock().ok().and_then(|game| game.clone());
    installed(game.as_deref()).and_then(|layer| installed_dlc(&layer)).is_some()
}

static LAST_GAME: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// remembers the game started, for available().
pub fn started(game: &Path) {
    if let Ok(mut last) = LAST_GAME.lock() {
        *last = Some(game.to_path_buf());
    }
}

// ---- the options menu's rows ------------------------------------------------

/// the voices the running game started with, and the ones the options
/// have chosen since: a restart applies the difference.
static RUNNING: std::sync::Mutex<Voices> = std::sync::Mutex::new(Voices::English);
static CHOSEN: std::sync::Mutex<Voices> = std::sync::Mutex::new(Voices::English);
static ASKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn set(cell: &std::sync::Mutex<Voices>, voices: Voices) {
    if let Ok(mut value) = cell.lock() {
        *value = voices;
    }
}
fn get(cell: &std::sync::Mutex<Voices>) -> Voices {
    cell.lock().map(|value| *value).unwrap_or_default()
}

/// the Audio tab's rows: Voices when there is a layer to choose, and the
/// restart while the choice differs from what the game runs with.
pub fn audio_rows<T>(list: &mut Vec<T>, voices: T, restart: T) {
    if !available() {
        return;
    }
    list.push(voices);
    if get(&CHOSEN) != get(&RUNNING) {
        list.push(restart);
    }
}

/// the options changed Voices.
pub fn chosen(voices: Voices) {
    set(&CHOSEN, voices);
}

/// the Voices row's help line.
pub fn help(voices: Voices) -> &'static str {
    let pending = voices != get(&RUNNING);
    match (voices, pending) {
        // jpdlc: the DLC's voices too, when Haiku Setup added them
        (Voices::Japanese, false) if dlc_available() => "The Japanese cast's voices, the DLC's included, from the dumps you gave Haiku Setup. The text stays English.",
        (Voices::Japanese, false) => "The Japanese cast's voices, from the dump you gave Haiku Setup. The text stays English.",
        (Voices::English, false) if dlc_available() => "The English cast's voices, the game's own. Japanese uses the voices Haiku Setup took from your Japanese game and DLC.",
        (Voices::English, false) => "The English cast's voices, the game's own. Japanese uses the voices Haiku Setup took from your Japanese dump.",
        (_, true) => "Takes effect when the game restarts: choose Restart with These Voices, or the next time you start Haiku.",
    }
}

/// the restart dialog is asking about the voices (for its question).
pub fn set_asking(on: bool) {
    ASKING.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn asking() -> bool {
    ASKING.load(std::sync::atomic::Ordering::Relaxed)
}

/// the voices the running game started with.
pub fn running() -> Voices {
    get(&RUNNING)
}

/// the other language.
pub fn other(voices: Voices) -> Voices {
    if voices == Voices::Japanese {
        Voices::English
    } else {
        Voices::Japanese
    }
}
