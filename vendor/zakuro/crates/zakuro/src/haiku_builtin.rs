//! sunglasses: Haiku's own game files, laid over the game's RomFS at boot
//! like a mod that is always on, under the player's mods (a mod's file wins).
//!
//! The folder is `assets/builtin/` beside Haiku.exe (or under FATES_ROOT),
//! in the mod layout (`romfs/...` with the game's paths). It holds data the
//! port adds to the game, such as the avatar creator's Sunglasses facial
//! detail (`dev/sunglasses/`, `dev/reports/sunglasses.md`): new files, and
//! game tables extended in place. The installed game is never written.
//! `HAIKU_BUILTIN=0` leaves it out (for comparing with the plain game).
//!
//! `assets/builtin/textures/` holds 4x pictures of Haiku's own new
//! textures (the Sunglasses portrait layers), named as Citra texture packs
//! name them. They are loaded only while a mod's texture pack is on, as
//! the lowest pack (any mod's picture wins): HD portraits from a pack then
//! get HD sunglasses to match, and without a pack the game's resolution is
//! kept.

use std::path::{Path, PathBuf};

/// the content folders of Haiku's own layer, lowest first: empty when the
/// folder is missing or HAIKU_BUILTIN=0.
pub fn content_dirs() -> Vec<PathBuf> {
    if std::env::var("HAIKU_BUILTIN").is_ok_and(|value| value == "0") {
        return Vec::new();
    }
    let root = std::env::var_os("FATES_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned)));
    let mut candidates: Vec<PathBuf> = root.into_iter().map(|root| root.join("assets").join("builtin")).collect();
    // a development run from the source tree
    candidates.push(PathBuf::from("assets").join("builtin"));
    candidates.into_iter().find(|dir| dir.join("romfs").is_dir()).into_iter().collect()
}

/// the texture packs of the mods turned on, with Haiku's own HD pictures
/// under them when there is at least one (sunglasses: the HD pack fix).
pub fn with_texture_packs(packs: Vec<PathBuf>) -> Vec<PathBuf> {
    if packs.is_empty() {
        return packs;
    }
    let mut dirs: Vec<PathBuf> = content_dirs().into_iter().map(|dir| dir.join("textures")).filter(|dir| dir.is_dir()).collect();
    dirs.extend(packs);
    dirs
}

/// the mods turned on, with Haiku's own layer under them.
pub fn with_mods(mods: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = content_dirs();
    dirs.extend(mods);
    dirs
}
