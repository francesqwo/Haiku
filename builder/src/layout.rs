//! versions: the USA, European and Japanese versions of the game installed
//! side by side in one Haiku folder, the one Haiku plays chosen in the
//! options (Interface > Game Version, a restart like the Mods screen's).
//!
//!   Haiku.exe, SDL3.dll, setup\          the program and what the setup ships
//!   userdata\haiku\                      shared: settings.toml (with
//!                                        [pc] game_version), the saves (per
//!                                        title id, as before), haiku.log
//!   setup-work\setup.log                 the setup's log
//!   games\USA\  games\EUR\  games\JPN\   one per version, each with:
//!     data\           the game's files from that dump, data\code\game.dll
//!     assets\         Haiku's files made from them
//!     mods\           that version's mods, and mods.toml, their list
//!     cache\          its caches (code mods' modules, pipelines)
//!     setup-work\     the setup's state and the module's objects
//!
//! Installing a version writes only its own folder. Haiku.exe sets
//! FATES_ROOT to the playing version's folder, so the game finds its data,
//! assets, mods and caches there, and HAIKU_HOME to the Haiku folder for
//! what is shared. A folder without games\ is laid out flat as before (a
//! development folder; an install from before versions, which migrate()
//! moves into games\ at the next start).

use std::path::{Path, PathBuf};

/// the folder the versions are in, beside Haiku.exe
pub const GAMES: &str = "games";
/// the setting that says which version plays ([pc] in settings.toml)
pub const SETTING: &str = "game_version";
/// a version's own list of its mods (the game reads it through
/// HAIKU_MODS_LIST)
pub const MODS_LIST: &str = "mods.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Version {
    Usa,
    Eur,
    Jpn,
}

impl Version {
    pub const ALL: [Version; 3] = [Version::Usa, Version::Eur, Version::Jpn];

    /// its folder's name under games\
    pub fn folder(self) -> &'static str {
        match self {
            Version::Usa => "USA",
            Version::Eur => "EUR",
            Version::Jpn => "JPN",
        }
    }

    /// as the setting and the module's region write it
    pub fn tag(self) -> &'static str {
        match self {
            Version::Usa => "usa",
            Version::Eur => "eur",
            Version::Jpn => "jpn",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Version> {
        Version::ALL.into_iter().find(|v| v.tag().eq_ignore_ascii_case(tag.trim()) || v.folder().eq_ignore_ascii_case(tag.trim()))
    }

    /// for the player
    pub fn name(self) -> &'static str {
        match self {
            Version::Usa => "the US version",
            Version::Eur => "the European version",
            Version::Jpn => "the Japanese version (Fire Emblem if)",
        }
    }

    /// the version a title id is (any edition of it)
    pub fn of_title(title: u64) -> Option<Version> {
        Some(match crate::regions::Region::of(title)? {
            crate::regions::Region::Usa => Version::Usa,
            crate::regions::Region::Eur => Version::Eur,
            crate::regions::Region::Jpn => Version::Jpn,
        })
    }

    /// the version a dump is
    pub fn of_region(region: crate::jpn::GameRegion) -> Version {
        match region {
            crate::jpn::GameRegion::Usa => Version::Usa,
            crate::jpn::GameRegion::Eur => Version::Eur,
            crate::jpn::GameRegion::Jpn => Version::Jpn,
        }
    }
}

/// the folder of one version
pub fn dir(root: &Path, version: Version) -> PathBuf {
    root.join(GAMES).join(version.folder())
}

/// the versions installed in root's games\ (a game in their data\)
pub fn installed(root: &Path) -> Vec<Version> {
    Version::ALL.into_iter().filter(|v| zakuro_fs::datadir::is_game_dir(&dir(root, *v).join("data"))).collect()
}

/// the version the settings ask for, if any
pub fn chosen(root: &Path) -> Option<Version> {
    pc_value(&settings_file(root), SETTING).and_then(|value| Version::from_tag(&value))
}

