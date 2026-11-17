//! input: the game's own 3DS button pictures (A/B/X/Y/L/R/START in its UI
//! atlas, the bouncing A of message boxes) swapped at run time for Xbox,
//! PlayStation or keyboard versions drawn in the same style, following the
//! glyph set (`pc.glyphs`, Auto = the device used last).
//!
//! The pictures are Citra-format texture-pack files in
//! `assets/glyphs/<set>/` (made by `dev/glyphs/make_glyphs.py` from the
//! game's textures), keyed by the hash of the texture each replaces. They
//! are put over the player's own texture pack, if any, with
//! `Pack::merged`, and the merged pack is handed to the GPU when the set
//! changes. The Nintendo set is the game's own art: no pictures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zakuro_core::System;
use zakuro_gpu::pack::Pack;

use crate::controls::GlyphSet;

#[derive(Default)]
pub struct Glyphs {
    /// the set drawn now, none before the first apply of a game.
    applied: Option<GlyphSet>,
    /// the player's own pack for this game, opened once.
    base: Option<Option<Arc<Pack>>>,
    /// each set's pictures, opened once.
    sets: HashMap<GlyphSet, Option<Arc<Pack>>>,
    /// mods: the texture packs of the mods turned on, in load order, merged
    /// over the player's own pack (a later mod's picture wins) and under
    /// the glyphs.
    pub mod_textures: Vec<PathBuf>,
}

/// the folder the glyph sets are in: assets/glyphs beside the program.
fn root() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join("assets").join("glyphs");
    if beside.is_dir() {
        return Some(beside);
    }
    let root = std::env::var_os("FATES_ROOT").map(|root| PathBuf::from(root).join("assets").join("glyphs"))?;
    root.is_dir().then_some(root)
}

/// FATES_GLYPHS=nintendo|xbox|playstation|keyboard forces a set, for tests.
pub fn forced() -> Option<GlyphSet> {
    match std::env::var("FATES_GLYPHS").ok()?.to_ascii_lowercase().as_str() {
        "nintendo" => Some(GlyphSet::Nintendo),
        "xbox" => Some(GlyphSet::Xbox),
        "playstation" | "ps" => Some(GlyphSet::PlayStation),
        "keyboard" => Some(GlyphSet::Keyboard),
        _ => None,
    }
}

pub fn folder_name(set: GlyphSet) -> &'static str {
    match set {
        GlyphSet::Nintendo => "nintendo",
        GlyphSet::Xbox => "xbox",
        GlyphSet::PlayStation => "playstation",
        GlyphSet::Keyboard => "keyboard",
    }
}

impl Glyphs {
    /// a new game: its own pack is opened again on the next apply.
    pub fn reset(&mut self) {
        self.applied = None;
        self.base = None;
    }

    /// draws `set`'s pictures from now on, when it is not drawn already.
    pub fn apply(&mut self, set: GlyphSet, system: &mut System, data_dir: Option<&Path>, texture_packs: bool) {
        if self.applied == Some(set) {
            return;
        }
        let first = self.applied.is_none();
        self.applied = Some(set);
        let mod_textures = &self.mod_textures;
        let base = self
            .base
            .get_or_insert_with(|| {
                if !texture_packs {
                    return None;
                }
                let program_id = system.title.as_ref()?.program_id();
                let dir = data_dir.map(|data_dir| zakuro_core::mods::textures(data_dir, program_id));
                let mut pack = dir.and_then(|dir| Pack::open(&dir));
                // mods: each mod's pack over the ones before it
                for dir in mod_textures {
                    if let Some(over) = Pack::open(dir) {
                        log::info!("mods: the texture pack in {}, {} pictures", dir.display(), over.len());
                        pack = Some(Pack::merged(pack.as_ref(), &over));
                    }
                }
                pack.map(Arc::new)
            })
            .clone();
        let overlay = self
            .sets
            .entry(set)
            .or_insert_with(|| {
                let dir = root()?.join(folder_name(set));
                let pack = Pack::open(&dir).map(Arc::new);
                if pack.is_none() && set != GlyphSet::Nintendo {
                    log::warn!("input: no {} glyph pictures in {}", folder_name(set), dir.display());
                }
                pack
            })
            .clone();
        let pack = match (&base, overlay) {
            (_, Some(overlay)) => Some(Arc::new(Pack::merged(base.as_deref(), &overlay))),
            (Some(base), None) => Some(base.clone()),
            (None, None) => None,
        };
        // nothing changes for a game with no pack starting on the game's own
        // pictures, so its textures are not decoded again for nothing
        if first && pack.is_none() {
            return;
        }
        let pictures = pack.as_ref().map_or(0, |pack| pack.len());
        let drawn = system.gpu.set_texture_pack(pack);
        log::info!(
            "input: button glyphs now {} ({pictures} pictures{})",
            folder_name(set),
            if drawn || pictures == 0 { "" } else { ", not drawn: the 3D is not on the GPU" }
        );
    }
}
