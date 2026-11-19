//! versions: Options > Interface > Game Version. With more than one version
//! of the game installed in the Haiku folder (games\USA, games\EUR,
//! games\JPN: haiku-setup's layout.rs), this row picks the one Haiku plays
//! (PcSettings::game_version), and "Restart with This Version" restarts the
//! game the way Apply and Restart does for the mods (the settings saved, a
//! fresh Haiku.exe). Haiku.exe reads the setting at start and plays that
//! version's data, module, assets and mods. Haiku.exe tells this module
//! what is installed (HAIKU_VERSIONS, "usa,eur,jpn") and what plays now
//! (HAIKU_VERSION).

//!
//! versions2: each version keeps its own game language and voices
//! (`[pc.per_version.<tag>]`): settings_loaded puts the playing version's
//! into the settings when they are read, settings_saved writes them back
//! into its entry when they are saved. The shared `language` and
//! `[pc] voices` lines stay what they were (an older settings file's
//! choice), the default for a version without an entry of its own, and
//! a language a version doesn't have is never carried over: the US game is
//! English, the Japanese game Japanese, the European one of its five.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::settings::{Language, Settings, VersionSettings, Voices};

static ASKING: AtomicBool = AtomicBool::new(false);
static CHOSEN: Mutex<Option<String>> = Mutex::new(None);
/// versions2: the shared lines as the file had them, written back as they
/// were
static SHARED: Mutex<Option<(Language, Voices)>> = Mutex::new(None);

/// versions2: the European Special Edition's languages, in its menus' order
pub const EUR_LANGUAGES: [Language; 5] = [Language::English, Language::French, Language::German, Language::Spanish, Language::Italian];

/// versions2: a language the version has: the one asked for when it has
/// it, else its own default (English; the Japanese game is Japanese only)
pub fn language_for(tag: &str, language: Language) -> Language {
    match tag {
        "eur" if EUR_LANGUAGES.contains(&language) => language,
        "jpn" => Language::Japanese,
        _ => Language::English,
    }
}

/// versions2: the version whose own settings these are: the one playing
/// in a Haiku folder with games\<version>\ (none in a flat folder, which
/// has one version and keeps the shared lines as before)
fn settings_tag() -> Option<String> {
    let tag = running().to_ascii_lowercase();
    matches!(tag.as_str(), "usa" | "eur" | "jpn").then_some(tag)
}

/// versions2: the version's own language and voices over the shared ones
fn resolve(settings: &mut Settings, tag: &str) {
    let own = settings.pc.per_version.get(tag).cloned().unwrap_or_default();
    let language = language_for(tag, own.language.unwrap_or(settings.language));
    let voices = own.voices.unwrap_or(settings.pc.voices);
    if language != settings.language || voices != settings.pc.voices {
        log::info!("versions2: {tag} plays with its own settings: language {} (shared {}), voices {voices:?} (shared {:?})", language.name(), settings.language.name(), settings.pc.voices);
    }
    settings.language = language;
    settings.pc.voices = voices;
}

/// versions2: the file's copy: the version's language and voices in its
/// entry, the shared lines as they were
fn store(settings: &Settings, tag: &str, shared: Option<(Language, Voices)>) -> Settings {
    let mut file = settings.clone();
    file.pc.per_version.insert(tag.to_owned(), VersionSettings { language: Some(settings.language), voices: Some(settings.pc.voices) });
    if let Some((language, voices)) = shared {
        file.language = language;
        file.pc.voices = voices;
    }
    file
}

/// versions2: settings just read (Settings::load)
pub fn settings_loaded(settings: &mut Settings) {
    let Some(tag) = settings_tag() else { return };
    if let Ok(mut shared) = SHARED.lock() {
        *shared = Some((settings.language, settings.pc.voices));
    }
    resolve(settings, &tag);
}

/// versions2: what Settings::save writes
pub fn settings_saved(settings: &Settings) -> Cow<'_, Settings> {
    let Some(tag) = settings_tag() else { return Cow::Borrowed(settings) };
    let shared = SHARED.lock().ok().and_then(|shared| *shared);
    Cow::Owned(store(settings, &tag, shared))
}

/// the versions installed, as Haiku.exe found them ("usa", "eur", "jpn")
pub fn installed() -> Vec<String> {
    std::env::var("HAIKU_VERSIONS").map(|list| list.split(',').filter(|v| !v.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

/// the version playing now
pub fn running() -> String {
    std::env::var("HAIKU_VERSION").unwrap_or_default()
}

/// the row is shown with two versions or more
pub fn available() -> bool {
    installed().len() > 1
}

/// the row's value for a setting: the version it names when installed,
/// else the one playing
pub fn shown(setting: &str) -> String {
    let installed = installed();
    if installed.iter().any(|v| v.eq_ignore_ascii_case(setting)) {
        setting.to_ascii_lowercase()
    } else {
        running()
    }
}

/// the next installed version after the one shown, either way
pub fn step(setting: &str, dir: i32) -> String {
    let installed = installed();
    if installed.is_empty() {
        return setting.to_owned();
    }
    let at = installed.iter().position(|v| *v == shown(setting)).unwrap_or(0) as i32;
    let n = installed.len() as i32;
    installed[((at + if dir < 0 { -1 } else { 1 }) % n + n) as usize % installed.len()].clone()
}

/// for the player
pub fn label(tag: &str) -> &'static str {
    match tag {
        "usa" => "USA",
        "eur" => "Europe",
        "jpn" => "Japan",
        _ => "",
    }
}

/// the options changed the row
pub fn chosen(tag: &str) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        *chosen = Some(tag.to_owned());
    }
}