/// the version that plays: the one chosen when it is installed, else the
/// first installed (USA, EUR, JPN)
pub fn active(root: &Path) -> Option<Version> {
    let installed = installed(root);
    chosen(root).filter(|v| installed.contains(v)).or_else(|| installed.first().copied())
}

/// the folder the game plays from: the active version's, or root itself
/// when there is no games\ (a flat folder)
pub fn game_dir(root: &Path) -> PathBuf {
    match active(root) {
        Some(version) => dir(root, version),
        None => root.to_path_buf(),
    }
}

pub fn settings_file(root: &Path) -> PathBuf {
    root.join("userdata").join("haiku").join("settings.toml")
}

/// a value in settings.toml's [pc] table (a plain line reader: the setup
/// runs before the game's settings are read)
pub fn pc_value(path: &Path, key: &str) -> Option<String> {
    table_value(path, "pc", key)
}

/// versions2: a value in one table of settings.toml ("pc",
/// "pc.per_version.eur")
pub fn table_value(path: &Path, table: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let header = format!("[{table}]");
    let mut in_pc = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pc = t == header;
        } else if in_pc {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().trim_matches('"').to_owned());
                }
            }
        }
    }
    None
}

/// sets a string in settings.toml's [pc] table, every other line kept as
/// it is (a missing file gets just this; the game fills in the rest)
pub fn set_pc_value(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    set_table_value(path, "pc", key, value)
}

/// versions2: sets a string in one table of settings.toml, made at the
/// end when the file has none
pub fn set_table_value(path: &Path, table: &str, key: &str, value: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let line = format!("{key} = \"{value}\"");
    let header = format!("[{table}]");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut in_pc = false;
    let mut pc_header = None;
    let mut done = false;
    for (i, l) in lines.iter_mut().enumerate() {
        let t = l.trim();
        if t.starts_with('[') {
            in_pc = t == header;
            if in_pc {
                pc_header = Some(i);
            }
        } else if in_pc && t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            *l = line.clone();
            done = true;
            break;
        }
    }
    if !done {
        match pc_header {
            Some(i) => lines.insert(i + 1, line),
            None => {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(header);
                lines.push(line);
            }
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, lines.join("\n") + "\n")
}

/// makes version the one that plays
pub fn set_active(root: &Path, version: Version) -> std::io::Result<()> {
    set_pc_value(&settings_file(root), SETTING, version.tag())
}

/// versions2: the settings table of what one version keeps of its own
/// (the game's PcSettings::per_version: its game language and voices)
pub fn own_table(version: Version) -> String {
    format!("pc.per_version.{}", version.tag())
}

/// versions2: a folder of a version being removed (renamed out of the way
/// first, then deleted): games\.removing-<FOLDER>-<n>
const REMOVING: &str = ".removing-";

/// versions2: what removing a version would delete, for the confirmation
#[derive(Debug, Clone, Default)]
pub struct RemoveSummary {
    pub bytes: u64,
    pub files: u64,
    /// the folders in its mods\
    pub mods: usize,
    /// its data\ has Japanese voices (voices-ja)
    pub voices: bool,
    /// it is the version playing now; the one that plays after
    pub playing: bool,
    pub next: Option<Version>,
}

fn walk_size(dir: &Path, bytes: &mut u64, files: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            walk_size(&entry.path(), bytes, files);
        } else if kind.is_file() {
            *bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            *files += 1;
        }
    }
}

/// versions2: what removing version would delete
pub fn remove_summary(root: &Path, version: Version) -> RemoveSummary {
    let folder = dir(root, version);
    let mut summary = RemoveSummary::default();
    walk_size(&folder, &mut summary.bytes, &mut summary.files);
    summary.mods = std::fs::read_dir(folder.join("mods")).map(|e| e.flatten().filter(|e| e.file_type().is_ok_and(|k| k.is_dir())).count()).unwrap_or(0);
    summary.voices = folder.join("data").join("voices-ja").is_dir();
    summary.playing = active(root) == Some(version);
    summary.next = if summary.playing { installed(root).into_iter().find(|v| *v != version) } else { active(root) };
    summary
}

