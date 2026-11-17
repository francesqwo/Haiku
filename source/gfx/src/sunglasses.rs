//! sunglasses: Haiku's Sunglasses facial detail drawn over Corrin's hair
//! (user feedback 2026-11-11c: some hairstyles covered the glasses).
//!
//! The game lays every facial detail under the hair: the portraits compose
//! face, detail, then hair (FaceCallback, prim.rs), the critical cut-in
//! draws hair_mat after face_mat, and on the 3D model the hair's bangs are
//! in front of the face. Glasses go over the hair, so for Haiku's
//! Sunglasses only (the portrait layers known by a mark in their texture
//! data, the 3D part by its material and texture names; dev/sunglasses):
//!
//! - portraits: the combiner order face, hair, glasses (prim.rs);
//! - the cut-in: face_mat drawn without the layer, hair_mat's geometry
//!   drawn again with it on top (`without_layer`, `over_hair`, lib.rs
//!   shade_draws);
//! - the 3D model: the glasses' depth is the depth they would have `PULL`
//!   head units nearer the camera (`pull`), so the bangs and side locks in
//!   front of them lose the depth test, while the head and anything further
//!   in front still hide them.
//!
//! FATES_SUNGLASSES_UNDER=1 (dev A/B) keeps the game's order everywhere.

use crate::prim;
use crate::setup::{Objects, Setup};
use zakuro_core::System;

/// How far the glasses' depth is pulled toward the camera, in the head's own
/// units (its model space: the face is about 2.1 wide and 2.5 deep). The
/// female Wild bangs need about 0.8 (0.5 leaves some over the lenses;
/// dev/sunglasses/pullcmp.py); the back of the head is more than 2 behind
/// them. FATES_SUNGLASSES_PULL overrides it (dev).
pub const PULL: f32 = 0.8;

fn disabled() -> bool {
    crate::perf::var_os("FATES_SUNGLASSES_UNDER").is_some()
}

/// Whether a mesh's material is the 3D part's: acc_mat with its texture
/// named acc_Facemyunit<F|M>_2_cl0n_P (make_model.py; the eyepatches are
/// _0 and _1).
pub fn is_3d(system: &mut System, material: u32) -> bool {
    // every mesh draw asks: the name compared in place first, no allocation
    let at = prim::r32(system, material + 0x28);
    if at == 0 || !system.memory.is_readable(at, 8) {
        return false;
    }
    let mut name = [0u8; 8];
    prim::rd(system, at, &mut name);
    if &name != b"acc_mat\0" || disabled() {
        return false;
    }
    let tex = string_at(system, material + 0x20);
    tex.starts_with("acc_Facemyunit") && tex.ends_with("_2_cl0n_P")
}

/// RenderState::Change's bind table for the projection (setup.rs
/// RENDERSTATE_PROJ_BIND): the uniform register is its byte +2.
const RENDERSTATE_PROJ_BIND: u32 = 0x001A_3340;

/// The glasses' draw with its depth pulled toward the camera. A point p in
/// view space moved to s * p (s < 1) stays at the same place on the screen
/// (x, y and w all scale by s when the projection's w has no constant
/// term) and its depth z/w becomes (P2 . p + P23 / s) / w: so only P23
/// changes. s = 1 - pull / distance, the distance being the head bone's
/// (the palette's translation) from the camera, and pull the head units
/// above times the palette's scale. Returns (distance, s), or None (an
/// orthographic projection, or no matrices) leaving the setup as it was.
pub fn pull(system: &mut System, s: &mut Setup, o: &Objects) -> Option<(f32, f32)> {
    let proj_bind = prim::r32(system, RENDERSTATE_PROJ_BIND);
    let proj = prim::r8(system, proj_bind + 2) as usize;
    let view = prim::r8(system, o.bind + 10) as usize;
    let palette = prim::r8(system, o.bind + 0x22) as usize;
    if [proj + 3, view + 2, palette + 2].iter().any(|&i| i >= s.floats.len()) {
        return None;
    }
    let p3 = s.floats[proj + 3];
    if p3[3].abs() > 1e-6 || p3[..3].iter().all(|v| v.abs() < 1e-9) {
        return None; // orthographic
    }
    let row = |m: usize, i: usize| s.floats[m + i];
    // the head bone's position and the palette's scale, in view space
    let t = [row(palette, 0)[3], row(palette, 1)[3], row(palette, 2)[3], 1.0];
    let col0 = [row(palette, 0)[0], row(palette, 1)[0], row(palette, 2)[0], 0.0];
    let in_view = |v: [f32; 4]| -> [f32; 3] {
        let r = |i: usize| {
            let m = row(view, i);
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2] + m[3] * v[3]
        };
        [r(0), r(1), r(2)]
    };
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let distance = len(in_view(t));
    let scale = len(in_view(col0));
    let amount = crate::perf::var("FATES_SUNGLASSES_PULL").ok().and_then(|v| v.parse().ok()).unwrap_or(PULL) * scale;
    if !(distance.is_finite() && amount.is_finite()) || distance <= amount * 2.0 {
        return None;
    }
    let k = 1.0 - amount / distance;
    s.floats[proj + 2][3] /= k;
    if crate::perf::var_os("FATES_SUNGLASSES_LOG").is_some() {
        eprintln!("sunglasses: 3D pull {amount:.3} at distance {distance:.2} (s {k:.4})");
    }
    Some((distance, k))
}