/// a version other than the one playing is chosen: the restart row shows
pub fn pending() -> bool {
    CHOSEN.lock().ok().and_then(|chosen| chosen.clone()).is_some_and(|tag| !tag.is_empty() && tag != running())
}

pub fn help(tag: &str) -> String {
    let what = match tag {
        "usa" => "The US version of the game (English).",
        "eur" => "The European version of the game (English, French, German, Italian or Spanish).",
        "jpn" => "The Japanese version of the game, Fire Emblem if.",
        _ => "",
    };
    if pending() {
        format!("{what} It plays after a restart: Restart with This Version. Each version has its own mods, game language and voices; saves are kept for each.")
    } else {
        format!("{what} Each version has its own mods folder (games\\{}\\mods), game language and voices; saves are kept for each.", tag.to_ascii_uppercase())
    }
}

/// the Interface rows: Game Version, and its restart while another is
/// chosen
pub fn interface_rows<T>(list: &mut Vec<T>, row: T, restart: T) {
    if !available() {
        return;
    }
    list.push(row);
    if pending() {
        list.push(restart);
    }
}

/// the restart dialog is asking about the version (for its question)
pub fn set_asking(on: bool) {
    ASKING.store(on, Ordering::Relaxed);
}

pub fn asking() -> bool {
    ASKING.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_steps_through_what_is_installed() {
        std::env::set_var("HAIKU_VERSIONS", "usa,eur,jpn");
        std::env::set_var("HAIKU_VERSION", "usa");
        assert!(available());
        assert_eq!(shown(""), "usa");
        assert_eq!(step("", 1), "eur");
        assert_eq!(step("eur", 1), "jpn");
        assert_eq!(step("jpn", 1), "usa");
        assert_eq!(step("usa", -1), "jpn");
        chosen("eur");
        assert!(pending());
        chosen("usa");
        assert!(!pending());
        std::env::set_var("HAIKU_VERSIONS", "usa");
        assert!(!available());
    }

    #[test]
    fn each_version_keeps_its_own_language_and_voices() {
        // an older file: one shared language and voices
        let mut file = Settings { language: Language::German, ..Settings::default() };
        file.pc.voices = Voices::Japanese;
        let shared = Some((file.language, file.pc.voices));
        // the European version: the shared German is one of its languages
        let mut eur = file.clone();
        resolve(&mut eur, "eur");
        assert_eq!(eur.language, Language::German);
        assert_eq!(eur.pc.voices, Voices::Japanese);
        // the US version has English only; the Japanese, Japanese only
        let mut usa = file.clone();
        resolve(&mut usa, "usa");
        assert_eq!(usa.language, Language::English);
        let mut jpn = file.clone();
        resolve(&mut jpn, "jpn");
        assert_eq!(jpn.language, Language::Japanese);
        // French chosen on the European version, English voices on the US
        eur.language = Language::French;
        let written = store(&eur, "eur", shared);
        assert_eq!(written.language, Language::German, "the shared line stays");
        assert_eq!(written.pc.per_version["eur"].language, Some(Language::French));
        let mut usa = written.clone();
        resolve(&mut usa, "usa");
        assert_eq!(usa.language, Language::English);
        usa.pc.voices = Voices::English;
        let written = store(&usa, "usa", shared);
        assert_eq!(written.pc.voices, Voices::Japanese, "the shared voices stay");
        // through the file and back: each version gets its own
        let text = toml::to_string_pretty(&written).unwrap();
        assert!(text.contains("[pc.per_version.eur]"), "{text}");
        let read: Settings = toml::from_str(&text).unwrap();
        let mut eur = read.clone();
        resolve(&mut eur, "eur");
        assert_eq!((eur.language, eur.pc.voices), (Language::French, Voices::Japanese));
        let mut usa = read.clone();
        resolve(&mut usa, "usa");
        assert_eq!((usa.language, usa.pc.voices), (Language::English, Voices::English));
        // a hand-written language the version doesn't have
        let mut odd = read.clone();
        odd.pc.per_version.get_mut("eur").unwrap().language = Some(Language::Korean);
        resolve(&mut odd, "eur");
        assert_eq!(odd.language, Language::English);
        // an unknown value in an entry leaves the rest
        let read: Settings = toml::from_str("language = \"italian\"\n[pc.per_version.eur]\nlanguage = \"klingon\"\nvoices = \"japanese\"\n").unwrap();
        let mut eur = read.clone();
        resolve(&mut eur, "eur");
        assert_eq!((eur.language, eur.pc.voices), (Language::Italian, Voices::Japanese));
    }
}