/// versions2: removes one version of the game: its games\<version>\ folder
/// (its game files and module, Haiku's files made from it, its caches, its
/// mods and their list, the setup's state for it). The shared userdata\
/// (settings, every version's saves) is not touched, nor the other
/// versions. The only version installed is never removed. When it is the
/// one playing, another plays from now on. The folder is renamed out of
/// the way first (so a file in use stops it before anything changes) and
/// then deleted: the folder to delete is returned, for delete_removed,
/// which may take a while.
pub fn remove(root: &Path, version: Version) -> Result<PathBuf, String> {
    let installed = installed(root);
    if !installed.contains(&version) {
        return Err(format!("{} isn't installed.", capital(version.name())));
    }
    let Some(other) = installed.iter().copied().find(|v| *v != version) else {
        return Err(format!("{} is the only version installed, and Haiku needs one: add another version first, or delete the Haiku folder to remove Haiku.", capital(version.name())));
    };
    let playing = active(root) == Some(version);
    let from = dir(root, version);
    let mut to = root.join(GAMES).join(format!("{REMOVING}{}-{}", version.folder(), std::process::id()));
    let mut n = 0;
    while to.exists() {
        n += 1;
        to = root.join(GAMES).join(format!("{REMOVING}{}-{}-{n}", version.folder(), std::process::id()));
    }
    std::fs::rename(&from, &to).map_err(|e| format!("{} can't be removed now: a file in {} is in use ({e}). Close the game (and anything with a file of it open) and try again.", capital(version.name()), from.display()))?;
    if playing {
        if let Err(e) = set_active(root, other) {
            // put back: nothing changed
            let _ = std::fs::rename(&to, &from);
            return Err(format!("the settings can't be written ({e}); nothing was removed"));
        }
    }
    Ok(to)
}

/// versions2: deletes a folder remove() renamed; what is left (a file in
/// use) goes at the next start (sweep)
pub fn delete_removed(folder: &Path) -> std::io::Result<()> {
    std::fs::remove_dir_all(folder)
}

/// versions2: what a removal left in games\ (Haiku closed before it was
/// all deleted), deleted now
pub fn sweep(root: &Path) -> Vec<String> {
    let mut said = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join(GAMES)) else { return said };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(REMOVING) || !entry.file_type().is_ok_and(|k| k.is_dir()) {
            continue;
        }
        match delete_removed(&entry.path()) {
            Ok(()) => said.push(format!("versions: deleted what was left of a removed version ({})", entry.path().display())),
            Err(e) => said.push(format!("versions: {} can't be deleted yet ({e})", entry.path().display())),
        }
    }
    said
}

/// "the European version" as a sentence starts
pub fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// what of a flat install belongs to its version: (from root, to the
/// version's folder)
const MOVED: [(&str, &str); 4] = [("data", "data"), ("assets", "assets"), ("mods", "mods"), ("cache", "cache")];