/// A string an H3D resource points to (relocated), up to 32 bytes.
fn string_at(system: &mut System, ptr: u32) -> String {
    let at = prim::r32(system, ptr);
    if at == 0 || !system.memory.is_readable(at, 32) {
        return String::new();
    }
    let mut b = [0u8; 32];
    prim::rd(system, at, &mut b);
    let end = b.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// An H3DMaterial's name (+0x28; SPICA H3DMaterial: parameters, three
/// texture command lists, three texture names, the name: 0x2C bytes).
pub fn material_name(system: &mut System, material: u32) -> String {
    string_at(system, material + 0x28)
}

/// A texture unit's registers: border colour, size, parameters, level of
/// detail, address, format (unit 0's format is 0x8E, past its cube faces).
pub fn unit_regs(s: &Setup, unit: usize) -> [u32; 6] {
    let regs = |r: usize| s.regs.get(r).copied().unwrap_or(0);
    match unit {
        0 => [regs(0x81), regs(0x82), regs(0x83), regs(0x84), regs(0x85), regs(0x8E)],
        _ => {
            let base = if unit == 1 { 0x91 } else { 0x99 };
            [regs(base), regs(base + 1), regs(base + 2), regs(base + 3), regs(base + 4), regs(base + 5)]
        }
    }
}

/// The cut-in's face material without the Sunglasses layer: its unit reads
/// only the layer's first 8x8 tile, which is fully transparent (make_art.py
/// keeps the corner clear; dev/sunglasses/check_marks.py checks it), so the
/// face comes out alone and over_hair lays the layer down once.
pub fn without_layer(s: &Setup, unit: usize) -> Setup {
    let mut s2 = s.clone();
    let (size, lod) = [(0x82usize, 0x84usize), (0x92, 0x94), (0x9A, 0x9C)][unit.min(2)];
    if lod < s2.regs.len() {
        s2.regs[size] = 8 << 16 | 8;
        s2.regs[lod] = 0;
    }
    s2
}

/// The critical cut-in (effect/Tlp_CutIn) composes the face and the facial
/// detail in face_mat and lays hair_mat over them, so a detail is under the
/// hair. For Haiku's Sunglasses the hair material's geometry (laid out like
/// the face's: the 512x256 layer and hair over both halves of the 512x512
/// cut-in) is drawn once more with the layer on unit 0 (`unit`: the face
/// material's registers for it, unit_regs) and a combiner that outputs it as
/// it is, with no depth test against the hair at the same depth: the
/// glasses over the hair.
pub fn over_hair(s: &Setup, unit: [u32; 6]) -> Setup {
    let mut s2 = s.clone();
    let mut set = |r: usize, v: u32| {
        if r < s2.regs.len() {
            s2.regs[r] = v;
        }
    };
    for (r, v) in [0x81usize, 0x82, 0x83, 0x84, 0x85, 0x8E].into_iter().zip(unit) {
        set(r, v);
    }
    // the texture unit config: unit 0 on, units 1 and 2 off
    let config = s.regs.get(0x80).copied().unwrap_or(0);
    set(0x80, (config & !0x7) | 0x1);
    // stage 0: colour and alpha of texture 0; the rest pass it on
    set(0xC0, 0x3 | 0x3 << 16);
    set(0xC1, 0);
    set(0xC2, 0);
    set(0xC4, 0);
    for st in [0xC8usize, 0xD0, 0xD8, 0xF0, 0xF8] {
        set(st, 0xF | 0xF << 16);
        set(st + 1, 0);
        set(st + 2, 0);
        set(st + 4, 0);
    }
    // depth: always passes, and is not written
    let depth = s.regs.get(0x107).copied().unwrap_or(0);
    set(0x107, (depth & !0x70 & !0x1000) | 0x10);
    s2
}