/// an install from before versions (a release install, setup\payload.txt
/// beside Haiku.exe, with the game in data\): its game, Haiku's files made
/// from it, its mods, mods.toml, caches and the setup's state move into
/// games\<its version>\, each a rename on the same drive (nothing is
/// copied). What can't be moved now (a file in use) is left and tried at
/// the next start; until then the game plays from where things are.
/// Returns what was done, for the log.
pub fn migrate(root: &Path) -> Vec<String> {
    let mut said = Vec::new();
    let managed = root.join("setup").join("payload.txt").is_file();
    let flat = root.join("data");
    if !managed || !zakuro_fs::datadir::is_game_dir(&flat) {
        return said;
    }
    let Some(version) = crate::pipeline::installed_title(&flat).and_then(Version::of_title) else {
        said.push("versions: the game in data\\ is no version Haiku knows; left where it is".into());
        return said;
    };
    let to = dir(root, version);
    if zakuro_fs::datadir::is_game_dir(&to.join("data")) {
        said.push(format!("versions: {} is installed in {} already; data\\ is left where it is", version.name(), to.display()));
        return said;
    }
    if let Err(e) = std::fs::create_dir_all(&to) {
        said.push(format!("versions: {} can't be made ({e})", to.display()));
        return said;
    }
    let mut move_one = |from: PathBuf, target: PathBuf, what: &str| {
        if !from.exists() || target.exists() {
            return;
        }
        match std::fs::rename(&from, &target) {
            Ok(()) => said.push(format!("versions: moved {what} to {}", target.display())),
            Err(e) => said.push(format!("versions: {what} can't be moved now ({e}); tried again at the next start")),
        }
    };
    for (name, into) in MOVED {
        move_one(root.join(name), to.join(into), &format!("{name}\\"));
    }
    move_one(root.join("userdata").join("haiku").join(MODS_LIST), to.join(MODS_LIST), "userdata\\haiku\\mods.toml");
    // the setup's state and the module's objects; its log stays at the top
    let work = root.join("setup-work");
    if let Ok(entries) = std::fs::read_dir(&work) {
        let target = to.join("setup-work");
        let _ = std::fs::create_dir_all(&target);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let text = name.to_string_lossy().to_ascii_lowercase();
            if text == "setup.log" || text.starts_with("setup-1") {
                continue;
            }
            move_one(entry.path(), target.join(&name), &format!("setup-work\\{}", name.to_string_lossy()));
        }
    }
    // the settings aren't written: the only version installed plays anyway
    // (active), and an upgrade leaves the player's settings as they are
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_game(data: &Path, title: u64) {
        use zakuro_fs::datadir::{EXEFS_FILE, EXHEADER_FILE, MANIFEST_FILE, NCCH_FILE};
        std::fs::create_dir_all(data).unwrap();
        for file in [NCCH_FILE, EXHEADER_FILE, EXEFS_FILE] {
            std::fs::write(data.join(file), b"").unwrap();
        }
        std::fs::write(data.join(MANIFEST_FILE), format!("title {title:016X}\n")).unwrap();
    }

    #[test]
    fn settings_line_is_set_and_read_and_other_lines_kept() {
        let dir = std::env::temp_dir().join(format!("haiku-layout-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "language = \"english\"\n[pc]\nvoices = \"japanese\"\n[keys]\na = 1\n").unwrap();
        set_pc_value(&path, SETTING, "eur").unwrap();
        assert_eq!(pc_value(&path, SETTING).as_deref(), Some("eur"));
        assert_eq!(pc_value(&path, "voices").as_deref(), Some("japanese"));
        set_pc_value(&path, SETTING, "jpn").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches(SETTING).count(), 1);
        assert!(text.contains("[keys]\na = 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_install_moves_into_its_version_and_plays_from_there() {
        let root = std::env::temp_dir().join(format!("haiku-layout-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("setup")).unwrap();
        std::fs::write(root.join("setup").join("payload.txt"), "x").unwrap();
        fake_game(&root.join("data"), 0x0004_0000_0017_9800);
        std::fs::create_dir_all(root.join("mods").join("a mod")).unwrap();
        std::fs::create_dir_all(root.join("userdata").join("haiku")).unwrap();
        std::fs::write(root.join("userdata").join("haiku").join(MODS_LIST), "[[mod]]\n").unwrap();
        std::fs::create_dir_all(root.join("setup-work").join("module")).unwrap();
        std::fs::write(root.join("setup-work").join("setup.log"), "log").unwrap();
        std::fs::write(root.join("setup-work").join("state.txt"), "state").unwrap();
        let said = migrate(&root);
        assert!(said.iter().any(|s| s.contains("moved data")), "{said:?}");
        let usa = dir(&root, Version::Usa);
        assert!(zakuro_fs::datadir::is_game_dir(&usa.join("data")));
        assert!(usa.join("mods").join("a mod").is_dir());
        assert!(usa.join(MODS_LIST).is_file());
        assert!(usa.join("setup-work").join("state.txt").is_file());
        assert!(usa.join("setup-work").join("module").is_dir());
        assert!(root.join("setup-work").join("setup.log").is_file());
        assert!(!root.join("data").exists());
        assert_eq!(active(&root), Some(Version::Usa));
        assert_eq!(game_dir(&root), usa);
        // a second start does nothing
        assert!(migrate(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_version_table_is_set_and_read() {
        let dir = std::env::temp_dir().join(format!("haiku-layout-table-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "language = \"english\"\n[pc]\nvoices = \"japanese\"\n[pc.cheats]\nx = true\n").unwrap();
        let eur = own_table(Version::Eur);
        assert_eq!(table_value(&path, &eur, "voices"), None);
        set_table_value(&path, &eur, "voices", "english").unwrap();
        assert_eq!(table_value(&path, &eur, "voices").as_deref(), Some("english"));
        assert_eq!(pc_value(&path, "voices").as_deref(), Some("japanese"), "the shared line stays");
        set_table_value(&path, &eur, "voices", "japanese").unwrap();
        set_pc_value(&path, SETTING, "eur").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[pc.per_version.eur]").count(), 1);
        assert_eq!(table_value(&path, &eur, "voices").as_deref(), Some("japanese"));
        assert_eq!(pc_value(&path, SETTING).as_deref(), Some("eur"));
        assert!(text.contains("[pc.cheats]\nx = true"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_is_removed_with_its_folder_and_the_saves_kept() {
        let root = std::env::temp_dir().join(format!("haiku-layout-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        fake_game(&dir(&root, Version::Usa).join("data"), 0x0004_0000_0017_9800);
        fake_game(&dir(&root, Version::Eur).join("data"), 0x0004_0000_0017_A800);
        std::fs::create_dir_all(dir(&root, Version::Eur).join("mods").join("a mod")).unwrap();
        std::fs::create_dir_all(dir(&root, Version::Eur).join("cache")).unwrap();
        let saves = root.join("userdata").join("haiku").join("user");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("save"), "kept").unwrap();
        set_active(&root, Version::Eur).unwrap();
        let summary = remove_summary(&root, Version::Eur);
        assert!(summary.playing && summary.mods == 1 && summary.files >= 4);
        assert_eq!(summary.next, Some(Version::Usa));
        // the playing one: the other plays from now on
        let trash = remove(&root, Version::Eur).unwrap();
        assert!(!dir(&root, Version::Eur).exists());
        assert_eq!(installed(&root), [Version::Usa]);
        assert_eq!(chosen(&root), Some(Version::Usa));
        assert_eq!(std::fs::read_to_string(saves.join("save")).unwrap(), "kept");
        // the only one left is never removed
        assert!(remove(&root, Version::Usa).is_err());
        assert!(remove(&root, Version::Jpn).is_err());
        assert!(zakuro_fs::datadir::is_game_dir(&dir(&root, Version::Usa).join("data")));
        // what a removal leaves goes at the next start
        assert!(trash.is_dir());
        assert_eq!(sweep(&root).len(), 1);
        assert!(!trash.exists());
        assert!(sweep(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_chosen_version_plays_when_installed() {
        let root = std::env::temp_dir().join(format!("haiku-layout-active-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        fake_game(&dir(&root, Version::Eur).join("data"), 0x0004_0000_0017_A800);
        fake_game(&dir(&root, Version::Jpn).join("data"), 0x0004_0000_0012_DE00);
        assert_eq!(installed(&root), [Version::Eur, Version::Jpn]);
        assert_eq!(active(&root), Some(Version::Eur));
        set_active(&root, Version::Jpn).unwrap();
        assert_eq!(active(&root), Some(Version::Jpn));
        set_active(&root, Version::Usa).unwrap(); // not installed: the first that is
        assert_eq!(active(&root), Some(Version::Eur));
        let _ = std::fs::remove_dir_all(&root);
    }
}
