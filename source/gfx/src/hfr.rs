//! gfx-hfr: pictures in between the native renderer's frames.
//!
//! Above the game's own rate the frontend shows pictures between two of the
//! game's frames (zakuro's hfr.rs and System::pace). With the emulated GPU
//! those are drawn by zakuro-gpu's `interpolation`: each PICA draw is
//! matched with the frame before's and drawn again with its vertex
//! uniforms part of the way between. The native renderer does the same with
//! its own draw lists, which keep the game's transforms:
//!
//! - a 3D draw (`scene::Shaded`, H3D meshes and nw::eft particles) runs the
//!   game's vertex program with its 96 float uniforms (matrices, bone
//!   palettes, the camera): the uniforms that place the vertices move;
//! - a 2D prim draw has its world and view-projection matrices;
//! - a post effect is run again as it is, over the moved picture.
//!
//! When a frame is complete (`track`), each draw is matched with the draw
//! of the display's frame before that is drawn the same way (`key`) and
//! whose placing uniforms are nearest, around its own place among them, by
//! the same rules as zakuro-gpu's interpolation (what moves, what jumps,
//! what keeps jumping). A picture in between at `t` (`draws_at`) is the
//! frame's draws with what moved placed `t` of the way from the frame
//! before's. The renderer draws it on demand, into a target of its own
//! (render.rs `render_between`), so the frames' own chain is untouched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::prim::Draw;
use crate::scene::Shaded;

/// a component's move differing from its move the frame before by more
/// than this (or this part of its size) is a jump, not a move. As
/// zakuro-gpu's interpolation.
const JUMP: f32 = 0.5;
const JUMP_PART: f32 = 0.25;
/// gfx-models: a position (a matrix's translation, a vertex) that starts
/// or speeds up by at most this much in a frame moves: the rule above
/// counted the first frames of every slide from rest (a level-up's text
/// and portrait, a banner, a window) as a jump, so they were held for the
/// game's whole frame, then jumped (judder). Components of a matrix's
/// rotation part (numbers of at most MOST_TURN) keep the rule above, which
/// is what keeps a camera cut from being drawn part of the way.
const MOST_MOVE: f32 = 64.0;
const MOST_TURN: f32 = 2.0;

/// gfx-models: see MOST_MOVE; FATES_GFX_HFR_SLIDE=0 keeps the rule above
/// alone.
fn slides(change: f32, before: f32, now: f32) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_SLIDE").map_or(true, |v| v != "0"));
    on && change <= MOST_MOVE && before.abs().max(now.abs()) > MOST_TURN
}

/// how many draws drawn the same way a draw's match is looked for among.
const NEAREST: usize = 32;
/// how many frames after a jump another jump counts as more of the same.
const RESTLESS: u8 = 4;
/// float components of a shaded draw's uniforms (96 vec4).
const SHADED_FLOATS: usize = 96 * 4;
/// a prim draw's: world (12) and view-projection (16).
const PRIM_FLOATS: usize = 28;

/// how the draws of a frame matched the frame before's, as
/// zakuro_gpu::Matches counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Matches {
    pub draws: u32,
    pub matched: u32,
    pub moved: u32,
    pub jumped: u32,
    pub jumping: u32,
}

impl Matches {
    /// whether pictures in between look right: most draws were there the
    /// frame before, and few that had not jumped lately jumped now.
    pub fn smooth(&self) -> bool {
        self.moved > 0 && self.matched * 2 >= self.draws && self.jumped * 4 <= self.matched
    }

    pub fn as_zakuro(&self) -> zakuro_gpu::Matches {
        zakuro_gpu::Matches { draws: self.draws, matched: self.matched, moved: self.moved, jumped: self.jumped, jumping: self.jumping }
    }
}

/// one draw of a frame against its match in the frame before.
#[derive(Clone, Default)]
pub struct Track {
    /// the placing components that changed, with the value the match had,
    /// and whether the picture in between moves it (a component the
    /// textures' coordinates read too that jumped stays as it is now)
    pub(crate) changed: Vec<(u16, f32, bool)>, // castlecam: pub(crate)
    /// drawn moved in pictures in between
    pub moves: bool,
    /// frames left during which a jump counts as one more of its jumps
    restless: u8,
    /// gfx-final: a 2D draw's look and vertex data against its match's
    look: Option<Arc<Look>>,
}

/// gfx-final: what a 2D prim draw changed besides its matrices, which
/// pictures in between place part of the way too:
/// - its colour uniform and combiner constants (fades: a full-screen quad
///   whose colour's alpha steps from frame to frame);
/// - its vertices, when the game moved the element by rewriting them
///   rather than its matrices (scrolling text, sliding panels): only a
///   rigid move (every vertex by the same amount, texture coordinates
///   unchanged) is placed in between, a reshaped element (a gauge filling,
///   other glyphs) is drawn as it is now;
/// - its vertex colours (vertex-coloured fades), with positions unchanged
///   or rigidly moved.
/// A change bigger than JUMP_LOOK (a flash, a new colour), a slide of more
/// than MOST_SLIDE, or a move that differs from the frame before's by more
/// than the matrices' jump rule allows stays as it is now.
#[derive(Clone, Default)]
pub struct Look {
    color: Option<[f32; 4]>,
    /// gfx-models: the alpha test's reference as the match had it, when it
    /// changed with the colours (a pulsing highlight keeps alpha >= its
    /// reference: fading one alone failed the test, and every picture in
    /// between drew the menu's highlight off, a flash)
    alpha_ref: Option<u8>,
    konst: [Option<[u8; 4]>; 6],
    /// the match's vertices, and whether positions / colours move
    vertices: Option<Arc<Vec<u8>>>,
    positions: bool,
    colors: bool,
    /// the rigid move this frame (now - before), for the next frame's jump rule
    shift: [f32; 3],
    /// gfxmodels3: the texture coordinates move too (the same art, cut
    /// elsewhere: see art_shift)
    uvs: bool,
}

/// the most a colour or constant changes in a frame and still fades (as
/// a fraction of full scale); more is a cut.
const JUMP_LOOK: f32 = 0.5;
/// the most a 2D element moves in a frame (framebuffer pixels) and still
/// slides.
const MOST_SLIDE: f32 = 64.0;

fn vertex_pos(v: &[u8], k: usize) -> [f32; 3] {
    std::array::from_fn(|c| f32::from_le_bytes(v[k * 24 + c * 4..k * 24 + c * 4 + 4].try_into().unwrap()))
}

/// gfxmodels3: a vertex's texture coordinates.
fn vertex_uv(v: &[u8], k: usize) -> [f32; 2] {
    std::array::from_fn(|c| f32::from_le_bytes(v[k * 24 + 12 + c * 4..k * 24 + 16 + c * 4].try_into().unwrap()))
}

/// gfxmodels3: FATES_GFX_HFR_ART=0 turns art_shift off.
fn follows_art() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_ART").map_or(true, |v| v != "0"))
}

/// gfxmodels3: the move of a 2D element whose art is cut elsewhere than the
/// frame before's, with its texture coordinates changed with the cut: the
/// portraits' fade strips in the wide sides (portrait.rs), whose split
/// slides along the art while the portrait slides through the side.
/// Each quad of the frame before maps its positions to the art (an affine
/// map, from its three vertices spanning the most area; a quad of no area
/// takes its neighbour's). When every vertex of the frame reads the art
/// where that map, moved by one and the same shift, puts it, that shift is
/// the element's move: moving each vertex's position and texture
/// coordinates part of the way then shows the art moved part of the way,
/// cut where the in-between places put the cut. None otherwise (a
/// flip-book frame, other art), which keeps the old rule: no move.
fn art_shift(bv: &[u8], nv: &[u8]) -> Option<[f32; 3]> {
    let n = nv.len() / 24;
    if n == 0 || n % 4 != 0 || bv.len() != nv.len() {
        return None;
    }
    // per quad: uv = m * (x, y) + off, and m's inverse
    type Map = ([[f32; 2]; 2], [f32; 2], [[f32; 2]; 2]);
    let fit = |q: usize| -> Option<Map> {
        let p: Vec<[f32; 3]> = (0..4).map(|k| vertex_pos(bv, q * 4 + k)).collect();
        let u: Vec<[f32; 2]> = (0..4).map(|k| vertex_uv(bv, q * 4 + k)).collect();
        let mut best = (0.0f32, [0usize, 1, 2]);
        for tri in [[0usize, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]] {
            let [a, b, c] = tri;
            let area = ((p[b][0] - p[a][0]) * (p[c][1] - p[a][1]) - (p[c][0] - p[a][0]) * (p[b][1] - p[a][1])).abs();
            if area > best.0 {
                best = (area, tri);
            }
        }
        if best.0 < 1e-3 {
            return None;
        }
        let [a, b, c] = best.1;
        let (dx1, dy1, dx2, dy2) = (p[b][0] - p[a][0], p[b][1] - p[a][1], p[c][0] - p[a][0], p[c][1] - p[a][1]);
        let det = dx1 * dy2 - dx2 * dy1;
        let mut m = [[0.0f32; 2]; 2];
        for r in 0..2 {
            let (du1, du2) = (u[b][r] - u[a][r], u[c][r] - u[a][r]);
            m[r][0] = (du1 * dy2 - du2 * dy1) / det;
            m[r][1] = (dx1 * du2 - dx2 * du1) / det;
        }
        let mdet = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if !mdet.is_finite() || mdet.abs() < 1e-9 {
            return None;
        }
        let inv = [[m[1][1] / mdet, -m[0][1] / mdet], [-m[1][0] / mdet, m[0][0] / mdet]];
        let off = [u[a][0] - m[0][0] * p[a][0] - m[0][1] * p[a][1], u[a][1] - m[1][0] * p[a][0] - m[1][1] * p[a][1]];
        // every vertex on the same map: a flat quad of one art
        for k in 0..4 {
            for r in 0..2 {
                let at = m[r][0] * p[k][0] + m[r][1] * p[k][1] + off[r];
                if (at - u[k][r]).abs() > 0.01 * (1.0 + u[k][r].abs()) {
                    return None;
                }
            }
        }
        Some((m, off, inv))
    };
    let quads = n / 4;
    let maps: Vec<Option<Map>> = (0..quads).map(fit).collect();
    let mut shift: Option<[f32; 3]> = None;
    let mut anchored = false;
    for q in 0..quads {
        // a quad of no area (a strip with nothing to fade) reads the art
        // with its nearest neighbour's map
        let map = maps[q].or_else(|| (1..quads).find_map(|d| q.checked_sub(d).and_then(|i| maps[i]).or_else(|| maps.get(q + d).copied().flatten())))?;
        let (_, off, inv) = map;
        for k in q * 4..q * 4 + 4 {
            let (pb, pn, un) = (vertex_pos(bv, k), vertex_pos(nv, k), vertex_uv(nv, k));
            // where the frame before's art had what this vertex reads
            let d = [un[0] - off[0], un[1] - off[1]];
            let was = [inv[0][0] * d[0] + inv[0][1] * d[1], inv[1][0] * d[0] + inv[1][1] * d[1]];
            let s = [pn[0] - was[0], pn[1] - was[1], pn[2] - pb[2]];
            match shift {
                None => shift = Some(s),
                Some(t) if (0..3).all(|c| (t[c] - s[c]).abs() <= 0.05) => {}
                Some(_) => return None,
            }
            // a corner of the art: the same texture coordinates, moved
            anchored |= vertex_uv(bv, k) == un && pn != pb;
        }
    }
    // a flip-book frame (the same quad reading another cell) reads the art
    // at one shift as well, but none of its corners keeps its texture
    // coordinates: it did not move
    // gfxmap: a window opening or closing over art that stays where it is
    // (the main menu's parchment: a bigger or smaller cut of it each frame,
    // its corners 42 x 23 pixels apart a frame) reads the art at no shift
    // with its corners moved: its corners and their cuts move part of the
    // way (the window opens in between too, not in 60 Hz steps)
    let cropped = crops() && shift.is_some_and(|s| s[0].abs() <= 0.05 && s[1].abs() <= 0.05) && (0..n).any(|k| vertex_pos(bv, k) != vertex_pos(nv, k));
    shift.filter(|_| anchored || cropped)
}

/// gfx-final: a 2D draw's changes against its match, see Look; None when
/// nothing changed that pictures in between place.
fn look(before: &Draw, now: &Draw, shift_before: Option<[f32; 3]>) -> Option<Look> {
    let mut l = Look::default();
    let mut any = false;
    // gfx-2d, gfx-models: both fixed the pulsing highlight; the reference fades with the colours below
    if before.color != now.color && before.color.iter().zip(now.color.iter()).all(|(a, b)| (a - b).abs() <= JUMP_LOOK) {
        l.color = Some(before.color);
        any = true;
    }
    for (i, (b, n)) in before.combiner.stages.iter().zip(now.combiner.stages.iter()).enumerate() {
        if b.konst != n.konst && b.konst.iter().zip(n.konst.iter()).all(|(&x, &y)| ((x as f32 - y as f32) / 255.0).abs() <= JUMP_LOOK) {
            l.konst[i] = Some(b.konst);
            any = true;
        }
    }
    // gfx-models: the alpha test's reference fades with the colours
    if now.alpha_test && before.alpha_test && before.alpha_func == now.alpha_func && before.alpha_ref != now.alpha_ref {
        if (before.alpha_ref as f32 - now.alpha_ref as f32).abs() / 255.0 <= JUMP_LOOK {
            l.alpha_ref = Some(before.alpha_ref);
            any = true;
        } else {
            // a cut of the test: no colour of it fades
            l.color = None;
            l.konst = [None; 6];
            return look_positions_only(before, now, shift_before);
        }
    }
    let (bv, nv) = (&before.vertices, &now.vertices);
    if bv.len() == nv.len() && !nv.is_empty() && nv.len() % 24 == 0 && bv != nv {
        let n = nv.len() / 24;
        // texture coordinates as they were: the same element
        let same_uv = (0..n).all(|k| bv[k * 24 + 12..k * 24 + 20] == nv[k * 24 + 12..k * 24 + 20]);
        let (p0, q0) = (vertex_pos(bv, 0), vertex_pos(nv, 0));
        let shift = [q0[0] - p0[0], q0[1] - p0[1], q0[2] - p0[2]];
        let rigid = (0..n).all(|k| {
            let (p, q) = (vertex_pos(bv, k), vertex_pos(nv, k));
            (0..3).all(|c| ((q[c] - p[c]) - shift[c]).abs() <= 1e-3)
        });
        let moved = shift.iter().any(|&d| d != 0.0);
        let slides = shift.iter().all(|d| d.abs() <= MOST_SLIDE) && {
            // the matrices' rule: a move that differs from the frame
            // before's by more than half a pixel or a quarter of where it
            // is jumped
            let prev = shift_before.unwrap_or([0.0; 3]);
            // gfxmodels3: a 2D vertex is a place, never a rotation: the
            // element's place (its vertex furthest out) for the slide rule;
            // a slide from rest at x = 0..2 (the level-up portrait leaving)
            // counted as rotation-sized, and its first step was held
            let place: [f32; 3] = std::array::from_fn(|c| (0..n).map(|k| vertex_pos(bv, k)[c].abs().max(vertex_pos(nv, k)[c].abs())).fold(0.0, f32::max));
            (0..3).all(|c| {
                let change = (shift[c] - prev[c]).abs();
                change <= JUMP || change <= JUMP_PART * p0[c].abs().max(q0[c].abs()) || slides(change, place[c], place[c])
            })
        };
        let colors_differ = (0..n).any(|k| bv[k * 24 + 20..k * 24 + 24] != nv[k * 24 + 20..k * 24 + 24]);
        let colors_fade = (0..n).all(|k| (20..24).all(|i| ((bv[k * 24 + i] as f32 - nv[k * 24 + i] as f32) / 255.0).abs() <= JUMP_LOOK));
        // gfx-models: an element reshaped with its texture as it was (a
        // window opening or closing: its frame's sides move apart, a gauge
        // filling) moves each vertex part of the way too, when none moves
        // further than an element slides; only a rigid move had before, so
        // windows opened in steps at the game's rate
        let stretches = !rigid
            && reshapes()
            && (0..n).all(|k| {
                let (p, q) = (vertex_pos(bv, k), vertex_pos(nv, k));
                (0..3).all(|c| (q[c] - p[c]).abs() <= MOST_SLIDE)
            });
        if same_uv && (rigid && (!moved || slides) || stretches) {
            l.positions = moved || stretches;
            l.colors = colors_differ && colors_fade;
            l.shift = shift;
            if l.positions || l.colors {
                l.vertices = Some(Arc::new(bv.clone()));
                any = true;
            }
        } else if !same_uv && follows_art() {
            // gfxmodels3: the same art cut elsewhere (a portrait's fade
            // strip in the wide side): it slides by the art's own move
            if let Some(art) = art_shift(bv, nv) {
                let prev = shift_before.unwrap_or([0.0; 3]);
                // where the element is (its vertex furthest out, not its
                // first, which a cut can put at 0), for the jump rule
                let place: [f32; 3] = std::array::from_fn(|c| (0..n).map(|k| vertex_pos(bv, k)[c].abs().max(vertex_pos(nv, k)[c].abs())).fold(0.0, f32::max));
                let art_slides = art.iter().all(|d| d.abs() <= MOST_SLIDE)
                    && (0..3).all(|c| {
                        let change = (art[c] - prev[c]).abs();
                        change <= JUMP || change <= JUMP_PART * place[c] || self::slides(change, place[c], place[c])
                    });
                // (a cut's vertices move along the art further than the art
                // moves: the art's move alone says whether it slides)
                if art_slides {
                    l.positions = true;
                    l.uvs = true;
                    l.colors = colors_differ && colors_fade;
                    l.shift = art;
                    l.vertices = Some(Arc::new(bv.clone()));
                    any = true;
                }
            }
        }
    }
    any.then_some(l)
}

/// gfx-models: look() for a draw whose alpha test changed by a cut: its
/// vertices may still move, its colours do not fade.
fn look_positions_only(before: &Draw, now: &Draw, shift_before: Option<[f32; 3]>) -> Option<Look> {
    let mut b = before.clone();
    b.color = now.color;
    b.combiner = now.combiner;
    b.alpha_ref = now.alpha_ref;
    b.alpha_func = now.alpha_func;
    b.alpha_test = now.alpha_test;
    // the vertices' colours as now, so only positions can differ
    if b.vertices.len() == now.vertices.len() && now.vertices.len() % 24 == 0 {
        for k in 0..now.vertices.len() / 24 {
            b.vertices[k * 24 + 20..k * 24 + 24].copy_from_slice(&now.vertices[k * 24 + 20..k * 24 + 24]);
        }
    }
    look(&b, now, shift_before)
}

/// a frame's draws against the frame before's.
#[derive(Default)]
pub struct Between {
    pub tracks: Vec<Track>,
    pub matches: Matches,
    /// smooth2: the frame's post effects read earlier frames (carries): it
    /// has no picture in between, the frontend cross-fades it instead
    pub carried: bool,
    /// gfxleft: models of the frame before gone now (see track): the frame
    /// is held, shown as it is
    pub vanished: u32,
    /// gfxleft: the share of the screen the models gone covered
    pub vanished_share: f32,
    /// gfxleft: and the models that jumped
    pub jumped_share: f32,
    /// castlecam: models of the frame before the camera's turn took out of
    /// the view, drawn in the pictures in between (castlecam.rs)
    pub ghosts: Vec<crate::castlecam::Ghost>,
}

impl Track {
    /// castlecam: a track that moves the components listed (see castlecam.rs)
    pub(crate) fn moving(changed: Vec<(u16, f32, bool)>) -> Track {
        Track { changed, moves: true, restless: 0, look: None }
    }
}

impl Between {
    pub fn moves(&self) -> bool {
        self.tracks.iter().any(|t| t.moves)
    }
}

fn mix(hash: u64, word: u64) -> u64 {
    (hash.rotate_left(5) ^ word).wrapping_mul(0x517C_C1B7_2722_0A95)
}

/// what a draw draws, the same from frame to frame for a model drawn the
/// same way; none for one never moved (post effects).
fn key(d: &Draw) -> Option<u64> {
    if d.post.is_some() {
        return None;
    }
    let tex = |t: &Option<(crate::prim::TexRef, crate::prim::Sampling)>| t.map_or(0, |(t, _)| t.addr as u64 | (t.width as u64) << 32 | (t.height as u64) << 48);
    let mut h = 0x9E37_79B9_7F4A_7C15u64;
    match d.shaded.as_deref() {
        Some(s) => {
            h = mix(h, 1);
            h = mix(h, Arc::as_ptr(&s.program) as u64);
            h = mix(h, s.shading.get(401).copied().unwrap_or(0) as u64); // the entry point
            if !s.loose {
                h = mix(h, s.vertices.len() as u64);
                h = mix(h, s.indices.len() as u64);
            }
            h = mix(h, tex(&s.textures[0]));
            h = mix(h, s.key.blend.unwrap_or(u32::MAX) as u64);
            h = mix(h, (s.key.mask as u64) | (s.key.depth_test as u64) << 8 | (s.key.depth_write as u64) << 9 | (s.key.topology as u64) << 10);
            for v in s.viewport {
                h = mix(h, v as u32 as u64);
            }
        }
        None => {
            h = mix(h, 2);
            h = mix(h, d.vertices.len() as u64);
            h = mix(h, d.indices.len() as u64);
            h = mix(h, tex(&d.textures[0]));
            h = mix(h, u32::from_le_bytes(d.blend.func) as u64 | (d.blend.enable as u64) << 32 | (d.depth_test as u64) << 33 | (d.depth_write as u64) << 34);
            for v in d.viewport {
                h = mix(h, v as u32 as u64);
            }
        }
    }
    Some(h)
}

/// mapjit: dev, `key` for mapjit.rs's log.
pub(crate) fn draw_key(d: &Draw) -> Option<u64> {
    key(d)
}

/// a draw's uniforms as floats, and which of them place the vertices (as
/// vec4 bits) and which the textures' coordinates can read too.
struct View<'a> {
    shaded: Option<&'a Shaded>,
    draw: &'a Draw,
}

impl View<'_> {
    fn new(d: &Draw) -> View<'_> {
        View { shaded: d.shaded.as_deref(), draw: d }
    }
    fn len(&self) -> usize {
        if self.shaded.is_some() { SHADED_FLOATS } else { PRIM_FLOATS }
    }
    #[inline]
    fn get(&self, i: usize) -> f32 {
        match self.shaded {
            Some(s) => f32::from_bits(s.shading[i]),
            None if i < 12 => self.draw.world[i],
            None => self.draw.view_proj[i - 12],
        }
    }
    fn placing(&self) -> (u128, u128) {
        match self.shaded {
            Some(s) => s.placing,
            None => ((1 << (PRIM_FLOATS / 4)) - 1, 0),
        }
    }
}

/// whether a component steps between round values (sixty-fourths), as a
/// sprite's offset into its sheet does, rather than moves.
fn steps(before: f32, now: f32) -> bool {
    let round = |v: f32| (v * 64.0).fract() == 0.0;
    round(before) && round(now)
}

/// matches each draw of `now` with one of `before`, the frame before on the
/// same display (`before_between` its own matches), and says how each one
/// moves.
/// cameras3: dev, FATES_GFX_HFR_TRACE=2 also names each draw of a frame that
/// matched none of the frame before's (what appears: its texture, sizes)
fn trace_new(n: usize, d: &Draw) {
    if !crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "2") {
        return;
    }
    let (tex, verts, indices) = match &d.shaded {
        Some(s) => (s.textures[0].map(|t| (t.0.addr, t.0.width, t.0.height)), s.vertices.len(), s.indices.len()),
        None => (d.textures[0].map(|t| (t.0.addr, t.0.width, t.0.height)), d.vertices.len(), d.indices.len()),
    };
    eprintln!("gfx-hfr: new draw {n} {} tex {tex:x?} vertex bytes {verts} indices {indices}", if d.shaded.is_some() { "mesh" } else { "prim" });
}

pub fn track(before: Option<(&[Draw], Option<&Between>)>, now: &[Draw]) -> Between {
    let mut out = Between { tracks: vec![Track::default(); now.len()], matches: Matches::default(), carried: false, vanished: 0, vanished_share: 0.0, jumped_share: 0.0, ghosts: Vec::new() };
    out.matches.draws = now.len() as u32;
    let Some((before, before_between)) = before else { return out };
    // the frame before's draws by key, in order
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, d) in before.iter().enumerate() {
        if let Some(k) = key(d) {
            buckets.entry(k).or_default().push(i);
        }
    }
    let mut taken = vec![false; before.len()];
    let mut places: HashMap<u64, usize> = HashMap::new();
    // gfxleft: dev, FATES_GFX_HFR_LIST=frame: every draw of that headless frame with its match
    let listing = list_frame();
    let mut matched_to: Vec<Option<usize>> = vec![None; now.len()];
    let mut jumped_draws: Vec<usize> = Vec::new();
    // gfxmap: each particle draw's own rows (see particle_rows)
    let own_rows = particle_rows(before, now);
    let instance_rows = mesh_rows(now);
    // mapjit: the copies of a mesh drawn many times paired as a whole
    let (paired, paired_new) = pair_copies(before, now, &buckets, &instance_rows);
    for (n, d) in now.iter().enumerate() {
        let Some(k) = key(d) else { continue };
        let place = {
            let p = places.entry(k).or_insert(0);
            *p += 1;
            *p - 1
        };
        let Some(bucket) = buckets.get(&k) else {
            trace_new(n, d); // cameras3
            continue;
        };
        // mapjit: a copy pair_copies found new takes no other copy's match
        if paired_new[n] {
            trace_new(n, d);
            continue;
        }
        let view = View::new(d);
        let (placing, shared) = view.placing();
        let distance = |b: usize| {
            let bv = View::new(&before[b]);
            let mut total = 0.0f32;
            let mut bits = placing;
            while bits != 0 {
                let u = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                for c in 0..4 {
                    let i = u * 4 + c;
                    if i < view.len() {
                        total += (bv.get(i) - view.get(i)).abs();
                    }
                }
            }
            // gfx-models: 2D draws drawn the same way with the same matrices
            // (a menu's boxes, placed by their vertices) are told apart by
            // where their vertices are and their colours: by matrices alone
            // they all tied, and the order decided, so one extra box before
            // the cursor's highlight matched it with a plain box, and every
            // picture in between drew the highlight off (menus flashed)
            if d.shaded.is_none() && distinguishes() {
                total += prim_distance(&before[b], d);
            }
            if total.is_nan() { f32::INFINITY } else { total }
        };
        // the nearest not taken, around its own place
        let count = bucket.len();
        // gfxmap: a mesh drawn many times (the map's tree clumps, one draw
        // each, 60-140 of them) is looked for among all of them: scrolling,
        // clumps leave and enter the view at its edges and the list shifts
        // by more than NEAREST / 2, so a clump matched another clump
        // (another place: a jump) and frames were held while the map scrolled
        let wide = d.shaded.as_deref().is_some_and(|s| !s.loose) && count <= MOST_SEARCHED && searches_wide();
        let around = if count <= NEAREST || wide { 0..count } else { place.saturating_sub(NEAREST / 2)..(place + NEAREST / 2).min(count) };
        let own = (place < count && !taken[bucket[place]]).then(|| (place, distance(bucket[place])));
        // mapjit: the pair pair_copies made
        let pre = paired[n].and_then(|b| bucket.iter().position(|&x| x == b)).filter(|&i| !taken[bucket[i]]);
        let found = if pre.is_some() {
            pre
        } else if own.is_some_and(|(_, d)| d == 0.0) {
            own.map(|(p, _)| p)
        } else {
            let others = around.filter(|&i| i != place && !taken[bucket[i]]).map(|i| (i, distance(bucket[i])));
            own.into_iter().chain(others).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
        };
        let Some(found) = found else {
            trace_new(n, d); // cameras3
            continue;
        };
        let b = bucket[found];
        taken[b] = true;
        matched_to[n] = Some(b);
        out.matches.matched += 1;
        let bv = View::new(&before[b]);
        let before_track = before_between.and_then(|bb| bb.tracks.get(b));
        let restless = before_track.map_or(0, |t| t.restless);
        // how far each component moved the frame before
        let moved_before = |i: usize| -> f32 {
            match before_track {
                // gfxmodels3: a jump is no speed: the move after one (an
                // animation's snap, then its settling) is judged from rest
                Some(t) => t.changed.iter().find(|c| c.0 as usize == i).map_or(0.0, |c| bv.get(i) - c.1),
                None => 0.0,
            }
        };
        let track = &mut out.tracks[n];
        track.restless = restless.saturating_sub(1);
        let placed = |u: usize| (placing | shared) & (1 << u) != 0;
        let is_shared = |u: usize| shared & (1 << u) != 0;
        let mut jumped = false;
        let mut other_particle = false; // gfxmap
        let mut lerped = false;
        // gfxmodels3: a model's rigid matrix turning fast about a pivot near
        // it turns (shaded_at slerps it) rather than counting as a jump
        let turning = if view.shaded.is_some() { turns(&bv, &view, placing | shared) } else { 0 };
        let mut bits = placing | shared;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if !placed(u) {
                continue;
            }
            let turns_here = turning & (1 << u) != 0;
            let mut moving = [false; 4];
            let mut jumping = false;
            for c in 0..4 {
                let i = u * 4 + c;
                if i >= view.len() {
                    continue;
                }
                let (bf, nw) = (bv.get(i), view.get(i));
                if bf == nw || (is_shared(u) && steps(bf, nw)) {
                    continue;
                }
                moving[c] = true;
                let change = (nw - bf - moved_before(i)).abs();
                // gfxmap: a particle's own row (its colour, size, place)
                // keeps going as it went or is another particle
                let particle = own_rows[n] & (1 << u) != 0;
                let ok = |change: f32| {
                    if particle {
                        return change <= JUMP || change <= PARTICLE_PART * bf.abs().max(nw.abs());
                    }
                    // gfxmap: a texture matrix (rows the textures read) is
                    // "rigid" when it only scrolls: its translation (w)
                    // keeps the rules above, so a scroll that wraps (the map's
                    // tile highlight, -0.74 to 0 every 90 frames) is not
                    // drawn through the whole texture, a flash, in between
                    let turns_here = turns_here && !(c == 3 && is_shared(u) && wraps_kept());
                    change <= JUMP || change <= JUMP_PART * bf.abs().max(nw.abs()) || slides(change, bf, nw) || turns_here
                };
                // gfxmodels3: after a jump, a move judged from rest as well
                // (an animation that snapped, then settles); going on as
                // the jump went still moves too
                let after_jump = after_jump_from_rest() && before_track.is_some_and(|t| t.changed.iter().any(|c| c.0 as usize == i && !c.2));
                if !(ok(change) || after_jump && ok((nw - bf).abs())) {
                    jumping = true;
                }
            }
            if !moving.contains(&true) {
                continue;
            }
            // vein2: a particle row that jumped is no jump of the draw (it
            // is another particle's, below): before, the whole draw was then
            // drawn as it is now, its camera rows too, so the rule below
            // never kept the camera moving (a respawning vein stood still for
            // a frame while the map scrolled)
            let particle_row = own_rows[n] & (1 << u) != 0;
            if jumping && !is_shared(u) && !(particle_row && lone_particles()) {
                jumped = true;
            }
            // gfxmap: a particle row that jumped is another particle's
            if jumping && particle_row {
                other_particle = true;
            }
            for (c, &m) in moving.iter().enumerate() {
                if m {
                    let i = u * 4 + c;
                    track.changed.push((i as u16, bv.get(i), !jumping));
                }
            }
            lerped |= !jumping;
        }
        // gfxmap: the match holds other particles (see particle_rows): every
        // row of the particles is drawn as it is now, the camera's still
        // moves (a vein's rings stay with the scrolling map)
        if other_particle && !jumped {
            for c in track.changed.iter_mut() {
                if own_rows[n] & (1 << (c.0 / 4)) != 0 {
                    c.2 = false;
                }
            }
            lerped = track.changed.iter().any(|c| c.2);
        }
        // gfxmap: a mesh drawn many times whose own rows (its place: rows
        // that differ between its copies) jumped matched another copy (a
        // tree clump leaving the view while this one enters): it is new, and
        // the copy it took is free again (see follow_camera)
        if jumped && instance_rows[n] != 0 && track.changed.iter().all(|c| c.2 || instance_rows[n] & (1 << (c.0 / 4)) != 0) {
            taken[b] = false;
            matched_to[n] = None;
            out.matches.matched -= 1;
            *track = Track::default();
            trace_new(n, d); // cameras3
            continue;
        }
        if jumped {
            jumped_draws.push(n);
            match restless {
                0 => out.matches.jumped += 1,
                _ => out.matches.jumping += 1,
            }
            track.restless = RESTLESS;
            if trace_2d() {
                let rows: Vec<String> = track.changed.iter().filter(|c| !c.2).map(|c| format!("{}:{:.3}->{:.3}", c.0, c.1, view.get(c.0 as usize))).collect();
                eprintln!("gfx-hfr 2d: draw {n} {} jumped, restless {restless}, components that jumped {}", if d.shaded.is_some() { "shaded" } else { "prim" }, rows.join(" "));
            }
            continue;
        }
        // gfx-final: a 2D draw's fades and rigid vertex moves
        if d.shaded.is_none() && looks() {
            let shift_before = before_track.and_then(|t| t.look.as_ref()).map(|l| l.shift);
            // gfx-2d: dev, FATES_GFX_HFR_DRAWS=frame: each 2D match of that headless frame
            if crate::perf::var("FATES_GFX_HFR_DRAWS").ok().and_then(|v| v.parse::<u64>().ok()) == Some(zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed)) {
                let c = |d: &Draw| d.vertices.get(20..24).map(|c| format!("{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])).unwrap_or_default();
                let uv = |d: &Draw| d.vertices.get(12..20).map(|b| (f32::from_le_bytes(b[0..4].try_into().unwrap()), f32::from_le_bytes(b[4..8].try_into().unwrap()))).unwrap_or_default();
                let l = look(&before[b], d, shift_before);
                eprintln!("gfx-2d hfr {n} <- {b} tex {:?} uv {:?}<-{:?} col #{}<-#{} color {:?}<-{:?} look {:?} at {} {} {}<-{} {} {} st0 {:?}/{:?} {:?}", d.textures[0].map(|t| t.0.addr), uv(d), uv(&before[b]), c(d), c(&before[b]), d.color, before[b].color,
                    l.as_ref().map(|l| (l.color.is_some(), l.positions, l.colors, l.konst.iter().filter(|k| k.is_some()).count())),
                    d.alpha_test, d.alpha_func, d.alpha_ref, before[b].alpha_test, before[b].alpha_func, before[b].alpha_ref, d.combiner.stages[0].src_a, d.combiner.stages[0].op_a, d.blend);
            }
            if let Some(l) = look(&before[b], d, shift_before) {
                track.look = Some(Arc::new(l));
                lerped = true;
            }
            if trace_2d() {
                trace_look(n, &before[b], d, shift_before, lerped);
            }
        }
        if lerped {
            track.moves = true;
            out.matches.moved += 1;
        }
    }
    // gfx3: a mesh drawn many times whose copies stay where they are (the
    // map's tree clumps: each copy's place, its own rows, is the same in
    // both frames; only the camera moves) has a copy whose own rows changed
    // matched another copy: the clump entering at the screen's edge took the
    // nearest clump of the frame before, 50-100 units away. That is no jump
    // by the rules above (JUMP_PART lets world places of ~1000 move by a
    // quarter), so pictures in between slid it from the other clump's place
    // to its own: trees flickered while the cursor scrolled the map. Such a
    // copy is new (see follow_camera); its match is free again.
    if instances_static() {
        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
        for n in 0..now.len() {
            if instance_rows[n] != 0 && matched_to[n].is_some() {
                if let Some(k) = key(&now[n]) {
                    groups.entry(k).or_default().push(n);
                }
            }
        }
        for ns in groups.values().filter(|ns| ns.len() >= MANY) {
            let own_moved = |n: usize| out.tracks[n].changed.iter().any(|c| instance_rows[n] & (1 << (c.0 / 4)) != 0);
            let moved: Vec<usize> = ns.iter().copied().filter(|&n| own_moved(n)).collect();
            // most copies still: a still set (scenery), not a moving crowd
            if moved.is_empty() || moved.len() * STILL_SHARE > ns.len() {
                continue;
            }
            for n in moved {
                if let Some(b) = matched_to[n].take() {
                    taken[b] = false;
                }
                out.matches.matched -= 1;
                if out.tracks[n].moves {
                    out.matches.moved -= 1;
                }
                out.tracks[n] = Track::default();
                trace_new(n, &now[n]);
            }
        }
    }
    // gfxleft: a copy, new in the frame, of a draw that matched (drawn the
    // same way, placed by the same uniforms: the crit cut-in's panel has a
    // second set of layers over the first for one frame) moves with it. As
    // a new draw it was drawn where it is now in every picture in between,
    // the panel twice, apart: a doubled, sliced panel.
    let mut copies = vec![false; now.len()];
    if holds_vanished() {
        for n in 0..now.len() {
            if matched_to[n].is_some() {
                continue;
            }
            if let Some(m) = (0..now.len()).find(|&m| m != n && matched_to[m].is_some() && same_place(&now[m], &now[n])) {
                let t = out.tracks[m].clone();
                if t.moves {
                    out.matches.moved += 1;
                }
                out.tracks[n] = t;
                copies[n] = true;
                out.matches.matched += 1;
            }
        }
    }
    // gfxmap: a new mesh moves with the camera (see follow_camera); it no
    // longer counts as new for the hold below
    if follows_camera() {
        for n in 0..now.len() {
            if matched_to[n].is_some() || copies[n] {
                continue;
            }
            if let Some(t) = follow_camera(now, &out.tracks, &matched_to, n) {
                out.tracks[n] = t;
                out.matches.moved += 1;
                copies[n] = true;
            }
        }
    }
    // castlecam: models new with the camera's turn that follow_camera could
    // not place move with the camera too (any program's camera rows)
    for (n, t) in crate::castlecam::follow_new(before, now, &out.tracks, &matched_to, &copies) {
        out.tracks[n] = t;
        out.matches.moved += 1;
        out.matches.matched += 1;
        copies[n] = true;
    }
    // castlecam: models gone with the camera's turn are drawn in between
    // moving with it, and leave nothing bare (castlecam.rs)
    let (ghosts, ghosted) = crate::castlecam::ghosts(before, now, &out.tracks, &matched_to, &taken);
    out.ghosts = ghosts;
    if listing {
        list_draws(before, now, &out, &matched_to);
    }
    // gfxleft: a model of the frame before that is gone now (not a
    // particle, whose draws come and go every frame) leaves what it covered
    // or edged bare in every picture in between, from the start: the crit
    // cut-in's lightning along the jaws ends while the panel slides out, and
    // the panel showed as a hard-edged white band the frames never show. A
    // model new in the frame is drawn where it is now in every picture in
    // between, from the start, over the rest moved part of the way: the
    // cut-in's panel has a second copy for one frame (344850), which showed
    // beside the moving panel. No picture in between of such a frame can be
    // right, so it is held. Only what covers part of the screen counts
    // (VANISHED_AREA): small models (a spark, a trail's end) come and go in
    // battles all the time.
    if holds_vanished() {
        let model = |d: &Draw| key(d).is_some() && d.shaded.as_deref().is_some_and(|s| !s.loose);
        let gone: Vec<f32> = before
            .iter()
            .enumerate()
            // (a copy gone, its draw still there, leaves nothing bare)
            .filter(|&(b, d)| !taken[b] && !ghosted[b] && model(d) && !(0..before.len()).any(|o| o != b && taken[o] && same_place(&before[o], d)))
            .chain(now.iter().enumerate().filter(|&(n, d)| matched_to[n].is_none() && !copies[n] && model(d)))
            .filter_map(|(i, d)| {
                let s = d.shaded.as_deref()?;
                let share = screen_share(s);
                if listing {
                    eprintln!("gfxleft list: gone or new {i} share {share:.3} v{} i{} tex {:?} blend {:08x} dw {} #{:016x}", s.vertices.len(), s.indices.len(), s.textures[0].map(|t| t.0.addr), s.key.blend.unwrap_or(u32::MAX), s.key.depth_write, s.shading.iter().fold(0u64, |h, &w| mix(h, w as u64)));
                }
                Some(share)
            })
            .collect();
        out.vanished_share = gone.iter().sum();
        if out.vanished_share >= VANISHED_AREA {
            out.vanished = gone.len() as u32;
        }
        // gfxleft: and a model that jumped while the rest moves part of the
        // way: the cut-in's lower lightning shrinks in one frame (344858)
        // while the jaws it edges close, and every picture in between showed
        // the panel's edge bare, a hard white band (as did the jaws' snap
        // shut, with the lightning moving towards them)
        // (an opaque model that jumps, a limb's snap, is whole at its new
        // pose: it does not count, as before)
        let opaque = |s: &Shaded| s.key.blend.map_or(true, |b| b >> 16 == 0x0101);
        let jumps: Vec<f32> = jumped_draws.iter().filter_map(|&n| now[n].shaded.as_deref().filter(|s| !s.loose && !opaque(s)).map(screen_share)).collect();
        out.jumped_share = jumps.iter().sum();
        if out.jumped_share >= JUMPED_AREA && out.matches.moved > 0 {
            out.vanished += jumps.len() as u32;
        }
    }
    if crate::vein2::on() {
        crate::vein2::frame(before, now, &out, &matched_to, &own_rows); // vein2: dev, FATES_VEIN2_LOG
    }
    out
}

/// gfxmap: how much a particle's own number may differ from going on as it
/// went, as part of its size (besides JUMP): a Dragon Vein's rings grow
/// 0.45 a frame at sizes of 20-70 and fade by 0.01, its positions creep by
/// 0.05; another ring of the same vein differs by 10-50 in size and up to 1
/// in alpha.
const PARTICLE_PART: f32 = 0.05;

/// gfxmap: per draw of `now`, for a mesh drawn at least MANY times the same
/// way (a tree clump), the rows that differ between its copies (each one's
/// place), else none (see track).
const MANY: usize = 4;

fn mesh_rows(now: &[Draw]) -> Vec<u128> {
    let mut own = vec![0u128; now.len()];
    if !searches_wide() {
        return own;
    }
    let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, d) in now.iter().enumerate() {
        if d.shaded.as_deref().is_some_and(|s| !s.loose) {
            if let Some(k) = key(d) {
                groups.entry(k).or_default().push(i);
            }
        }
    }
    for ns in groups.values().filter(|ns| ns.len() >= MANY) {
        let first = now[ns[0]].shaded.as_deref().unwrap();
        let mut differ = 0u128;
        let mut bits = first.placing.0 | first.placing.1;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let at = |d: &Draw| -> [u32; 4] { std::array::from_fn(|c| d.shaded.as_deref().and_then(|s| s.shading.get(u * 4 + c).copied()).unwrap_or(0)) };
            if ns.iter().skip(1).any(|&i| at(&now[i]) != at(&now[ns[0]])) {
                differ |= 1 << u;
            }
        }
        for &n in ns {
            own[n] = differ;
        }
    }
    own
}

/// mapjit: FATES_GFX_HFR_PAIRS=0 matches the copies of a mesh one after
/// another in the frame's order, as before (see pair_copies).
fn pairs_copies() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| searches_wide() && crate::perf::var("FATES_GFX_HFR_PAIRS").map_or(true, |v| v != "0"))
}

/// mapjit: how far one of a copy's own numbers (its place) may change in a
/// frame and still be the same copy moving, in a set whose copies mostly
/// stay put: at least this many units, and this part of the number.
const OWN_STEP: f32 = 8.0;
const OWN_STEP_PART: f32 = 0.02;

/// mapjit: the copies of a mesh drawn many times (the map's tree clumps and
/// bushes, 8-140 of them) paired with the frame before's as a whole: every
/// pair of copies by how far apart their placing numbers are, the nearest
/// pairs first. Matched in the frame's order (each copy the nearest copy
/// still free), a clump entering the view took the place of its neighbour
/// in the list (the game lists them by place), which then took the next
/// one's, and so on: on the Prologue map, every few frames of a scroll
/// whole runs of clumps were matched with other clumps 50-100 units away,
/// and their pictures in between slid them up to 600 display pixels across
/// the screen and back (the "jittery trees"). gfx3's rule could not tell
/// them from a moving crowd then, as more than a quarter of the copies had
/// moved.
///
/// Paired nearest first, every clump still on screen finds itself (its
/// own numbers the same; only the camera's rows differ). When most copies
/// stay put that way (scenery), a copy left over whose own numbers jumped
/// (more than OWN_STEP and OWN_STEP_PART) is new: it moves with the camera
/// (follow_camera) instead of from a clump that left the view. Returns, per
/// draw of `now`, its pair, and whether it is new.
fn pair_copies(before: &[Draw], now: &[Draw], buckets: &HashMap<u64, Vec<usize>>, instance_rows: &[u128]) -> (Vec<Option<usize>>, Vec<bool>) {
    let mut paired = vec![None; now.len()];
    let mut new = vec![false; now.len()];
    if !pairs_copies() {
        return (paired, new);
    }
    let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
    for (n, d) in now.iter().enumerate() {
        if instance_rows[n] != 0 && d.shaded.as_deref().is_some_and(|s| !s.loose) {
            if let Some(k) = key(d) {
                groups.entry(k).or_default().push(n);
            }
        }
    }
    for (k, ns) in groups {
        let Some(bucket) = buckets.get(&k) else { continue };
        if ns.len() > MOST_SEARCHED || bucket.len() > MOST_SEARCHED {
            continue;
        }
        let mut pairs: Vec<(f32, usize, usize)> = Vec::with_capacity(ns.len() * bucket.len());
        for &n in &ns {
            let view = View::new(&now[n]);
            let (placing, _) = view.placing();
            for &b in bucket {
                let bv = View::new(&before[b]);
                let mut total = 0.0f32;
                let mut bits = placing;
                while bits != 0 {
                    let u = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    for c in 0..4 {
                        let i = u * 4 + c;
                        if i < view.len() {
                            total += (bv.get(i) - view.get(i)).abs();
                        }
                    }
                }
                if !total.is_nan() {
                    pairs.push((total, n, b));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut used_b: Vec<usize> = Vec::with_capacity(ns.len());
        for (_, n, b) in pairs {
            if paired[n].is_some() || used_b.contains(&b) {
                continue;
            }
            paired[n] = Some(b);
            used_b.push(b);
        }
        // scenery: most copies paired with themselves (own rows the same)
        let rows = instance_rows[ns[0]];
        let own_jump = |n: usize, b: usize| -> bool {
            let (now_v, before_v) = (View::new(&now[n]), View::new(&before[b]));
            let mut bits = rows;
            while bits != 0 {
                let u = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                for c in 0..4 {
                    let i = u * 4 + c;
                    if i >= now_v.len() {
                        continue;
                    }
                    let (bf, nw) = (before_v.get(i), now_v.get(i));
                    let change = (nw - bf).abs();
                    if change > OWN_STEP && change > OWN_STEP_PART * bf.abs().max(nw.abs()) {
                        return true;
                    }
                }
            }
            false
        };
        let same = |n: usize, b: usize| -> bool {
            let (now_v, before_v) = (View::new(&now[n]), View::new(&before[b]));
            let mut bits = rows;
            while bits != 0 {
                let u = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if (0..4).any(|c| u * 4 + c < now_v.len() && now_v.get(u * 4 + c).to_bits() != before_v.get(u * 4 + c).to_bits()) {
                    return false;
                }
            }
            true
        };
        let still = ns.iter().filter(|&&n| paired[n].is_some_and(|b| same(n, b))).count();
        if still * 2 < ns.len() {
            continue;
        }
        for &n in &ns {
            match paired[n] {
                Some(b) if own_jump(n, b) => {
                    paired[n] = None;
                    new[n] = true;
                }
                None => new[n] = true,
                _ => {}
            }
        }
    }
    (paired, new)
}

/// gfx3: a copy of a many-times-drawn mesh whose own rows changed is
/// another copy when at most one in STILL_SHARE of its copies moved;
/// FATES_GFX_HFR_STILL=0 turns the rule off (see track).
const STILL_SHARE: usize = 4;

fn instances_static() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| searches_wide() && crate::perf::var("FATES_GFX_HFR_STILL").map_or(true, |v| v != "0"))
}

/// gfxmap: FATES_GFX_HFR_CROP=0 leaves windows opening over fixed art in
/// steps (see art_shift).
fn crops() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_CROP").map_or(true, |v| v != "0"))
}

/// gfxmap: the most draws of a mesh searched whole for a match (see track).
const MOST_SEARCHED: usize = 512;

/// gfxmap: FATES_GFX_HFR_WIDE=0 searches meshes around their place only
/// (before gfxmap); FATES_GFX_HFR_FOLLOW=0 leaves new meshes where they are
/// now (see follow_camera).
fn searches_wide() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_WIDE").map_or(true, |v| v != "0"))
}

fn follows_camera() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_FOLLOW").map_or(true, |v| v != "0"))
}

/// gfxmap: a mesh new in the frame (a tree clump or a piece of ground
/// entering the view at its edge while the map scrolls) was drawn where it
/// is now in every picture in between, while the rest moved with the camera
/// part of the way: it sat a scroll step ahead of the ground under it, then
/// snapped into line (the trees "glitchy" at the screen's edges). Its rows
/// that are a matched mesh's of the same program now (the camera's, the
/// map's own matrices) move as that mesh's moved; its own rows (its place)
/// stay. The track for draw `n`, or None.
fn follow_camera(now: &[Draw], tracks: &[Track], matched_to: &[Option<usize>], n: usize) -> Option<Track> {
    let s = now[n].shaded.as_deref().filter(|s| !s.loose)?;
    let entry = s.shading.get(401).copied();
    let rows = s.placing.0 | s.placing.1;
    let mut best: Option<(u32, Track)> = None;
    for (m, d) in now.iter().enumerate() {
        if m == n || matched_to[m].is_none() || !tracks[m].moves {
            continue;
        }
        let Some(o) = d.shaded.as_deref() else { continue };
        if !Arc::ptr_eq(&o.program, &s.program) || o.shading.get(401).copied() != entry || o.placing != s.placing {
            continue;
        }
        let t = &tracks[m];
        // the rows it moved, whole and not jumped, that are this draw's too
        let mut same = 0u128;
        let mut bits = rows;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let in_row: Vec<&(u16, f32, bool)> = t.changed.iter().filter(|c| c.0 as usize / 4 == u).collect();
            if in_row.is_empty() || in_row.iter().any(|c| !c.2) {
                continue;
            }
            if (0..4).all(|c| s.shading.get(u * 4 + c) == o.shading.get(u * 4 + c)) {
                same |= 1 << u;
            }
        }
        if same == 0 {
            continue;
        }
        let count = same.count_ones();
        if best.as_ref().is_some_and(|(c, _)| *c >= count) {
            continue;
        }
        let changed: Vec<(u16, f32, bool)> = t.changed.iter().filter(|c| same & (1 << (c.0 / 4)) != 0).copied().collect();
        best = Some((count, Track { changed, moves: true, restless: 0, look: None }));
    }
    best.map(|b| b.1)
}

/// gfxmap: FATES_GFX_HFR_WRAP=0 lets a texture-read matrix's translation
/// turn with it (before gfxmap).
fn wraps_kept() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_WRAP").map_or(true, |v| v != "0"))
}

/// gfxmap: FATES_GFX_HFR_PARTICLES=0 judges particles' rows as any other
/// draw's (before gfxmap).
fn particles_own() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_PARTICLES").map_or(true, |v| v != "0"))
}

/// gfxmap: per draw of `now`, the uniform rows (vec4 bits) that belong to
/// its particles rather than to the camera or emitter: a particle draw
/// (nw::eft, `loose`) holds each particle's colour, size, place and turn in
/// rows of its own, beside the camera rows every draw of its program has
/// alike. An emitter's particles move from draw to draw and from slot to
/// slot of its draws as particles are born and die (a Dragon Vein's rings
/// every second or so), so a draw's match can hold other particles: the
/// slide rules let a dying ring (alpha 1, size 33) fade "towards" the newborn
/// now in its slot (alpha 0, size 20), and the vein's bright core dimmed in
/// every picture in between of those frames (a flicker at the display rate
/// while the vein idles). Rows equal in every draw of the program, in both
/// frames, are the camera's and keep the old rules (the map scrolling).
/// A program drawn once in both frames tells nothing apart by itself.
/// vein2: it is told apart against the frame's other particle programs
/// (see lone_particle_rows).
fn particle_rows(before: &[Draw], now: &[Draw]) -> Vec<u128> {
    let mut own = vec![0u128; now.len()];
    if !particles_own() {
        return own;
    }
    let group = |d: &Draw| d.shaded.as_deref().filter(|s| s.loose).map(|s| (Arc::as_ptr(&s.program) as usize, s.shading.get(401).copied().unwrap_or(0)));
    let mut groups: HashMap<(usize, u32), (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (i, d) in before.iter().enumerate() {
        if let Some(g) = group(d) {
            groups.entry(g).or_default().0.push(i);
        }
    }
    for (i, d) in now.iter().enumerate() {
        if let Some(g) = group(d) {
            groups.entry(g).or_default().1.push(i);
        }
    }
    for (g, (bs, ns)) in groups.iter() {
        if ns.is_empty() {
            continue;
        }
        if bs.len() < 2 && ns.len() < 2 {
            // vein2: a program drawn once (one Dragon Vein in view: its core)
            if lone_particles() {
                own[ns[0]] = lone_particle_rows(before, now, bs.first().copied(), ns[0], |d| group(d).is_some_and(|o| o != *g));
            }
            continue;
        }
        let first = now[ns[0]].shaded.as_deref().unwrap();
        let rows = first.placing.0 | first.placing.1;
        let mut differ = 0u128;
        let mut bits = rows;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let at = |d: &Draw| -> [u32; 4] { std::array::from_fn(|c| d.shaded.as_deref().and_then(|s| s.shading.get(u * 4 + c).copied()).unwrap_or(0)) };
            // within each frame (the camera can move between them)
            let varies = |ds: &[Draw], is: &[usize]| is.iter().skip(1).any(|&i| at(&ds[i]) != at(&ds[is[0]]));
            if varies(now, ns) || varies(before, bs) {
                differ |= 1 << u;
            }
        }
        for &n in ns {
            own[n] = differ;
        }
    }
    own
}

/// vein2: FATES_GFX_HFR_PARTICLES_ONE=0 leaves a particle program drawn
/// once in both frames without rows of its own (judged as any other draw),
/// and a draw whose particle rows jumped drawn as it is now whole, camera
/// rows too (both before vein2).
fn lone_particles() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| particles_own() && crate::perf::var("FATES_GFX_HFR_PARTICLES_ONE").map_or(true, |v| v != "0"))
}

/// vein2: the particle rows of draw `n` of `now`, whose program is drawn
/// once in this frame and at most once (`b`) in the frame before. With one
/// Dragon Vein in view (most maps: the vein the user idles by) its core is
/// such a program: one draw of three particles, which move up a slot each
/// as a ring is born (alpha 1, size 33 then alpha 0, size 20 in the first
/// slot; the second slot takes the first's old ring). particle_rows could
/// not tell its rows apart, the slide rules let part of them fade "towards"
/// the next particle, and the core dimmed or jumped in the pictures in
/// between of every respawn, every 31 frames.
/// Every particle program of nw::eft has the camera's and the emitter's rows
/// at the same places with the same numbers (rows 85-95 on the veins), so
/// a row is the particles' own when it differs from that row of each of the
/// frame's other particle programs (`other`), in this frame or the one
/// before. None when the frame has no other particle program to compare
/// with (as before).
fn lone_particle_rows(before: &[Draw], now: &[Draw], b: Option<usize>, n: usize, other: impl Fn(&Draw) -> bool) -> u128 {
    let others_now: Vec<&Draw> = now.iter().filter(|d| other(d)).collect();
    if others_now.is_empty() {
        return 0;
    }
    let others_before: Vec<&Draw> = before.iter().filter(|d| other(d)).collect();
    let Some(s) = now[n].shaded.as_deref() else { return 0 };
    let row = |d: &Draw, u: usize| -> Option<[u32; 4]> {
        let s = d.shaded.as_deref()?;
        Some([*s.shading.get(u * 4)?, *s.shading.get(u * 4 + 1)?, *s.shading.get(u * 4 + 2)?, *s.shading.get(u * 4 + 3)?])
    };
    let mut own = 0u128;
    let mut bits = s.placing.0 | s.placing.1;
    while bits != 0 {
        let u = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        let alone = |d: &Draw, others: &[&Draw]| row(d, u).is_some_and(|r| !others.iter().any(|o| row(o, u) == Some(r)));
        let before_alone = b.is_some_and(|b| !others_before.is_empty() && alone(&before[b], &others_before));
        if alone(&now[n], &others_now) || before_alone {
            own |= 1 << u;
        }
    }
    own
}

/// gfxleft: the share of the screen the models gone or new must have
/// covered (their boxes on the screen) for the frame to be held. The map's
/// blended range overlay re-tiles its pieces (4% of the screen each) every
/// few frames while the map scrolls at 60 Hz: holding those frames made the
/// menu's ticker judder. The cut-in's lightning ending covers 37%.
const VANISHED_AREA: f32 = 0.10;

/// gfxleft: and for blended models that jumped (a sword's trail or a range
/// overlay's piece, 2-4% of the screen, drawn where it is now beside the
/// rest moving part of the way, is no reason to hold a frame; the cut-in's
/// lightning, 15%, is).
const JUMPED_AREA: f32 = 0.10;

/// gfxleft: the share of the screen a shaded draw's box covers (1 when its
/// vertices cannot be placed: one is behind the eye).
fn screen_share(s: &Shaded) -> f32 {
    if s.program.len() < 4096 || s.shading.len() < 512 {
        return 1.0; // no program to run (tests)
    }
    let Some(clip) = crate::wide2::clip_most(s, 256) else { return 1.0 };
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    let front: Vec<(f32, f32, f32)> = clip.iter().copied().filter(|p| p.2 > 1e-5).collect();
    for &(a, b, w) in &front {
        lo = [lo[0].min(a / w), lo[1].min(b / w)];
        hi = [hi[0].max(a / w), hi[1].max(b / w)];
    }
    // transblur: a model partly or wholly behind the eye. Its part in
    // front is bounded by its vertices in front and by where the lines
    // from them to those behind cross the eye's plane, which go off to
    // infinity on the screen in the direction they cross it at. Before, any
    // vertex behind counted the whole screen: the ground the battle
    // transitions' camera flies low over, culled as it passes under it,
    // held frame after frame. Wholly behind covers nothing.
    if front.len() < clip.len() {
        if front.is_empty() {
            return 0.0;
        }
        for &(pa, pb, pw) in &front {
            for &(qa, qb, qw) in clip.iter().filter(|q| q.2 <= 1e-5) {
                let t = pw / (pw - qw);
                let (ca, cb) = (pa + t * (qa - pa), pb + t * (qb - pb));
                if ca > 0.0 { hi[0] = f32::MAX } else if ca < 0.0 { lo[0] = f32::MIN }
                if cb > 0.0 { hi[1] = f32::MAX } else if cb < 0.0 { lo[1] = f32::MIN }
            }
            if lo[0] == f32::MIN && hi[0] == f32::MAX && lo[1] == f32::MIN && hi[1] == f32::MAX {
                break;
            }
        }
    }
    let side = |c: usize| (hi[c].min(1.0) - lo[c].max(-1.0)).max(0.0) / 2.0;
    side(0) * side(1)
}

/// gfxleft: two shaded draws drawn the same way and placed by the same
/// uniforms (a copy).
fn same_place(a: &Draw, b: &Draw) -> bool {
    let (Some(x), Some(y)) = (a.shaded.as_deref(), b.shaded.as_deref()) else { return false };
    if key(a) != key(b) || x.placing != y.placing {
        return false;
    }
    let mut bits = x.placing.0 | x.placing.1;
    while bits != 0 {
        let u = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        if (0..4).any(|c| x.shading.get(u * 4 + c) != y.shading.get(u * 4 + c)) {
            return false;
        }
    }
    true
}

/// gfxleft: FATES_GFX_HFR_VANISH=0 draws pictures in between of frames
/// that lost a model, as before.
fn holds_vanished() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_VANISH").map_or(true, |v| v != "0"))
}

/// gfxleft: dev, FATES_GFX_HFR_LIST=frame (the headless frame, or the game
/// frame windowed): the frame's draws, how each is drawn and how it matched.
fn list_frame() -> bool {
    crate::perf::var("FATES_GFX_HFR_LIST").ok().and_then(|v| v.parse::<u64>().ok()) == Some(zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed))
}

fn list_draws(before: &[Draw], now: &[Draw], out: &Between, matched_to: &[Option<usize>]) {
    let frame = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!("gfxleft list: frame {frame}, {} draws, {} before, {:?}", now.len(), before.len(), out.matches);
    for (n, d) in now.iter().enumerate() {
        let t = &out.tracks[n];
        let lerps = t.changed.iter().filter(|c| c.2).count();
        let holds = t.changed.len() - lerps;
        let what = match (d.shaded.as_deref(), d.post.is_some()) {
            (_, true) => "post".to_string(),
            (Some(s), _) => {
                // the first rows that changed, with their values before and now
                let shared = |i: u16| s.placing.1 & (1u128 << (i / 4)) != 0;
                let mut rows: Vec<String> = t.changed.iter().take(if s.loose { 48 } else { 6 }).map(|c| format!("{}{}:{:.3}->{:.3}{}", if shared(c.0) { "*" } else { "" }, c.0, c.1, f32::from_bits(s.shading[c.0 as usize]), if c.2 { "" } else { "!" })).collect();
                if matched_to[n].is_none() {
                    // a new draw: where it is (its placing rows, up to 12 numbers)
                    let mut bits = s.placing.0;
                    while bits != 0 && rows.len() < 24 {
                        let u = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        if u < 20 { continue; }
                        rows.push(format!("c{u} {:?}", (0..4).map(|c| (f32::from_bits(s.shading[u * 4 + c]) * 100.0).round() / 100.0).collect::<Vec<_>>()));
                    }
                    rows.push(format!("share {:.3}", screen_share(s)));
                }
                format!(
                    "shaded #{:016x} v{} i{} tex {:?} blend {:08x} mask {:x} dt {} dw {} loose {} vp {:?} {}",
                    s.shading.iter().fold(0u64, |h, &w| mix(h, w as u64)),
                    s.vertices.len(),
                    s.indices.len(),
                    s.textures[0].map(|t| t.0.addr),
                    s.key.blend.unwrap_or(u32::MAX),
                    s.key.mask,
                    s.key.depth_test,
                    s.key.depth_write,
                    s.loose,
                    s.viewport,
                    rows.join(" ")
                )
            }
            (None, _) => format!(
                "prim v{} tex {:?} blend {:?} dt {} dw {} at {:?} look {}",
                d.vertices.len() / 24,
                d.textures[0].map(|t| t.0.addr),
                d.blend.func,
                d.depth_test,
                d.depth_write,
                (d.vertices.len() >= 24).then(|| vertex_pos(&d.vertices, 0)),
                t.look.is_some()
            ),
        };
        eprintln!("gfxleft list: {n:4} <- {:>5} moves {} lerp {lerps} hold {holds} restless {} {what}", matched_to[n].map_or("-".into(), |b| b.to_string()), t.moves, t.restless);
        // gfxmap: dev, FATES_GFX_HFR_LIST_ROWS=1: a shaded draw's placing rows, now and in its match
        if let (Some(s), true) = (d.shaded.as_deref(), crate::perf::var("FATES_GFX_HFR_LIST_ROWS").is_ok_and(|v| v == "1")) {
            let bs = matched_to[n].and_then(|b| before[b].shaded.clone());
            let mut bits = s.placing.0 | s.placing.1;
            let mut rows = Vec::new();
            while bits != 0 {
                let u = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let r = |sh: &Shaded| (0..4).map(|c| (f32::from_bits(sh.shading[u * 4 + c]) * 1000.0).round() / 1000.0).collect::<Vec<_>>();
                rows.push(format!("c{u} {:?}<-{:?}", r(s), bs.as_deref().map(r)));
            }
            eprintln!("gfxmap rows: {n} {}", rows.join(" "));
            if let Some(b) = bs.as_deref() {
                let dd: Vec<String> = s.draw.iter().zip(b.draw.iter()).enumerate().filter(|(_, (x, y))| x != y).map(|(i, (x, y))| format!("{i}:{y:08x}->{x:08x}")).collect();
                let vd = s.vertices.len() == b.vertices.len() && s.vertices == b.vertices;
                let others: Vec<usize> = (384..s.shading.len().min(b.shading.len())).filter(|&i| s.shading[i] != b.shading[i]).collect();
                eprintln!("gfxmap rows: {n} draw block changes {} vertices same {vd} ({} / {}) tex {:?}/{:?} shading words beyond floats changed {others:?}", dd.join(" "), s.vertices.len(), b.vertices.len(), s.textures.map(|t| t.map(|t| t.0.addr)), b.textures.map(|t| t.map(|t| t.0.addr)));
            }
        }
        // FATES_GFX_HFR_LIST_GLSL=n: that draw's vertex program as GLSL, with its placing bits
        if let (Some(s), Ok(want)) = (d.shaded.as_deref(), crate::perf::var("FATES_GFX_HFR_LIST_GLSL")) {
            if want.parse::<usize>().ok() == Some(n) {
                let entry = s.shading.get(401).copied().unwrap_or(0);
                eprintln!("gfxleft glsl: placing {:032x} shared {:032x} entry {entry}", s.placing.0, s.placing.1);
                let (prog, desc) = s.program.split_at(4096.min(s.program.len()));
                eprintln!("{}", zakuro_gpu::native_hfr::program_glsl(prog, desc, entry).unwrap_or_else(|e| e));
            }
        }
    }
}

/// gfx-models: dev, FATES_GFX_HFR_TRACE2D=lo-hi: the 2D draws of frames
/// lo..=hi as matched (set per frame by hfr_track).
static TRACE_2D: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn trace_2d() -> bool {
    TRACE_2D.load(std::sync::atomic::Ordering::Relaxed)
}

/// gfx-models: dev, why a matched 2D draw does or does not move.
fn trace_look(n: usize, before: &Draw, now: &Draw, shift_before: Option<[f32; 3]>, moves: bool) {
    let (bv, nv) = (&before.vertices, &now.vertices);
    let k = nv.len() / 24;
    let mut why = String::new();
    if bv.len() != nv.len() {
        why = format!("vertex count {} -> {}", bv.len() / 24, k);
    } else if bv != nv && k > 0 {
        let same_uv = (0..k).all(|i| bv[i * 24 + 12..i * 24 + 20] == nv[i * 24 + 12..i * 24 + 20]);
        let (p0, q0) = (vertex_pos(bv, 0), vertex_pos(nv, 0));
        let shift = [q0[0] - p0[0], q0[1] - p0[1], q0[2] - p0[2]];
        let rigid = (0..k).all(|i| {
            let (p, q) = (vertex_pos(bv, i), vertex_pos(nv, i));
            (0..3).all(|c| ((q[c] - p[c]) - shift[c]).abs() <= 1e-3)
        });
        why = format!("same uv {same_uv} rigid {rigid} shift {shift:?} before {shift_before:?} at {q0:?}");
    }
    let world = (before.world != now.world).then(|| format!(" world {:?} -> {:?}", [before.world[3], before.world[7]], [now.world[3], now.world[7]]));
    let colors = (k > 0 && bv.len() == nv.len()).then(|| {
        let all = |v: &[u8]| (0..k.min(4)).map(|i| format!("{:?}", &v[i * 24 + 20..i * 24 + 24])).collect::<Vec<_>>().join("");
        format!(
            " vertex colours {} -> {} colour {:?} -> {:?} blend {:?} tex {:?} alpha test {:?} -> {:?} konst {:?} -> {:?}",
            all(bv),
            all(nv),
            before.color,
            now.color,
            now.blend.func,
            now.textures[0].map(|t| t.0.addr),
            (before.alpha_test, before.alpha_func, before.alpha_ref),
            (now.alpha_test, now.alpha_func, now.alpha_ref),
            before.combiner.stages.iter().map(|s| s.konst).collect::<Vec<_>>(),
            now.combiner.stages.iter().map(|s| s.konst).collect::<Vec<_>>()
        )
    });
    eprintln!("gfx-hfr 2d: draw {n} verts {k} moves {moves} {why}{}{}", world.unwrap_or_default(), colors.unwrap_or_default());
}

/// gfx-models: a 3x4 matrix's rotation as a quaternion (w, x, y, z) and
/// its scale, when its 3x3 part is a rotation times one scale (bones,
/// models, the camera); none for anything else (projections, shears).
fn rigid(m: &[f32; 12]) -> Option<([f32; 4], f32)> {
    let row = |r: usize| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (r0, r1, r2) = (row(0), row(1), row(2));
    let (n0, n1, n2) = (dot(r0, r0), dot(r1, r1), dot(r2, r2));
    let s2 = (n0 + n1 + n2) / 3.0;
    if !(s2.is_finite() && s2 > 1e-12) {
        return None;
    }
    // the same length every row, at right angles
    let tol = 0.02 * s2;
    if (n0 - s2).abs() > tol || (n1 - s2).abs() > tol || (n2 - s2).abs() > tol {
        return None;
    }
    if dot(r0, r1).abs() > tol || dot(r0, r2).abs() > tol || dot(r1, r2).abs() > tol {
        return None;
    }
    let s = s2.sqrt();
    let r = |i: usize, j: usize| m[i * 4 + j] / s;
    // a reflection is no rotation
    let det = r(0, 0) * (r(1, 1) * r(2, 2) - r(1, 2) * r(2, 1)) - r(0, 1) * (r(1, 0) * r(2, 2) - r(1, 2) * r(2, 0)) + r(0, 2) * (r(1, 0) * r(2, 1) - r(1, 1) * r(2, 0));
    if det < 0.9 {
        return None;
    }
    let trace = r(0, 0) + r(1, 1) + r(2, 2);
    let q = if trace > 0.0 {
        let k = (trace + 1.0).sqrt() * 2.0;
        [0.25 * k, (r(2, 1) - r(1, 2)) / k, (r(0, 2) - r(2, 0)) / k, (r(1, 0) - r(0, 1)) / k]
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let k = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
        [(r(2, 1) - r(1, 2)) / k, 0.25 * k, (r(0, 1) + r(1, 0)) / k, (r(0, 2) + r(2, 0)) / k]
    } else if r(1, 1) > r(2, 2) {
        let k = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
        [(r(0, 2) - r(2, 0)) / k, (r(0, 1) + r(1, 0)) / k, 0.25 * k, (r(1, 2) + r(2, 1)) / k]
    } else {
        let k = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
        [(r(1, 0) - r(0, 1)) / k, (r(0, 2) + r(2, 0)) / k, (r(1, 2) + r(2, 1)) / k, 0.25 * k]
    };
    Some((q, s))
}

/// gfx-models: two rigid 3x4 matrices `t` of the way: the rotation turned
/// (slerp), the scale and the translation moved straight.
fn rigid_at(before: &[f32; 12], now: &[f32; 12], t: f32) -> Option<[f32; 12]> {
    let ((qa, sa), (mut qb, sb)) = (rigid(before)?, rigid(now)?);
    let mut d = qa[0] * qb[0] + qa[1] * qb[1] + qa[2] * qb[2] + qa[3] * qb[3];
    if d < 0.0 {
        qb = qb.map(|c| -c);
        d = -d;
    }
    let q: [f32; 4] = if d > 0.9995 {
        std::array::from_fn(|i| qa[i] + (qb[i] - qa[i]) * t)
    } else {
        let theta = d.min(1.0).acos();
        let (wa, wb) = (((1.0 - t) * theta).sin() / theta.sin(), (t * theta).sin() / theta.sin());
        std::array::from_fn(|i| qa[i] * wa + qb[i] * wb)
    };
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let [w, x, y, z] = q.map(|c| c / n);
    let s = sa + (sb - sa) * t;
    let rot = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let mut out = [0.0f32; 12];
    // castlecam: the translation goes round with the turn (screw_at)
    let screwed = screw_at(qa, qb, sa, sb, [before[3], before[7], before[11]], [now[3], now[7], now[11]], t);
    for r in 0..3 {
        for c in 0..3 {
            out[r * 4 + c] = rot[r][c] * s;
        }
        out[r * 4 + 3] = match screwed {
            Some(p) => p[r],
            None => before[r * 4 + 3] + (now[r * 4 + 3] - before[r * 4 + 3]) * t,
        };
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// castlecam: FATES_GFX_HFR_SCREW=0 moves a turning matrix's translation
/// straight, as before.
fn screws() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_SCREW").map_or(true, |v| v != "0"))
}

fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// a vector turned by a unit quaternion [w, x, y, z].
fn qrot(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = qmul(qmul(q, [0.0, v[0], v[1], v[2]]), [q[0], -q[1], -q[2], -q[3]]);
    [p[1], p[2], p[3]]
}

/// castlecam: where a rigid matrix's translation is `t` of the way through
/// its move, the move taken as one screw: the turn from the frame before's
/// matrix to the frame's (qa to qb, the rotation slerped) about an axis
/// through a fixed point, and a slide along that axis. Straight (each
/// number from before to now), a camera going round what it looks at, as
/// My Castle's follow camera does while it turns, put the view's
/// translation on the chord of its arc: half way through a frame
/// everything was drawn nearer the point it turns round, by 1 - cos of
/// half the frame's turn (2 % at 23 degrees a frame, a fast mouse turn), so
/// the picture shrank and grew back each frame while it turned (a wobble at
/// the game's 30 Hz). On the screw every picture in between is the camera
/// turned part of the way, exactly. Only for such a turn (the output
/// space's origin moving less than the translation); a move with (almost)
/// no turn, or whose scale changes, stays straight (None).
fn screw_at(qa: [f32; 4], qb: [f32; 4], sa: f32, sb: f32, pa: [f32; 3], pb: [f32; 3], t: f32) -> Option<[f32; 3]> {
    if !screws() || (sa / sb - 1.0).abs() > 0.01 {
        return None;
    }
    // the turn from before to now, the short way
    let mut r = qmul(qb, [qa[0], -qa[1], -qa[2], -qa[3]]);
    let n = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
    if !(n > 0.5) {
        return None;
    }
    r = r.map(|c| c / n);
    if r[0] < 0.0 {
        r = r.map(|c| -c);
    }
    let half = r[0].min(1.0).acos();
    // under about a twentieth of a degree the chord is the arc
    if half < 4e-4 {
        return None;
    }
    let sin_half = half.sin();
    let axis = [r[1] / sin_half, r[2] / sin_half, r[3] / sin_half];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let turned = qrot(r, pa);
    let u = [pb[0] - turned[0], pb[1] - turned[1], pb[2] - turned[2]];
    // only a turn round something nearer the output space's origin (the
    // eye, for a view matrix) than the matrix's own origin: the screw's
    // origin moves less than the translation does. A model spinning as it
    // flies (a thrown axe) or a limb at its joint keeps the straight
    // translation, as before
    let straight = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    if dot(u, u) >= dot(straight, straight) {
        return None;
    }
    let along = dot(axis, u);
    let flat = [u[0] - axis[0] * along, u[1] - axis[1] * along, u[2] - axis[2] * along];
    // the axis's point: (I - R) c = flat, c = (flat + cot(half) axis x flat) / 2
    let cot = half.cos() / sin_half;
    let cross = [axis[1] * flat[2] - axis[2] * flat[1], axis[2] * flat[0] - axis[0] * flat[2], axis[0] * flat[1] - axis[1] * flat[0]];
    let c = [0.5 * (flat[0] + cot * cross[0]), 0.5 * (flat[1] + cot * cross[1]), 0.5 * (flat[2] + cot * cross[2])];
    let (st, ct) = ((t * half).sin(), (t * half).cos());
    let part = [ct, axis[0] * st, axis[1] * st, axis[2] * st];
    let v = qrot(part, [pa[0] - c[0], pa[1] - c[1], pa[2] - c[2]]);
    let p = [c[0] + v[0] + axis[0] * along * t, c[1] + v[1] + axis[1] * along * t, c[2] + v[2] + axis[2] * along * t];
    p.iter().all(|x| x.is_finite()).then_some(p)
}

/// gfxmodels3: FATES_GFX_HFR_REST=0 judges the move after a jump against
/// the jump, as before (the lightning along the crit cut-in's jaws, which
/// snap shut in one frame, were held for the four frames after).
fn after_jump_from_rest() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_REST").map_or(true, |v| v != "0"))
}

/// gfxmodels3: the most a rigid matrix turns in a frame and still turns
/// (radians), and the most its translation moves meanwhile. The crit
/// cut-in's jaws close by about 50 degrees in a frame at 30 Hz; their
/// numbers broke the jump rule, so they were held while the panel and the
/// lightning along their edges moved (and, restless, kept holding for four
/// frames): pictures in between showed the edges apart from the jaws. A
/// camera cut moves the view's translation by far more than MOST_MOVE (the
/// eye is hundreds of units from what it looks at), so it still jumps.
const MOST_TURN_ANGLE: f32 = 1.2;

fn turns_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_TURN").map_or(true, |v| v != "0"))
}

/// gfxmodels3: the placing uniforms of a shaded draw that are rows of a
/// rigid matrix (three in a row) turning by at most MOST_TURN_ANGLE, its
/// scale kept, its translation moving by at most MOST_MOVE.
fn turns(before: &View, now: &View, bits: u128) -> u128 {
    if !turns_on() || !slerps() {
        return 0;
    }
    let mut out = 0u128;
    let mut u = 0usize;
    while u + 3 <= SHADED_FLOATS / 4 {
        let all = (0..3).all(|k| bits & (1 << (u + k)) != 0);
        if all {
            let was: [f32; 12] = std::array::from_fn(|k| before.get(u * 4 + k));
            let now_m: [f32; 12] = std::array::from_fn(|k| now.get(u * 4 + k));
            if was != now_m {
                if let (Some((qa, sa)), Some((qb, sb))) = (rigid(&was), rigid(&now_m)) {
                    let dot = (qa[0] * qb[0] + qa[1] * qb[1] + qa[2] * qb[2] + qa[3] * qb[3]).abs().min(1.0);
                    let angle = 2.0 * dot.acos();
                    let moved = (0..3).map(|r| (now_m[r * 4 + 3] - was[r * 4 + 3]).abs()).fold(0.0f32, f32::max);
                    // castlecam: or a camera turning about (or near) its eye
                    let moved = moved.min(if angle <= TURN_ABOUT_EYE { eye_moved(qa, qb, &was, &now_m) } else { f32::MAX });
                    if angle <= MOST_TURN_ANGLE && moved <= MOST_MOVE && (sa / sb - 1.0).abs() <= 0.05 {
                        out |= 0b111 << u;
                        u += 3;
                        continue;
                    }
                }
            }
        }
        u += 1;
    }
    out
}

/// castlecam: the most a view matrix turns in a frame and still counts as
/// a camera turning about its eye (radians, 34 degrees: a fast mouse turn
/// in My Castle is 21 a frame).
const TURN_ABOUT_EYE: f32 = 0.6;

/// castlecam: how far the output space's origin moved (for a view matrix,
/// the eye: the translation of the move from the frame before's matrix to
/// the frame's, now - turn * before). The rule above judges a rigid
/// matrix's turn by how far its translation moved; a view matrix's
/// translation is the world's origin seen from the eye, hundreds of units
/// away in My Castle (x 850), so the camera tilting by 5 degrees as it
/// started to rise over a wall moved it 71 units and the whole view
/// counted as a cut (held, then jumped). Turning about the eye moves the
/// eye little. FATES_GFX_HFR_EYE=0: the translation alone, as before.
fn eye_moved(qa: [f32; 4], qb: [f32; 4], was: &[f32; 12], now: &[f32; 12]) -> f32 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_EYE").map_or(true, |v| v != "0")) {
        return f32::MAX;
    }
    let mut r = qmul(qb, [qa[0], -qa[1], -qa[2], -qa[3]]);
    let n = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
    if !(n > 0.5) {
        return f32::MAX;
    }
    r = r.map(|c| c / n);
    let turned = qrot(r, [was[3], was[7], was[11]]);
    let u = [now[3] - turned[0], now[7] - turned[1], now[11] - turned[2]];
    (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt()
}

/// FATES_GFX_HFR_SLERP=0: dev, every placing component moved straight, as
/// before gfx-models.
fn slerps() -> bool {
    crate::perf::var("FATES_GFX_HFR_SLERP").map_or(true, |v| v != "0")
}

/// a shaded draw with its uniforms moved.
pub(crate) fn shaded_at(s: &Shaded, changed: &[(u16, f32, bool)], t: f32) -> Shaded { // castlecam: pub(crate)
    let mut shading = s.shading.clone();
    // gfx-models: three rows that make a rigid matrix in both frames (a
    // bone of the palette, a model or the camera matrix) turn rather than
    // move each number straight: straight, a limb turning fast shrank
    // towards its joint half way
    let mut done = [false; SHADED_FLOATS];
    if slerps() && changed.iter().any(|c| c.2) {
        let mut before: Vec<f32> = s.shading.iter().map(|&b| f32::from_bits(b)).collect();
        let mut lerped = [0u8; SHADED_FLOATS]; // 1 moves, 2 stays (jumped)
        for &(i, b, lerp) in changed {
            if (i as usize) < SHADED_FLOATS {
                before[i as usize] = b;
                lerped[i as usize] = if lerp { 1 } else { 2 };
            }
        }
        let mut u = 0usize;
        while u + 3 <= SHADED_FLOATS / 4 {
            let range = u * 4..u * 4 + 12;
            let moves = lerped[range.clone()].contains(&1) && !lerped[range.clone()].contains(&2);
            if moves {
                let now: [f32; 12] = std::array::from_fn(|k| f32::from_bits(s.shading[u * 4 + k]));
                let was: [f32; 12] = std::array::from_fn(|k| before[u * 4 + k]);
                // a 3x4 matrix's rows end in its translation; w of a vec4
                // row of something else is rarely a rigid matrix's
                if let Some(m) = rigid_at(&was, &now, t) {
                    for k in 0..12 {
                        shading[u * 4 + k] = m[k].to_bits();
                        done[u * 4 + k] = true;
                    }
                    u += 3;
                    continue;
                }
            }
            u += 1;
        }
    }
    for &(i, before, lerp) in changed {
        if lerp && !done.get(i as usize).copied().unwrap_or(false) {
            let now = f32::from_bits(shading[i as usize]);
            shading[i as usize] = (before + (now - before) * t).to_bits();
        }
    }
    Shaded {
        program: s.program.clone(),
        shading,
        draw: s.draw.clone(),
        tables: s.tables.clone(),
        vertices: s.vertices.clone(),
        indices: s.indices.clone(),
        textures: s.textures,
        mips: s.mips,
        key: s.key,
        viewport: s.viewport,
        scissor: s.scissor,
        depth_range: s.depth_range,
        blend_color: s.blend_color,
        placing: s.placing,
        loose: s.loose,
    }
}

/// the frame's draws at `t` of the way from the frame before (0) to its own
/// (1): what moved, moved; the rest as the frame has it.
pub fn draws_at(draws: &[Draw], between: &Between, t: f32) -> Vec<Draw> {
    // gfx-models: dev, FATES_GFX_HFR_IDENTITY=1 draws the frame's own draws
    // (a picture in between equal to the frame, through the same path)
    if crate::perf::var("FATES_GFX_HFR_IDENTITY").is_ok_and(|v| v == "1") {
        return draws.to_vec();
    }
    // gfxleft: dev, FATES_GFX_HFR_HOLD=a-b,c,...: those draws as the frame has them
    let held: Vec<(usize, usize)> = crate::perf::var("FATES_GFX_HFR_HOLD")
        .map(|v| {
            v.split(',')
                .filter_map(|r| match r.split_once('-') {
                    Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
                    None => r.trim().parse().ok().map(|a| (a, a)),
                })
                .collect()
        })
        .unwrap_or_default();
    // gfx3: dev, FATES_GFX3_DBG=frame: the tracks draws_at applies to that headless frame's draws
    if crate::perf::var("FATES_GFX3_DBG").ok().and_then(|v| v.parse::<u64>().ok()) == Some(zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed)) {
        eprintln!("gfx3 dbg: draws_at t {t} draws {} tracks {}", draws.len(), between.tracks.len());
        for (n, d) in draws.iter().enumerate().take(64) {
            if let Some(s) = d.shaded.as_deref() {
                let tr = between.tracks.get(n);
                let m = |u: usize| f32::from_bits(s.shading[u * 4 + 3]);
                eprintln!(
                    "gfx3 dbg: {n} v{} model {:.0} {:.0} {:.0} moves {:?} changed {:?}",
                    s.vertices.len(),
                    m(25),
                    m(26),
                    m(27),
                    tr.map(|t| t.moves),
                    tr.map(|t| t.changed.iter().map(|c| (c.0, c.1, c.2)).collect::<Vec<_>>())
                );
            }
        }
    }
    let placed: Vec<Draw> = draws
        .iter()
        .zip(between.tracks.iter().chain(std::iter::repeat(&Track::default())))
        .enumerate()
        .map(|(n, (d, track))| {
            if !track.moves || held.iter().any(|&(a, b)| n >= a && n <= b) {
                return d.clone();
            }
            match d.shaded.as_deref() {
                Some(s) => {
                    let mut moved = d.clone();
                    moved.shaded = Some(Arc::new(shaded_at(s, &track.changed, t)));
                    moved
                }
                None => {
                    let mut moved = d.clone();
                    for &(i, before, lerp) in &track.changed {
                        if !lerp {
                            continue;
                        }
                        let i = i as usize;
                        let slot = if i < 12 { &mut moved.world[i] } else { &mut moved.view_proj[i - 12] };
                        *slot = before + (*slot - before) * t;
                    }
                    // gfx-final: fades and rigid vertex moves
                    if let Some(l) = track.look.as_deref() {
                        look_at(&mut moved, l, t);
                    }
                    moved
                }
            }
        })
        .collect();
    crate::castlecam::with_ghosts(placed, &between.ghosts, t) // castlecam
}

/// gfx-final: a 2D draw's look `t` of the way from its match's.
fn look_at(d: &mut Draw, l: &Look, t: f32) {
    let mixu8 = |b: u8, n: u8| (b as f32 + (n as f32 - b as f32) * t).round().clamp(0.0, 255.0) as u8;
    if let Some(c) = l.color {
        for (now, before) in d.color.iter_mut().zip(c) {
            *now = before + (*now - before) * t;
        }
    }
    if let Some(r) = l.alpha_ref {
        d.alpha_ref = mixu8(r, d.alpha_ref); // gfx-2d, gfx-models
    }
    for (stage, k) in d.combiner.stages.iter_mut().zip(l.konst.iter()) {
        if let Some(k) = k {
            stage.konst = std::array::from_fn(|i| mixu8(k[i], stage.konst[i]));
        }
    }
    if let Some(bv) = l.vertices.as_deref() {
        if bv.len() == d.vertices.len() {
            for k in 0..d.vertices.len() / 24 {
                if l.positions {
                    // gfxmodels3: and the texture coordinates, for art cut elsewhere
                    let last = if l.uvs { 5 } else { 3 };
                    for c in 0..last {
                        let at = k * 24 + c * 4;
                        let b = f32::from_le_bytes(bv[at..at + 4].try_into().unwrap());
                        let n = f32::from_le_bytes(d.vertices[at..at + 4].try_into().unwrap());
                        d.vertices[at..at + 4].copy_from_slice(&(b + (n - b) * t).to_le_bytes());
                    }
                }
                if l.colors {
                    for i in 20..24 {
                        d.vertices[k * 24 + i] = mixu8(bv[k * 24 + i], d.vertices[k * 24 + i]);
                    }
                }
            }
        }
    }
}

/// gfx-models: how far apart two 2D draws drawn the same way are besides
/// their matrices: their first and last vertices' positions (pixels), and
/// their colours (a full change of one channel counting as 32 pixels).
fn prim_distance(before: &Draw, now: &Draw) -> f32 {
    let (bv, nv) = (&before.vertices, &now.vertices);
    let mut total = 0.0f32;
    if bv.len() == nv.len() && nv.len() >= 24 && nv.len() % 24 == 0 {
        let last = nv.len() / 24 - 1;
        for k in [0, last] {
            let (p, q) = (vertex_pos(bv, k), vertex_pos(nv, k));
            total += (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>();
            total += (20..24).map(|i| (bv[k * 24 + i] as f32 - nv[k * 24 + i] as f32).abs()).sum::<f32>() / 255.0 * 32.0;
        }
    }
    total += before.color.iter().zip(now.color.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() * 32.0;
    for (x, y) in before.combiner.stages.iter().zip(now.combiner.stages.iter()) {
        total += x.konst.iter().zip(y.konst.iter()).map(|(&a, &b)| (a as f32 - b as f32).abs()).sum::<f32>() / 255.0 * 32.0;
    }
    total
}

/// gfx-models: FATES_GFX_HFR_DISTINGUISH=0 matches 2D draws by their
/// matrices alone, as before.
fn distinguishes() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_DISTINGUISH").map_or(true, |v| v != "0"))
}

/// gfx-models: FATES_GFX_HFR_RESHAPE=0 keeps reshaped 2D elements as the
/// frame has them (rigid moves only).
fn reshapes() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_RESHAPE").map_or(true, |v| v != "0"))
}

/// gfx-final: FATES_GFX_HFR_LOOK=0 leaves 2D fades and vertex moves as the
/// frame has them (matrices only, as before).
fn looks() -> bool {
    crate::perf::var("FATES_GFX_HFR_LOOK").map_or(true, |v| v != "0")
}

/// gfx-models: whether a post effect reads a surface it did not write
/// itself first: what an earlier frame's post effect left there (a
/// feedback effect: the battle transitions blend the picture with the
/// frames before it this way). The renderer keeps surfaces from frame to
/// frame, and a picture in between is drawn by a renderer of its own whose
/// surfaces hold what its own last picture left, or nothing: the effect
/// came out missing in every picture in between, so the screen flickered
/// between blurred frames and sharp pictures in between at the display's
/// rate. No picture in between of such a frame can be right (its reads are
/// of earlier frames' pictures, not of pictures in between), so the frame
/// is shown as it is.
pub fn carries(block: &crate::post::Block) -> bool {
    use crate::post::{Op, Target, Unit};
    use crate::surfaces::Action;
    let mut written: Vec<crate::surfaces::Key> = Vec::new();
    let read = |written: &Vec<crate::surfaces::Key>, key: crate::surfaces::Key| !written.contains(&key);
    for op in &block.ops {
        match op {
            Op::Act(Action::Capture { dst, .. }) | Op::Act(Action::Clear { dst, .. }) => written.push(dst.key),
            Op::Act(Action::Copy { src, dst, .. }) | Op::Act(Action::Filter { src, dst, .. }) => {
                if read(&written, src.key) {
                    return true;
                }
                written.push(dst.key);
            }
            Op::Draw(d) => {
                for unit in &d.units {
                    if let Unit::Surface { desc, .. } = unit {
                        if read(&written, desc.key) {
                            return true;
                        }
                    }
                }
                if let Target::Surface { desc, .. } = d.target {
                    written.push(desc.key);
                }
            }
            Op::Callback(_) => {}
        }
    }
    false
}

/// the placing uniforms of a setup's program, see Shaded::placing; cached
/// by program, entry point and output map.
pub fn placing(setup: &crate::setup::Setup, program: &Arc<Vec<u32>>) -> (u128, u128) {
    type Key = (usize, u32, u16, u16);
    // gfx-models: each entry holds its program, so while it is cached no
    // other program can be made at the same address (cache::program drops
    // its programs when it grows past 1024; a program made later at a freed
    // one's address took the freed one's placing uniforms)
    type Entry = (Arc<Vec<u32>>, (u128, u128));
    static CACHE: Mutex<Option<(Key, (u128, u128), HashMap<Key, Entry>)>> = Mutex::new(None);
    let (outputs, coordinates) = zakuro_gpu::native_hfr::placing_outputs(&setup.regs);
    let key: Key = (Arc::as_ptr(program) as usize, setup.entry, outputs, coordinates);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(|| ((usize::MAX, 0, 0, 0), (0, 0), HashMap::new()));
    if cache.0 == key && cache.2.contains_key(&key) {
        return cache.1;
    }
    let found = match cache.2.get(&key) {
        Some(e) => e.1,
        None => {
            let p = zakuro_gpu::native_hfr::placing_uniforms(&setup.program, &setup.descriptors, setup.entry, outputs, coordinates);
            if cache.2.len() > 4096 {
                cache.2.clear();
            }
            cache.2.insert(key, (program.clone(), p));
            p
        }
    };
    cache.0 = key;
    cache.1 = found;
    found
}

/// gfxleft: the placing uniforms of a particle's program (nw::eft, one
/// particle a draw) that its texture coordinates read straight too: they
/// count with those the coordinates read (Shaded::placing's second). A
/// particle's block holds its flip-book cell (c3.x, in eighths) beside its
/// place, size and turn. Moved part of the way, a cell 0.125 -> 0.375 was
/// read at 0.19, between two cells, and every picture in between cut the
/// pink flame aura of the soldier in Corrin's battle into hard-edged
/// rectangles. A step between sixty-fourths is a step (steps): the picture
/// in between shows the frame's cell; a scroll or a turn still moves.
/// Particles only: a mesh's texture coordinates can read its camera
/// straight (an environment map), whose rows must keep jumping at a cut.
pub fn particle_texture_shared(setup: &crate::setup::Setup, program: &Arc<Vec<u32>>) -> u128 {
    if !text_direct_on() {
        return 0;
    }
    let (outputs, coordinates) = zakuro_gpu::native_hfr::placing_outputs(&setup.regs);
    if coordinates == 0 {
        return 0;
    }
    // by program (shared by content, cache::program), as placing: each
    // entry holds its program, so no other is made at its address meanwhile
    type Key = (usize, u32, u16, u16);
    static CACHE: Mutex<Option<HashMap<Key, (Arc<Vec<u32>>, u128)>>> = Mutex::new(None);
    let key: Key = (Arc::as_ptr(program) as usize, setup.entry, outputs, coordinates);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(e) = cache.get(&key) {
        return e.1;
    }
    let place = zakuro_gpu::native_hfr::uniforms_read_directly(&setup.program, &setup.descriptors, setup.entry, outputs);
    let texture = zakuro_gpu::native_hfr::uniforms_read_directly(&setup.program, &setup.descriptors, setup.entry, coordinates);
    if cache.len() > 4096 {
        cache.clear();
    }
    cache.insert(key, (program.clone(), place & texture));
    place & texture
}

/// gfxleft: FATES_GFX_HFR_TEXDIRECT=0 moves a particle's uniforms read
/// straight by its texture coordinates as well as its place like any
/// other, as before.
fn text_direct_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_TEXDIRECT").map_or(true, |v| v != "0"))
}

/// gfx-models: with FATES_GFX_HFR_SHOT_IDS the picture in between is also
/// kept as <disp>-<frame>-b<at in 1000ths>.png, so every place of one frame
/// shows.
fn shot_names_at(dir: &str, index: usize, id: u64, at: f32) -> std::io::Result<()> {
    if crate::perf::var("FATES_GFX_HFR_SHOT_IDS").is_err() && crate::perf::var("FATES_GFX_HFR_SHOT_GAME").is_err() {
        return Ok(());
    }
    let d = std::path::Path::new(dir);
    std::fs::copy(d.join(format!("{index}-{id:06}-b.png")), d.join(format!("{index}-{id:06}-b{:03}.png", (at * 1000.0).round() as u32)))?;
    Ok(())
}

/// gfx-models: dev, FATES_GFX_PACE_LOG=file, see compose_gpu.
fn pace_log(id: u64, pick: zakuro_core::native_gfx::Pick) {
    use std::io::Write;
    static LOG: Mutex<Option<(std::time::Instant, std::io::BufWriter<std::fs::File>)>> = Mutex::new(None);
    let Ok(path) = crate::perf::var("FATES_GFX_PACE_LOG") else { return };
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Ok(file) = std::fs::File::create(&path) else { return };
        *guard = Some((std::time::Instant::now(), std::io::BufWriter::new(file)));
    }
    let (start, out) = guard.as_mut().unwrap();
    let game = game_frame(id); // gfxmodels3
    let (kind, at) = match pick {
        zakuro_core::native_gfx::Pick::Newest => ("n", 1.0),
        zakuro_core::native_gfx::Pick::Previous => ("p", 0.0),
        zakuro_core::native_gfx::Pick::Between(at) => ("b", at),
    };
    let _ = writeln!(out, "{}	{id}	{kind}	{at}	{game}", start.elapsed().as_micros());
    let _ = out.flush(); // a window is stopped by killing it
}

/// gfxmodels3: dev, the game frame each native frame was drawn in, for
/// FATES_GFX_PACE_LOG (a fifth column) and FATES_GFX_HFR_SHOT_GAME=lo-hi
/// (windowed shots of the frames drawn in those game frames).
static GAME_FRAMES: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);

fn dev_frames() -> bool {
    crate::perf::var("FATES_GFX_PACE_LOG").is_ok() || crate::perf::var("FATES_GFX_HFR_SHOT_GAME").is_ok()
}

fn note_game_frame(id: u64) {
    if !dev_frames() {
        return;
    }
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let mut map = GAME_FRAMES.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    if map.len() > 100_000 {
        map.clear();
    }
    map.insert(id, game);
}

fn game_frame(id: u64) -> u64 {
    GAME_FRAMES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).copied()).unwrap_or(0)
}

/// FATES_GFX_HFR=0 turns native pictures in between off (each shows the
/// frame's own picture).
pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_HFR").map_or(true, |v| v != "0")
}

/// the pictures drawn for the presenter, by frame, kept while their images
/// are (the renderer's presented images go round a ring).
#[derive(Default)]
pub struct State {
    /// per display, the last frame committed (what the next is matched
    /// against)
    last: [Option<Arc<crate::Frame>>; 2],
    /// per display: frames drawn on the GPU path, with the emulated image a
    /// hybrid was drawn over, newest last
    drawn: [std::collections::VecDeque<(u64, Option<ash::vk::ImageView>, zakuro_gpu::GpuScreen)>; 2],
    /// per display: pictures in between drawn on the GPU path, by frame,
    /// place (f32 bits) and the emulated image under them
    /// (gfxcastle: and the fraction of the scale it was drawn at)
    between: [std::collections::VecDeque<(u64, u32, Option<ash::vk::ImageView>, zakuro_gpu::GpuScreen, u32)>; 2],
    /// the GPU path's pictures in between are drawn by a renderer of their
    /// own on the presenter's device: its own commands, fence and buffers,
    /// so a picture in between does not wait for the frame drawn just
    /// before it, nor the next frame for it (FATES_GFX_HFR_RENDERER=shared
    /// draws them with the frames' renderer)
    pub renderer: Option<crate::render::Renderer>,
    renderer_failed: bool,
    /// gfx-final: the lower display's (see lib.rs split_renderers)
    pub renderer_lower: Option<crate::render::Renderer>,
    /// counts, for the report line
    pub tracked: u64,
    pub smooth: u64,
    pub drawn_between: u64,
    pub fallbacks: u64,
    /// gfx-models: frames shown without pictures in between because their
    /// post effects read earlier frames' surfaces (carries)
    pub carried: u64,
    /// gfxleft: frames held because a model of the frame before was gone
    pub vanished: u64,
    /// gfx-models: frames committed into the buffer their screen showed
    pub into_shown: u64,
    /// gfx-models: the upper display's newest frame as pacing last saw it,
    /// and the one before it (see compose_gpu)
    top_ids: (u64, u64),
    /// gfx-models: lower-display picks of a frame older than the upper's
    /// picture shown as the frame
    pub stale_lower: u64,
    /// gfx-models: pictures in between drawn before the frame before
    pub before_previous: u64,
    /// gfx-models: dev, pictures in between seen in FATES_GFX_HFR_SHOT_IDS
    shots_in_range: u64,
    /// smooth2: pictures in between drawn at a fraction of the frames'
    /// scale while the GPU cannot keep up (see between_scale)
    pub scaling: BetweenScale,
    /// gfxcastle: frames shown at the scale of the pictures in between
    pub reduced_frames: u64,
    /// sharp: upper pictures handed over while pictures are drawn smaller:
    /// at full scale (the frames' renderer's) and at the smaller scale
    pub shown_full: u64,
    pub shown_scaled: u64,
    /// rtpace: a picture in between on the drawing thread (rtpace.rs),
    /// which has `renderer` meanwhile
    drawing: Vec<(usize, std::sync::mpsc::Receiver<crate::rtpace::Done>)>,
    /// rtpace: the presented images' views of the renderers lent out
    /// (still theirs: prune keeps their pictures), and textures to forget
    /// once they are back
    lent_views: Vec<ash::vk::ImageView>,
    forget_later: Vec<(crate::prim::TexRef, u64)>,
}

/// rtpace: how a picture in between starts (render::Start's two kinds it
/// uses, without a borrow, for the drawing thread)
#[derive(Clone, Copy)]
pub(crate) enum JobStart {
    Clear([f32; 4]),
    View(ash::vk::ImageView, [f32; 4]),
}

/// rtpace: a picture in between to draw, see Gfx::prepare_between.
pub(crate) struct BetweenJob {
    index: usize,
    frame: Arc<crate::Frame>,
    between: Arc<Between>,
    pub(crate) at: f32,
    own: bool,
    place: f32,
    base_view: Option<ash::vk::ImageView>,
    fraction: f32,
    width: u32,
    height: u32,
    scale: f32,
    start: JobStart,
    depth: f32,
    newest: Option<zakuro_gpu::GpuScreen>,
}

impl BetweenJob {
    pub(crate) fn frame_id(&self) -> u64 {
        self.frame.id
    }

    /// draws it with `renderer` (the in-between renderer, or the frames'
    /// when it has none of its own).
    pub(crate) fn draw(&self, renderer: &mut crate::render::Renderer, decoded: &HashMap<(crate::prim::TexRef, u64), Arc<crate::render::Decoded>>) -> Result<crate::render::Output, String> {
        let _t = crate::perf::scope(crate::perf::C::Between);
        let draws = if self.own { self.frame.draws.clone() } else { draws_at(&self.frame.draws, &self.between, self.at) };
        let keys = if self.own { std::borrow::Cow::Borrowed(&self.frame.texture_keys[..]) } else { crate::castlecam::keys_with_ghosts(&self.frame.texture_keys, &self.between.ghosts) }; // castlecam
        let start = match self.start {
            JobStart::Clear(rgba) => crate::render::Start::Clear(rgba),
            JobStart::View(view, area) => crate::render::Start::View(view, area),
        };
        renderer.render_between(self.index, self.width, self.height, self.scale, start, self.depth, &draws, &keys, decoded, crate::render::Finish::Present)
    }
}

/// gfxcastle: the place a frame's own picture drawn by the in-between
/// renderer is kept under (see reduced_frame).
const OWN_PLACE: f32 = 2.0;

/// sharp: whether frames with nothing moving are shown at the pictures in
/// between's scale too while that is down (FATES_SHARP_STILL=0: at full
/// scale, gfxcastle's way).
fn still_follows() -> bool {
    !crate::perf::var("FATES_SHARP_STILL").is_ok_and(|v| v == "0")
}

/// sharp: the frames themselves are drawn at the smaller scale (by the
/// frames' renderer, see frame_scale), so no full-scale picture is drawn
/// that is never shown.
static FRAMES_REDUCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn frames_reduced() -> bool {
    FRAMES_REDUCED.load(std::sync::atomic::Ordering::Relaxed)
}

impl State {
    /// sharp: the whole-number scale the frames are drawn at while every
    /// picture follows a smaller scale (the user's 8x at 3/4 is 6x, the
    /// window's own rows), instead of `scale`; None to draw them at `scale`
    /// (a fraction that gives no whole number keeps gfxcastle's way: the
    /// frame at full scale, shown drawn again at the smaller one).
    /// FATES_SHARP_FRAMES=0: always gfxcastle's way.
    pub(crate) fn frame_scale(&self, scale: u32) -> Option<u32> {
        let on = still_follows() && self.scaling.frames_follow() && !crate::perf::var("FATES_SHARP_FRAMES").is_ok_and(|v| v == "0");
        let s = scale as f32 * self.scaling.fraction();
        let whole = s.round();
        let reduced = (on && whole >= 1.0 && (s - whole).abs() < 1e-3 && (whole as u32) < scale).then_some(whole as u32);
        FRAMES_REDUCED.store(reduced.is_some(), std::sync::atomic::Ordering::Relaxed);
        reduced
    }
}

/// smooth2: the scale pictures in between are drawn at, against the
/// frames'. At the user's 8x render scale and 21:9 a picture in between is
/// 7680x1920 drawn again in full, 175 times a second with the frames': the
/// GPU fell behind (p90 13 ms a picture in between, 98 pictures a second
/// shown, a repeated refresh in every other one). A picture in between is
/// on screen for one refresh, between two full-scale frames, so while
/// drawing one takes long (its wall time, which takes in waiting for the
/// GPU) they are drawn at 3/4 or 1/2 of the scale, and back at full scale
/// once there is time again. FATES_GFX_HFR_SCALE=f pins the fraction.
#[derive(Default)]
pub struct BetweenScale {
    /// the step: 0 full, 1 three quarters, 2 half.
    step: usize,
    /// a picture in between's wall time, smoothed (seconds).
    cost: f64,
    /// since when the cost asked for a change.
    since: Option<std::time::Instant>,
    pub changes: u64,
}

const BETWEEN_STEPS: [f32; 3] = [1.0, 0.75, 0.5];

impl BetweenScale {
    /// the fraction to draw the next picture in between at.
    pub(crate) fn fraction(&self) -> f32 {
        if let Ok(v) = crate::perf::var("FATES_GFX_HFR_SCALE") {
            if let Ok(f) = v.parse::<f32>() {
                return f.clamp(0.25, 1.0);
            }
        }
        // the frontend's pacing knows when the display went without a
        // picture, which this renderer's own timing cannot tell
        if let Some(f) = zakuro_core::native_gfx::between_fraction() {
            return f.clamp(0.25, 1.0);
        }
        BETWEEN_STEPS[self.step]
    }

    /// gfxcastle: whether frames are shown at the pictures in between's
    /// scale (reduced_frame): while that is below full, unless
    /// FATES_GFX_HFR_SCALE_FRAMES=0 (smooth2's way: frames always full).
    pub fn frames_follow(&self) -> bool {
        self.fraction() < 0.999 && !crate::perf::var("FATES_GFX_HFR_SCALE_FRAMES").is_ok_and(|v| v == "0")
    }

    /// a picture in between took `took` to draw: down a step after a
    /// quarter second above 4 ms, up a step after two seconds below 2 ms.
    fn took(&mut self, took: std::time::Duration) {
        // the frontend's pacing decides when it says (between_fraction)
        if zakuro_core::native_gfx::between_fraction().is_some() {
            return;
        }
        let t = took.as_secs_f64();
        self.cost = if self.cost == 0.0 { t } else { self.cost * 0.9 + t * 0.1 };
        let (down, up) = (self.cost > 0.004 && self.step + 1 < BETWEEN_STEPS.len(), self.cost < 0.002 && self.step > 0);
        if !(down || up) {
            self.since = None;
            return;
        }
        let since = *self.since.get_or_insert_with(std::time::Instant::now);
        let wait = if down { 0.25 } else { 2.0 };
        if since.elapsed().as_secs_f64() >= wait {
            self.step = if down { self.step + 1 } else { self.step - 1 };
            self.since = None;
            self.cost = 0.0;
            self.changes += 1;
            zakuro_gpu::smooth::point("hfrscale", std::time::Instant::now(), self.step as i64, BETWEEN_STEPS[self.step] as f64);
        }
    }
}

const KEPT: usize = 6;

impl State {
    /// a frame drawn on the GPU path before, still in its image.
    pub fn drawn(&self, index: usize, id: u64, base: Option<ash::vk::ImageView>) -> Option<zakuro_gpu::GpuScreen> {
        self.drawn.get(index)?.iter().rev().find(|(i, b, _)| *i == id && *b == base).map(|e| e.2)
    }

    /// the newest picture drawn of a frame before `id`, drawn natively in
    /// full.
    fn before(&self, index: usize, id: u64) -> Option<zakuro_gpu::GpuScreen> {
        self.drawn.get(index)?.iter().filter(|(i, b, _)| *i < id && b.is_none()).max_by_key(|e| e.0).map(|e| e.2)
    }

    /// gfx-final: pictures whose images are gone (a renderer made its
    /// presented images again at a new size, after the top screen changed
    /// shape) are forgotten; drawing from or showing one lost the device
    pub fn prune(&mut self, alive: &dyn Fn(ash::vk::ImageView) -> bool) {
        // rtpace: a renderer lent to the drawing thread still has its images
        let lent = std::mem::take(&mut self.lent_views);
        let alive = |v: ash::vk::ImageView| alive(v) || lent.contains(&v);
        for d in 0..2 {
            self.drawn[d].retain(|e| alive(e.2.view()));
            self.between[d].retain(|e| alive(e.3.view()));
        }
        self.lent_views = lent;
    }

    /// rtpace: a texture to forget, for the renderers lent to the drawing
    /// thread once they are back.
    pub(crate) fn forget_when_back(&mut self, key: (crate::prim::TexRef, u64)) {
        if !self.drawing.is_empty() {
            self.forget_later.push(key);
        }
    }

    /// a picture went into an image: whatever was in it is gone.
    fn forget_view(&mut self, view: ash::vk::ImageView) {
        for d in 0..2 {
            self.drawn[d].retain(|e| e.2.view() != view);
            self.between[d].retain(|e| e.3.view() != view);
        }
    }

    pub fn note_drawn(&mut self, index: usize, id: u64, base: Option<ash::vk::ImageView>, picture: zakuro_gpu::GpuScreen) {
        if index > 1 {
            return;
        }
        self.forget_view(picture.view());
        self.drawn[index].push_back((id, base, picture));
        while self.drawn[index].len() > KEPT {
            self.drawn[index].pop_front();
        }
    }

    fn note_between(&mut self, index: usize, id: u64, at: f32, base: Option<ash::vk::ImageView>, picture: zakuro_gpu::GpuScreen, fraction: f32) {
        self.forget_view(picture.view());
        self.between[index].push_back((id, at.to_bits(), base, picture, fraction.to_bits()));
        while self.between[index].len() > KEPT {
            self.between[index].pop_front();
        }
    }
}

impl crate::Gfx {
    /// a complete frame: its draws matched against the display's frame
    /// before, while frame interpolation is on.
    pub(crate) fn hfr_track(&mut self, system: &mut zakuro_core::System, frame: &mut crate::Frame) {
        let index = frame.disp as usize;
        if index > 1 {
            return;
        }
        if !enabled() || !system.gpu.interpolates() {
            self.hfr.last[index] = None;
            return;
        }
        note_game_frame(frame.id); // gfxmodels3
        let before = self.hfr.last[index].as_ref().map(|f| (f.draws.as_slice(), f.hfr.as_deref()));
        let traced = crate::perf::var("FATES_GFX_HFR_TRACE2D").ok().and_then(|v| {
            let (lo, hi) = v.split_once('-')?;
            Some((lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?))
        });
        let on = index == 0 && traced.is_some_and(|(lo, hi)| frame.id >= lo && frame.id <= hi);
        TRACE_2D.store(on, std::sync::atomic::Ordering::Relaxed);
        if on {
            // gfxmodels3: with the game's frame
            eprintln!("gfx-hfr 2d: frame {} game {}", frame.id, zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed));
        }
        let mut between = track(before, &frame.draws);
        TRACE_2D.store(false, std::sync::atomic::Ordering::Relaxed);
        // castlecam: the ghosts' textures, as the frame before had them
        if let Some(last) = self.hfr.last[index].as_ref() {
            for g in &mut between.ghosts {
                g.keys = last.texture_keys.get(g.from).copied().unwrap_or([None; 3]);
            }
        }
        // gfx-models: a frame whose post effects read what earlier frames
        // left in their surfaces has no right picture in between (carries)
        if frame.draws.iter().any(|d| d.post.as_deref().is_some_and(carries)) {
            for t in &mut between.tracks {
                t.moves = false;
            }
            between.matches.moved = 0;
            between.carried = true; // smooth2
            self.hfr.carried += 1;
            // castlecam: dev, FATES_GFX_HFR_TRACE=carry: which post blocks carry
            if crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "carry") {
                for (n, d) in frame.draws.iter().enumerate() {
                    if let Some(b) = d.post.as_deref().filter(|b| carries(b)) {
                        let ops: Vec<String> = b.ops.iter().map(|op| format!("{op:?}").chars().take(160).collect()).collect();
                        eprintln!("castlecam carry: frame {} game {} draw {n} of {} ops {}: {}", frame.id, zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed), frame.draws.len(), b.ops.len(), ops.join(" | "));
                    }
                }
            }
            // smooth2: in the timing log, to find the battle transitions
            zakuro_gpu::smooth::point("carried", std::time::Instant::now(), frame.id as i64, index as f64);
        }
        // transblur: in the timing log, the frames the game draws with the
        // radial blur's strength set (post effect parameters +9, the battle
        // transitions'), whether or not the blur is drawn
        if index == 0 && zakuro_gpu::smooth::enabled() {
            let strength = crate::prim::r8(system, zakuro_core::region::us(0x0070_091D));
            if strength != 0 {
                zakuro_gpu::smooth::point("tblur", std::time::Instant::now(), frame.id as i64, strength as f64);
            }
        }
        // gfxleft: a frame that lost a model is held (see track)
        if between.vanished > 0 && !between.carried {
            for t in &mut between.tracks {
                t.moves = false;
            }
            between.matches.moved = 0;
            self.hfr.vanished += 1;
            if crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "1") {
                eprintln!("gfxleft: frame {} game {} held, {} models gone or jumped, {:.3} / {:.3} of the screen", frame.id, zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed), between.vanished, between.vanished_share, between.jumped_share);
            }
        } else if between.vanished_share + between.jumped_share > 0.0 && crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "1") {
            eprintln!("gfxleft: frame {} game {} not held, models gone / jumped covered {:.3} / {:.3} of the screen", frame.id, zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed), between.vanished_share, between.jumped_share);
        }
        if index == 0 {
            crate::mapjit::frame(frame.id, &frame.draws, &between); // mapjit: dev, FATES_MAPJIT_LOG
        }
        self.hfr.tracked += 1;
        self.hfr.smooth += between.matches.smooth() as u64;
        frame.hfr = Some(Arc::new(between));
        if crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "1" || v == "2") {
            let m = frame.hfr.as_ref().unwrap().matches;
            eprintln!("gfx-hfr: frame {} disp {} {:?} smooth {} game {}", frame.id, frame.disp, m, m.smooth(), zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed)); // gfxmap: with the game frame
        }
    }

    /// a frame complete and shared: its draws are what the display's next
    /// frame matches against.
    pub(crate) fn hfr_commit(&mut self, frame: &Arc<crate::Frame>) {
        let index = frame.disp as usize;
        if index < 2 && frame.hfr.is_some() {
            self.hfr.last[index] = Some(frame.clone());
        }
    }

    /// the native frame for a display buffer, see NativeScreens::newest.
    pub(crate) fn hfr_newest(&mut self, index: usize, address: u32) -> Option<(u64, Option<zakuro_gpu::Matches>)> {
        let frame = self.committed.get(&address)?;
        if frame.disp as usize != index {
            return None;
        }
        // smooth2: whether the newest frame carries (cross-faded, not drawn in between)
        zakuro_core::native_gfx::set_newest_carried(index, frame.hfr.as_ref().is_some_and(|b| b.carried));
        // gfx-models: pacing asks for the upper display at every refresh
        if index == 0 && frame.id != self.hfr.top_ids.0 {
            self.hfr.top_ids = (frame.id, self.hfr.top_ids.0);
        }
        Some((frame.id, frame.hfr.as_ref().map(|b| b.matches.as_zakuro())))
    }

    /// the address of the frame committed before the one at `address`, on
    /// the same display.
    fn hfr_previous_address(&self, index: usize, address: u32) -> Option<u32> {
        let id = self.committed.get(&address)?.id;
        self.committed.iter().filter(|(_, f)| f.disp as usize == index && f.id < id).max_by_key(|(_, f)| f.id).map(|(a, _)| *a)
    }

    /// the GPU path at a pick of the frame's time: the frame, the one
    /// before it, or a picture in between drawn now.
    pub(crate) fn compose_gpu(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        address: u32,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        pick: zakuro_core::native_gfx::Pick,
    ) -> Option<zakuro_gpu::GpuScreen> {
        let picture = self.compose_gpu_pick(system, index, address, emulated, size, pick);
        // sharp: a frame's full-scale picture shown while the pictures are
        // drawn smaller (the paths above that fall back to it: no frame
        // before kept, nothing to draw in between) is shown at their scale
        let picture = match picture {
            Some(p) if index <= 1 && still_follows() && self.hfr.scaling.frames_follow() && !frames_reduced() && self.gpu.as_ref().is_some_and(|r| r.presents(p.view())) => {
                self.follow_scale(system, index, p, emulated, size).or(Some(p))
            }
            p => p,
        };
        if index == 0 {
            if let Some(p) = picture {
                self.seq_shot(index, p, pick); // gfxcastle: dev, FATES_GFX_SEQ
                // sharp: upper pictures handed over while pictures are drawn
                // smaller, by whether the frames' renderer drew it (full
                // scale) or the in-between renderer (the smaller scale); and
                // in the timing log (a = 1 full scale, b = the fraction)
                if self.hfr.scaling.frames_follow() {
                    let full = self.gpu.as_ref().is_some_and(|r| r.presents(p.view())) && !frames_reduced();
                    if full { self.hfr.shown_full += 1 } else { self.hfr.shown_scaled += 1 }
                    if zakuro_gpu::smooth::enabled() {
                        zakuro_gpu::smooth::point("sharpfull", std::time::Instant::now(), full as i64, self.hfr.scaling.fraction() as f64);
                    }
                }
            }
        }
        picture
    }

    /// gfxcastle: dev, FATES_GFX_SEQ=dir with FATES_GFX_SEQ_GAME=lo-hi: every
    /// upper-screen picture handed to the presenter while the game's frame is
    /// in lo..=hi, in order, as dir/NNNNN-<pick>-<w>x<h>.png, cropped to
    /// FATES_GFX_SEQ_RECT=x0,y0,x1,y1 (fractions of the picture, default the
    /// middle). Frames and pictures in between alike, at the size each was
    /// drawn at: what the display was given, to see whether detail holds.
    fn seq_shot(&mut self, index: usize, picture: zakuro_gpu::GpuScreen, pick: zakuro_core::native_gfx::Pick) {
        let Ok(dir) = crate::perf::var("FATES_GFX_SEQ") else { return };
        let Some((lo, hi)) = crate::perf::var("FATES_GFX_SEQ_GAME").ok().and_then(|v| {
            let (lo, hi) = v.split_once('-')?;
            Some((lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?))
        }) else {
            return;
        };
        let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
        if game < lo || game > hi {
            return;
        }
        let rect: Vec<f32> = crate::perf::var("FATES_GFX_SEQ_RECT")
            .ok()
            .map(|v| v.split(',').filter_map(|f| f.trim().parse().ok()).collect())
            .filter(|r: &Vec<f32>| r.len() == 4)
            .unwrap_or_else(|| vec![0.30, 0.15, 0.70, 0.85]);
        let view = picture.view();
        let renderers = [self.gpu.as_mut(), self.hfr.renderer.as_mut()];
        let Some(renderer) = renderers.into_iter().flatten().find(|r| r.presents(view)) else { return };
        let (w, h) = renderer.present_size(index);
        let (x0, y0) = ((rect[0] * w as f32) as u32, (rect[1] * h as f32) as u32);
        let (x1, y1) = (((rect[2] * w as f32) as u32).min(w), ((rect[3] * h as f32) as u32).min(h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        // vein2: only the rectangle read back, and written on a thread of
        // its own, so the capture holds the presenter up far less
        let Ok(crop) = renderer.read_present_rect(index, view, (x0, y0, x1 - x0, y1 - y0)) else { return };
        let tag = match pick {
            zakuro_core::native_gfx::Pick::Newest => "n".to_owned(),
            zakuro_core::native_gfx::Pick::Previous => "p".to_owned(),
            zakuro_core::native_gfx::Pick::Between(at) => format!("b{:03}", (at * 1000.0).round() as i32),
        };
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::create_dir_all(&dir);
        let path = std::path::Path::new(&dir).join(format!("{n:05}-g{game}-{tag}-{w}x{h}.png"));
        std::thread::spawn(move || {
            let _ = crate::write_png(&path, &crop, x1 - x0, y1 - y0);
        });
    }

    fn compose_gpu_pick(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        address: u32,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        pick: zakuro_core::native_gfx::Pick,
    ) -> Option<zakuro_gpu::GpuScreen> {
        use zakuro_core::native_gfx::Pick;
        let pick = if enabled() { pick } else { Pick::Newest };
        // gfx-models: dev, FATES_GFX_PACE_LOG=file: each upper-screen pick
        // (time, frame shown, pick) for checking the time shown per present
        if index == 0 {
            pace_log(self.committed.get(&address).map_or(0, |f| f.id), pick);
        }
        // gfx-models: pacing's picks are the upper display's picture's
        // time. A lower-display frame from before the upper one's frame
        // before (the lower screen unchanged since, its repeats left out)
        // has no part in that time: its picture before or in between would
        // show a change long past again, a flash, at every upper frame
        let pick = match pick {
            Pick::Previous | Pick::Between(_) if index == 1 && self.committed.get(&address).is_some_and(|f| f.id <= self.hfr.top_ids.1) => {
                self.hfr.stale_lower += 1;
                Pick::Newest
            }
            pick => pick,
        };
        match pick {
            Pick::Newest => {
                let newest = self.compose_gpu_frame(system, index, address, emulated, size);
                // gfxcastle: while pictures in between are drawn smaller, the
                // frame is shown at their scale too (see reduced_frame)
                if self.hfr.scaling.frames_follow() {
                    self.prune_pictures();
                    if let Some(frame) = self.committed.get(&address).cloned() {
                        if let Some(p) = self.reduced_frame(system, index, frame, newest, emulated, size) {
                            return Some(p);
                        }
                    }
                }
                newest
            }
            Pick::Previous => {
                // the frames up to the newest drawn first, in order
                let newest = self.compose_gpu_frame(system, index, address, emulated, size);
                self.prune_pictures(); // gfx-final: the newest can have made the images again
                let Some(before) = self.hfr_previous_address(index, address) else { return newest };
                // still in its image (a hybrid: drawn over the emulated
                // picture before, which the presenter passes for this pick)
                let frame = self.committed.get(&before)?.clone();
                let base = if frame.hybrid { emulated.map(|e| e.view()) } else { None };
                let previous = self.hfr.drawn(index, frame.id, base).or(newest);
                // gfxcastle: at the scale of the pictures in between, as the newest
                if self.hfr.scaling.frames_follow() {
                    if let Some(p) = self.reduced_frame(system, index, frame, previous, emulated, size) {
                        return Some(p);
                    }
                }
                previous
            }
            Pick::Between(at) => {
                let newest = self.compose_gpu_frame(system, index, address, emulated, size);
                self.prune_pictures(); // gfx-final
                // gfx-models: a negative place is before the frame before:
                // between the one before that (-1) and it (0), drawn from
                // the frame before's own matches. Pacing shows that when its
                // time runs behind the frame before's (after the game was
                // late), rather than holding the frame before
                let (frame, at, newest) = if at < 0.0 {
                    let Some(before) = self.hfr_previous_address(index, address) else { return newest };
                    let Some(frame) = self.committed.get(&before).cloned() else { return newest };
                    // the frame before's own picture (Pick::Previous)
                    let previous = self.hfr.drawn(index, frame.id, None).or(newest);
                    if frame.hybrid || previous.is_none() {
                        return previous;
                    }
                    self.hfr.before_previous += 1;
                    (frame, (at + 1.0).clamp(0.0, 1.0), previous)
                } else {
                    (self.committed.get(&address)?.clone(), at, newest)
                };
                let Some(between) = frame.hfr.clone().filter(|b| b.moves() && frame.disp as usize == index) else {
                    // sharp: nothing moves: the frame, at the scale the
                    // pictures are at (see reduced_frame)
                    if self.hfr.scaling.frames_follow() && still_follows() {
                        if let Some(p) = self.reduced_frame(system, index, frame, newest, emulated, size) {
                            return Some(p);
                        }
                    }
                    return newest;
                };
                self.draw_between(system, index, frame, &between, at, newest, emulated, size, false)
            }
        }
    }

    /// gfxcastle: a frame's own picture drawn by the in-between renderer at
    /// the scale pictures in between are drawn at, when they are drawn
    /// smaller (the GPU fell behind, see BetweenScale). smooth2 drew only the
    /// pictures in between smaller: at 8x in a 30 Hz scene five of six
    /// pictures shown were soft and every sixth sharp, so fine detail (trees,
    /// roof tiles, paving) pulsed at 30 Hz while the view moved. Now every
    /// picture of a moving stretch is drawn at one scale.
    /// sharp: frames with nothing moving too. gfxcastle left them at full
    /// scale, so the picture sharpened each time the view came to rest and
    /// softened when it moved again (8x shown in a 1440-row window against
    /// 6x). Now while the scale is down every picture is at it, and a change
    /// of the scale (rare, see the frontend's pacing) is the only change.
    fn reduced_frame(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        frame: Arc<crate::Frame>,
        full: Option<zakuro_gpu::GpuScreen>,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
    ) -> Option<zakuro_gpu::GpuScreen> {
        // sharp: none when the frames are drawn at the smaller scale already
        if frame.hybrid || frame.disp as usize != index || full.is_none() || frames_reduced() {
            return None;
        }
        let between = match frame.hfr.clone() {
            Some(b) if b.carried => return None,
            Some(b) if b.moves() => b,
            // sharp: drawn as it is (own), the matches are not used
            _ if still_follows() => Arc::new(Between::default()),
            _ => return None,
        };
        let picture = self.draw_between(system, index, frame, &between, 1.0, full, emulated, size, true)?;
        // draw_between falls back to the full picture when it cannot draw
        if full.is_some_and(|f| f.view() == picture.view()) {
            return None;
        }
        self.hfr.reduced_frames += 1;
        Some(picture)
    }

    /// sharp: the frame whose full-scale picture `full` is, drawn at the
    /// pictures' smaller scale (see reduced_frame); None when it cannot be.
    fn follow_scale(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        full: zakuro_gpu::GpuScreen,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
    ) -> Option<zakuro_gpu::GpuScreen> {
        let id = self.hfr.drawn[index].iter().rev().find(|e| e.2.view() == full.view())?.0;
        let frame = self.committed.values().find(|f| f.id == id && f.disp as usize == index)?.clone();
        self.reduced_frame(system, index, frame, Some(full), emulated, size)
    }

    /// a picture in between of `frame` at `at` (or, `own`, the frame's own
    /// draws as they are), at the fraction of the scale BetweenScale says;
    /// `newest` is the frame's picture, shown when it cannot be drawn.
    #[allow(clippy::too_many_arguments)]
    fn draw_between(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        frame: Arc<crate::Frame>,
        between: &Arc<Between>,
        at: f32,
        newest: Option<zakuro_gpu::GpuScreen>,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        own: bool,
    ) -> Option<zakuro_gpu::GpuScreen> {
        // rtpace: one drawn on the drawing thread comes back first (it is
        // usually this very picture, which the cache below then has)
        self.reclaim_between();
        let drawing = std::time::Instant::now();
        let job = match self.prepare_between(system, index, frame, between, at, newest, emulated, size, own) {
            Ok(job) => job,
            Err(early) => return early,
        };
        let own_renderer = !crate::perf::var("FATES_GFX_HFR_RENDERER").is_ok_and(|v| v == "shared");
        let lower = index == 1 && self.hfr.renderer_lower.is_some();
        let renderer = match (lower, self.hfr.renderer.as_mut().filter(|_| own_renderer)) {
            (true, _) if own_renderer => self.hfr.renderer_lower.as_mut()?,
            (_, Some(r)) => r,
            _ => self.gpu.as_mut()?,
        };
        let drawn = job.draw(renderer, &self.decoded);
        self.finish_between(job, drawn, drawing.elapsed())
    }

    /// rtpace: what drawing a picture in between needs (draw_between's
    /// start, unchanged): a job, or what to show instead (the cached picture,
    /// or the newest frame's when none can be drawn). Makes the in-between
    /// renderers when there are none yet.
    #[allow(clippy::too_many_arguments)]
    fn prepare_between(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        frame: Arc<crate::Frame>,
        between: &Arc<Between>,
        at: f32,
        newest: Option<zakuro_gpu::GpuScreen>,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        own: bool,
    ) -> Result<BetweenJob, Option<zakuro_gpu::GpuScreen>> {
        let base_view = if frame.hybrid { emulated.map(|e| e.view()) } else { None };
        // gfxcastle: the frame's own picture is kept under a place no
        // picture in between has (pacing's places are 0..1 exclusive)
        let place = if own { OWN_PLACE } else { at };
        // gfxcastle: and at the scale it was drawn at
        let fraction = self.hfr.scaling.fraction();
        if let Some(e) = self.hfr.between[index].iter().find(|e| e.0 == frame.id && e.1 == place.to_bits() && e.2 == base_view && e.4 == fraction.to_bits()) {
            return Err(Some(e.3));
        }
        let (width, height) = size;
        let (lw, _) = crate::prim::display_size(frame.disp);
        let scale = height / lw as u32;
        if scale == 0 || newest.is_none() {
            return Err(newest);
        }
        // smooth2: at a fraction of the scale while the GPU is behind
        let (width, height) = ((width as f32 * fraction).round().max(1.0) as u32, (height as f32 * fraction).round().max(1.0) as u32);
        let start = if frame.hybrid {
            let Some(e) = emulated else { return Err(None) };
            JobStart::View(e.view(), e.area())
        } else if let Some((rgba, _)) = frame.clear {
            JobStart::Clear(rgba)
        } else {
            // drawn over the frame before's picture
            match self.hfr.before(index, frame.id) {
                Some(p) => JobStart::View(p.view(), p.area()),
                None => {
                    self.hfr.fallbacks += 1;
                    return Err(newest);
                }
            }
        };
        let own_renderer = !crate::perf::var("FATES_GFX_HFR_RENDERER").is_ok_and(|v| v == "shared");
        // rtpace: a renderer lent to the drawing thread is still there
        let lent = |i: usize| self.hfr.drawing.iter().any(|(index, _)| *index == i);
        let (top_lent, lower_lent) = (lent(0), lent(1));
        if own_renderer && self.hfr.renderer.is_none() && !top_lent && !self.hfr.renderer_failed {
            match system.config.device.clone().map(crate::render::Renderer::new_shared).map(|r| r.map(|mut r| { r.between = true; r })) { // perf3: marked
                Some(Ok(r)) => self.hfr.renderer = Some(r),
                Some(Err(e)) => {
                    eprintln!("gfx-hfr: no renderer of its own for pictures in between ({e}), the frames' draws them");
                    self.hfr.renderer_failed = true;
                }
                None => self.hfr.renderer_failed = true,
            }
        }
        // gfx-final: the lower display's pictures in between have a
        // renderer of their own too
        if own_renderer && index == 1 && super::split_renderers() && (self.hfr.renderer.is_some() || top_lent) && self.hfr.renderer_lower.is_none() && !lower_lent {
            if let Some(Ok(mut r)) = system.config.device.clone().map(crate::render::Renderer::new_shared) {
                r.between = true; // perf3
                self.hfr.renderer_lower = Some(r);
            }
        }
        let depth = frame.clear.map_or(1.0, |c| c.1);
        Ok(BetweenJob { index, frame, between: between.clone(), at, own, place, base_view, fraction, width, height, scale: scale as f32 * fraction, start, depth, newest })
    }

    /// rtpace: a drawn picture in between kept (draw_between's end,
    /// unchanged).
    fn finish_between(&mut self, job: BetweenJob, drawn: Result<crate::render::Output, String>, took: std::time::Duration) -> Option<zakuro_gpu::GpuScreen> {
        self.hfr.scaling.took(took); // smooth2
        let BetweenJob { index, frame, at, own, place, base_view, fraction, newest, .. } = job;
        match drawn {
            Ok(crate::render::Output::Screen(view, w, h)) => {
                let (w, h) = (w as f32, h as f32);
                let picture = zakuro_gpu::GpuScreen::new(view, [0.0, 0.0, 1.0, 1.0], [0.5 / w, 0.5 / h, 1.0 - 0.5 / w, 1.0 - 0.5 / h]);
                self.hfr.note_between(index, frame.id, place, base_view, picture, fraction);
                if own {
                    self.hfr_shot_own(index, frame.id, picture);
                } else {
                    self.hfr.drawn_between += 1;
                    self.hfr_shot(index, frame.id, at, picture, newest);
                }
                Some(picture)
            }
            Ok(_) => newest,
            Err(e) => {
                eprintln!("gfx: picture in between failed, {e}");
                newest
            }
        }
    }

    /// rtpace: real time: the upper screen's picture in between at `at`, as
    /// compose_gpu_pick(Pick::Between(at)) would draw it, started on the
    /// drawing thread (rtpace.rs). False when none was started.
    pub(crate) fn begin_between(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        address: u32,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        at: f32,
    ) -> bool {
        if index > 1 || !enabled() || !crate::rtpace::enabled() || crate::perf::var("FATES_GFX_HFR_RENDERER").is_ok_and(|v| v == "shared") {
            return false;
        }
        // the lower display's only with a renderer of its own (else both
        // would need the one renderer)
        if index == 1 && !super::split_renderers() {
            return false;
        }
        // one per display at a time
        if self.hfr.drawing.iter().any(|(i, _)| *i == index) {
            return false;
        }
        // compose_gpu_pick's Pick::Between, up to the drawing: a lower
        // display's frame older than the upper's picture is shown as it is
        if index == 1 && self.committed.get(&address).is_some_and(|f| f.id <= self.hfr.top_ids.1) {
            return false;
        }
        let newest = self.compose_gpu_frame(system, index, address, emulated, size);
        self.prune_pictures();
        // gfx-models: a negative place is in the frame before's own time
        // (as compose_gpu_pick draws it)
        let (frame, at, newest) = if at < 0.0 {
            let Some(before) = self.hfr_previous_address(index, address) else { return false };
            let Some(frame) = self.committed.get(&before).cloned() else { return false };
            let previous = self.hfr.drawn(index, frame.id, None).or(newest);
            if frame.hybrid || previous.is_none() {
                return false;
            }
            (frame, (at + 1.0).clamp(0.0, 1.0), previous)
        } else {
            let Some(frame) = self.committed.get(&address).cloned() else { return false };
            (frame, at, newest)
        };
        let Some(between) = frame.hfr.clone().filter(|b| b.moves() && frame.disp as usize == index) else { return false };
        let Ok(job) = self.prepare_between(system, index, frame, &between, at, newest, emulated, size, false) else { return false };
        let renderer = match index {
            0 => self.hfr.renderer.take(),
            _ => self.hfr.renderer_lower.take(),
        };
        let Some(renderer) = renderer else { return false };
        let views = renderer.present_views();
        match crate::rtpace::send(job, renderer, self.decoded.clone()) {
            Ok(rx) => {
                self.hfr.drawing.push((index, rx));
                self.hfr.lent_views.extend(views);
                true
            }
            Err((_, renderer)) => {
                match index {
                    0 => self.hfr.renderer = Some(renderer),
                    _ => self.hfr.renderer_lower = Some(renderer),
                }
                false
            }
        }
    }

    /// rtpace: the picture in between the drawing thread has, waited for,
    /// kept, and its renderer back.
    pub(crate) fn reclaim_between(&mut self) {
        for (index, rx) in std::mem::take(&mut self.hfr.drawing) {
            let waited = std::time::Instant::now();
            match rx.recv() {
                Ok(done) => {
                    zakuro_gpu::smooth::since("reclaim", waited, index as i64, 0.0);
                    match index {
                        0 => self.hfr.renderer = Some(done.renderer),
                        _ => self.hfr.renderer_lower = Some(done.renderer),
                    }
                    let _ = self.finish_between(done.job, done.drawn, done.took);
                }
                Err(_) => eprintln!("rtpace: the drawing thread lost a picture in between and its renderer; a new one is made"),
            }
        }
        self.hfr.lent_views.clear();
        for key in std::mem::take(&mut self.hfr.forget_later) {
            for r in [self.hfr.renderer.as_mut(), self.hfr.renderer_lower.as_mut()].into_iter().flatten() {
                r.forget_texture(key);
            }
        }
    }

    /// gfxcastle: dev, with FATES_GFX_HFR_SHOT, a frame's own picture drawn
    /// at the reduced scale is written as dir/<disp>-<frame>-r.png, for the
    /// frames FATES_GFX_HFR_SHOT_IDS/_GAME pick.
    fn hfr_shot_own(&mut self, index: usize, id: u64, picture: zakuro_gpu::GpuScreen) {
        let Ok(spec) = crate::perf::var("FATES_GFX_HFR_SHOT") else { return };
        let dir = spec.rsplit_once(':').filter(|(_, n)| n.parse::<u64>().is_ok()).map_or(spec.clone(), |(d, _)| d.to_owned());
        let range = |name: &str| {
            let v = crate::perf::var(name).ok()?;
            let (lo, hi) = v.split_once('-')?;
            Some((lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?))
        };
        let wanted = match (range("FATES_GFX_HFR_SHOT_IDS"), range("FATES_GFX_HFR_SHOT_GAME")) {
            (Some((lo, hi)), _) => id >= lo && id <= hi,
            (None, Some((lo, hi))) => (lo..=hi).contains(&game_frame(id)),
            (None, None) => false,
        };
        if !wanted {
            return;
        }
        let renderer = match if index == 1 && self.hfr.renderer_lower.is_some() { self.hfr.renderer_lower.as_mut() } else { self.hfr.renderer.as_mut() } {
            Some(r) => r,
            None => return,
        };
        if let Ok(pixels) = renderer.read_present(index, picture.view()) {
            let (w, h) = renderer.present_size(index);
            if pixels.len() == (w * h * 4) as usize {
                let _ = std::fs::create_dir_all(&dir);
                let _ = crate::write_png(&std::path::Path::new(&dir).join(format!("{index}-{id:06}-r.png")), &pixels, w, h);
            }
        }
    }

    /// dev, FATES_GFX_HFR_SHOT=dir[:every]: a windowed run writes every
    /// `every`th (default 600) picture in between the GPU path drew, with
    /// the frame before's picture and the frame's own, as
    /// dir/<disp>-<frame>-{a,b,c}.png (a before, b in between at t, c the frame).
    fn hfr_shot(&mut self, index: usize, id: u64, at: f32, between: zakuro_gpu::GpuScreen, newest: Option<zakuro_gpu::GpuScreen>) {
        let Ok(spec) = crate::perf::var("FATES_GFX_HFR_SHOT") else { return };
        let (dir, every) = match spec.rsplit_once(':').filter(|(_, n)| n.parse::<u64>().is_ok()) {
            Some((d, n)) => (d.to_owned(), n.parse::<u64>().unwrap_or(600).max(1)),
            None => (spec.clone(), 600),
        };
        // gfx-models: FATES_GFX_HFR_SHOT_IDS=lo-hi: only frames lo..=hi,
        // every picture in between of them (every then counts in the range)
        let ids = crate::perf::var("FATES_GFX_HFR_SHOT_IDS").ok().and_then(|v| {
            let (lo, hi) = v.split_once('-')?;
            Some((lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?))
        });
        // gfxmodels3: FATES_GFX_HFR_SHOT_GAME=lo-hi: by the game's frames
        let ids = ids.or_else(|| {
            let v = crate::perf::var("FATES_GFX_HFR_SHOT_GAME").ok()?;
            let (lo, hi) = v.split_once('-')?;
            let (lo, hi) = (lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?);
            let game = game_frame(id);
            Some(if game >= lo && game <= hi { (id, id) } else { (u64::MAX, u64::MAX) })
        });
        match ids {
            Some((lo, hi)) if id < lo || id > hi => return,
            Some(_) => {
                self.hfr.shots_in_range += 1;
                if (self.hfr.shots_in_range - 1) % every != 0 {
                    return;
                }
            }
            None if self.hfr.drawn_between % every != 1 => return,
            None => {}
        }
        let before = self.hfr.before(index, id);
        let _ = std::fs::create_dir_all(&dir);
        let mut size = None;
        for (tag, picture) in [("a", before), ("b", Some(between)), ("c", newest)] {
            let Some(picture) = picture else { continue };
            // gfx-final: per display (lib.rs split_renderers)
            let lower = index == 1;
            let renderer = match tag {
                "b" => match if lower && self.hfr.renderer_lower.is_some() { self.hfr.renderer_lower.as_mut() } else { self.hfr.renderer.as_mut() } {
                    Some(r) => r,
                    None => continue,
                },
                _ => match if lower && self.gpu_lower.is_some() { self.gpu_lower.as_mut() } else { self.gpu.as_mut() } {
                    Some(r) => r,
                    None => continue,
                },
            };
            if let Ok(pixels) = renderer.read_present(index, picture.view()) {
                // gfxmodels3, cameras3: each renderer's own size (a picture in between
                // drawn at a fraction of the scale is smaller than its frames)
                let (w, h) = renderer.present_size(index);
                let _ = size.get_or_insert((w, h));
                if pixels.len() == (w * h * 4) as usize {
                    let _ = crate::write_png(&std::path::Path::new(&dir).join(format!("{index}-{id:06}-{tag}.png")), &pixels, w, h);
                }
            }
        }
        eprintln!("gfx-hfr: shot display {index} frame {id} at {at}");
        let _ = shot_names_at(&dir, index, id, at); // gfx-models
    }

    /// the CPU path at a pick of the frame's time (headless dumps): a
    /// picture in between drawn now and read back, else the frame's own.
    pub(crate) fn compose_at(
        &mut self,
        system: &mut zakuro_core::System,
        index: usize,
        address: u32,
        pick: zakuro_core::native_gfx::Pick,
        emulated: &zakuro_core::Screen,
    ) -> Option<zakuro_core::Screen> {
        use zakuro_core::native_gfx::Pick;
        let at = match pick {
            Pick::Between(at) if enabled() => at,
            _ => return self.compose(system, index, address, emulated),
        };
        let frame = self.committed.get(&address)?.clone();
        let between = match frame.hfr.clone().filter(|b| b.moves() && frame.disp as usize == index) {
            Some(b) => b,
            None => return self.compose(system, index, address, emulated),
        };
        let scale = system.gpu.scale().max(1);
        let (lw, lh) = crate::prim::display_size(frame.disp);
        let k = if frame.disp == 0 { zakuro_gpu::wide::factor().max(1.0) } else { 1.0 };
        let (uw, uh) = ((lh as f32 * scale as f32 * k).round() as u32, lw as u32 * scale);
        let before_view = match self.cache.pictures[index].iter().filter(|(id, _)| *id < frame.id).max_by_key(|(id, _)| *id) {
            Some(&(_, view)) => Some(view),
            None => None,
        };
        let (start, width, height) = if frame.hybrid {
            if emulated.0.len() != (emulated.1 * emulated.2 * 4) as usize {
                return self.compose(system, index, address, emulated);
            }
            (crate::render::Start::Pixels(emulated.0.as_slice()), emulated.1, emulated.2)
        } else if let Some((rgba, _)) = frame.clear {
            (crate::render::Start::Clear(rgba), uw, uh)
        } else if let Some(view) = before_view {
            (crate::render::Start::View(view, [0.0, 0.0, 1.0, 1.0]), uw, uh)
        } else {
            self.hfr.fallbacks += 1;
            return self.compose(system, index, address, emulated);
        };
        let _t = crate::perf::scope(crate::perf::C::Between);
        let draws = draws_at(&frame.draws, &between, at);
        self.ensure_renderer();
        let renderer = self.renderer.as_mut()?;
        let depth = frame.clear.map_or(1.0, |c| c.1);
        let keys = crate::castlecam::keys_with_ghosts(&frame.texture_keys, &between.ghosts); // castlecam
        match renderer.render_between(index, width, height, scale as f32, start, depth, &draws, &keys, &self.decoded, crate::render::Finish::Readback) {
            Ok(crate::render::Output::Pixels(pixels)) => {
                self.hfr.drawn_between += 1;
                Some((Arc::new(pixels), width, height))
            }
            Ok(_) => None,
            Err(e) => {
                eprintln!("gfx: picture in between failed, {e}");
                self.compose(system, index, address, emulated)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// castlecam: a camera going round a point (My Castle's follow
    /// camera turning): its view matrix, rows of 3x4, at a yaw (radians)
    fn orbit_view(yaw: f32, pitch: f32) -> [f32; 12] {
        let c = [800.0f32, 38.0, 300.0]; // My Castle's numbers: far from the world's origin
        let d = 30.0f32;
        // the way it looks, and the eye behind the point
        let f = [pitch.cos() * yaw.sin(), -pitch.sin(), pitch.cos() * yaw.cos()];
        let eye = [c[0] - f[0] * d, c[1] - f[1] * d, c[2] - f[2] * d];
        let norm = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let z = norm([-f[0], -f[1], -f[2]]);
        let x = norm(cross([0.0, 1.0, 0.0], z));
        let y = cross(z, x);
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        [x[0], x[1], x[2], -dot(x, eye), y[0], y[1], y[2], -dot(y, eye), z[0], z[1], z[2], -dot(z, eye)]
    }

    #[test]
    fn a_camera_going_round_is_drawn_turned_part_of_the_way() {
        // 23 degrees a frame (a fast mouse turn), pitched down a little
        let (a, b) = (0.3f32, 0.3 + 23f32.to_radians());
        let (before, now) = (orbit_view(a, 0.35), orbit_view(b, 0.35));
        for t in [0.25f32, 0.5, 0.75] {
            let want = orbit_view(a + (b - a) * t, 0.35);
            let got = rigid_at(&before, &now, t).unwrap();
            for k in 0..12 {
                assert!((got[k] - want[k]).abs() < 2e-3, "t {t} component {k}: {} against {}", got[k], want[k]);
            }
        }
        // straight (as before), the translation is off by the chord
        let half = orbit_view((a + b) / 2.0, 0.35);
        let chord: f32 = (0..3).map(|r| (before[r * 4 + 3] + now[r * 4 + 3]) / 2.0 - half[r * 4 + 3]).map(|v| v * v).sum::<f32>().sqrt();
        assert!(chord > 0.1, "the chord is {chord} units off the arc");
    }

    #[test]
    fn a_move_without_a_turn_stays_straight() {
        let before = [1.0, 0.0, 0.0, 5.0, 0.0, 1.0, 0.0, -2.0, 0.0, 0.0, 1.0, 40.0];
        let now = [1.0, 0.0, 0.0, 9.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, 30.0];
        let got = rigid_at(&before, &now, 0.5).unwrap();
        assert_eq!([got[3], got[7], got[11]], [7.0, 0.0, 35.0]);
    }

    fn prim(x: f32, verts: usize) -> Draw {
        let mut d: Draw = crate::post::as_draw(Arc::new(crate::post::Block::default()));
        d.post = None;
        d.vertices = vec![0; verts * 24];
        d.indices = vec![0; verts];
        d.world = [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        d.view_proj = [0.0; 16];
        d
    }

    /// a 2D draw that moved is placed part of the way, one that stayed or
    /// is new is drawn as it is.
    #[test]
    fn prim_draws_move_part_of_the_way() {
        let f0 = vec![prim(0.0, 3), prim(4.0, 6)];
        let b0 = track(None, &f0);
        assert_eq!(b0.matches.matched, 0);
        let f1 = vec![prim(0.5, 3), prim(4.0, 6), prim(1.0, 9)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert_eq!(b1.matches, Matches { draws: 3, matched: 2, moved: 1, jumped: 0, jumping: 0 });
        assert!(b1.matches.smooth());
        let at = draws_at(&f1, &b1, 0.5);
        assert!((at[0].world[3] - 0.25).abs() < 1e-6);
        assert_eq!(at[1].world[3], 4.0);
        assert_eq!(at[2].world[3], 1.0);
        let at0 = draws_at(&f1, &b1, 0.0);
        assert_eq!(at0[0].world[3], 0.0);
    }

    fn quad(x: f32, u: f32, alpha: u8) -> Draw {
        let mut d = prim(0.0, 4);
        for (k, (dx, dy)) in [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)].into_iter().enumerate() {
            let v = &mut d.vertices[k * 24..k * 24 + 24];
            v[0..4].copy_from_slice(&(x + dx).to_le_bytes());
            v[4..8].copy_from_slice(&(50.0f32 + dy).to_le_bytes());
            v[12..16].copy_from_slice(&(u + dx / 10.0).to_le_bytes());
            v[20..24].copy_from_slice(&[255, 255, 255, alpha]);
        }
        d
    }

    /// gfx-final: a quad the game slid by rewriting its vertices, and one
    /// that fades by its vertex colours, are placed part of the way; one
    /// whose texture coordinates changed (another sprite) is not; a colour
    /// uniform fades, a cut stays.
    #[test]
    fn vertex_moves_and_fades() {
        let f0 = vec![quad(100.0, 0.0, 255), quad(20.0, 0.0, 0), quad(60.0, 0.0, 255)];
        let b0 = track(None, &f0);
        let f1 = vec![quad(104.0, 0.0, 255), quad(20.0, 0.0, 64), quad(60.0, 0.5, 255)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        let at = draws_at(&f1, &b1, 0.5);
        assert_eq!(vertex_pos(&at[0].vertices, 0)[0], 102.0);
        assert_eq!(at[1].vertices[23], 32);
        assert_eq!(vertex_pos(&at[2].vertices, 0)[0], 60.0);
        assert_eq!(at[2].vertices[12..16], f1[2].vertices[12..16]);
        let mut g0 = prim(0.0, 3);
        g0.color = [0.0, 0.0, 0.0, 0.2];
        let mut g1 = prim(0.0, 3);
        g1.color = [0.0, 0.0, 0.0, 0.4];
        let (a, b) = (vec![g0], vec![g1]);
        let bb = track(Some((&a, None)), &b);
        assert!((draws_at(&b, &bb, 0.5)[0].color[3] - 0.3).abs() < 1e-6);
        let mut h0 = prim(0.0, 3);
        h0.color = [0.0, 0.0, 0.0, 0.0];
        let mut h1 = prim(0.0, 3);
        h1.color = [1.0, 1.0, 1.0, 1.0];
        let (a, b) = (vec![h0], vec![h1]);
        let bb = track(Some((&a, None)), &b);
        assert_eq!(draws_at(&b, &bb, 0.5)[0].color, [1.0; 4]);
    }

    /// gfx-2d: a highlight whose alpha test reference follows its vertex
    /// alpha (the name entry's ABC box) keeps the two together in between.
    #[test]
    fn alpha_reference_follows_the_fade() {
        let with_ref = |alpha: u8| {
            let mut d = quad(20.0, 0.0, alpha);
            d.alpha_test = true;
            d.alpha_func = 7;
            d.alpha_ref = alpha;
            d
        };
        let f0 = vec![with_ref(171)];
        let b0 = track(None, &f0);
        let f1 = vec![with_ref(175)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        for t in [0.0, 0.25, 0.5, 0.75] {
            let at = draws_at(&f1, &b1, t);
            assert_eq!(at[0].vertices[23], at[0].alpha_ref, "at {t}");
        }
        assert_eq!(draws_at(&f1, &b1, 0.0)[0].alpha_ref, 171);
        // a reference that jumps (a cut) stays as the frame has it
        let f2 = vec![with_ref(10)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        assert_eq!(draws_at(&f2, &b2, 0.0)[0].alpha_ref, 10);
    }

    /// gfx-models: a rigid matrix turned half way keeps its size (moving
    /// each number straight shrank it), and lands between the two.
    #[test]
    fn rigid_matrices_turn() {
        let rot = |a: f32, tx: f32| [a.cos(), -a.sin(), 0.0, tx, a.sin(), a.cos(), 0.0, 0.0, 0.0, 0.0, 1.0, 5.0];
        let (a, b) = (rot(0.0, 0.0), rot(1.5, 4.0));
        let m = rigid_at(&a, &b, 0.5).unwrap();
        let want = rot(0.75, 2.0);
        for k in 0..12 {
            assert!((m[k] - want[k]).abs() < 1e-4, "{k}: {m:?} {want:?}");
        }
        // scaled too
        let sc = |m: [f32; 12], s: f32| { let mut o = m; for r in 0..3 { for c in 0..3 { o[r * 4 + c] *= s; } } o };
        let m = rigid_at(&sc(a, 2.0), &sc(b, 2.0), 0.5).unwrap();
        assert!((m[0] - 2.0 * 0.75f32.cos()).abs() < 1e-4);
        // a projection is no rigid matrix
        let p = [1.5, 0.0, 0.0, 0.0, 0.0, 2.4, 0.0, 0.0, 0.0, 0.0, -1.0, -0.2];
        assert!(rigid(&p).is_none());
    }

    /// gfx-models: a quad stretched with its texture as it was (a window
    /// opening) moves each vertex part of the way.
    #[test]
    fn stretched_quads_move() {
        let mut a = quad(100.0, 0.0, 255);
        let b = quad(100.0, 0.0, 255);
        // the right side 16 further right
        let mut c = b.clone();
        for k in [1usize, 3] {
            let x = vertex_pos(&c.vertices, k)[0] + 16.0;
            c.vertices[k * 24..k * 24 + 4].copy_from_slice(&x.to_le_bytes());
        }
        a.vertices.clone_from(&b.vertices);
        let (f0, f1) = (vec![a], vec![c]);
        let b0 = track(None, &f0);
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        let at = draws_at(&f1, &b1, 0.5);
        assert_eq!(vertex_pos(&at[0].vertices, 1)[0], 118.0);
        assert_eq!(vertex_pos(&at[0].vertices, 0)[0], 100.0);
    }

    /// gfx-models: two boxes drawn the same way with the same matrices are
    /// matched by where they are and their colour, not their order: a box
    /// added in front does not make the highlight match a plain box.
    #[test]
    fn boxes_match_by_place_and_colour() {
        let plain = |x: f32| quad(x, 0.0, 255);
        let mut hl = quad(100.0, 0.0, 255);
        hl.color = [1.0, 0.4, 0.2, 1.0];
        let f0 = vec![plain(10.0), hl.clone()];
        let b0 = track(None, &f0);
        let f1 = vec![plain(10.0), plain(100.0), hl.clone()];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        // the highlight did not change: drawn as it is, no colour fade
        let at = draws_at(&f1, &b1, 0.0);
        assert_eq!(at[2].color, [1.0, 0.4, 0.2, 1.0]);
    }

    /// gfx-models: a highlight pulsing its vertex alpha with its alpha
    /// test's reference keeps the two together in between.
    #[test]
    fn alpha_reference_fades_with_alpha() {
        let mut a = quad(100.0, 0.0, 187);
        a.alpha_test = true;
        a.alpha_func = 7;
        a.alpha_ref = 187;
        let mut b = quad(100.0, 0.0, 191);
        b.alpha_test = true;
        b.alpha_func = 7;
        b.alpha_ref = 191;
        let (f0, f1) = (vec![a], vec![b]);
        let b0 = track(None, &f0);
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        for t in [0.0, 0.5] {
            let at = draws_at(&f1, &b1, t);
            assert_eq!(at[0].vertices[23], at[0].alpha_ref, "t {t}");
        }
    }

    /// gfx-models: a slide from rest (a few dozen pixels in a frame) moves;
    /// a far teleport still jumps.
    #[test]
    fn slides_from_rest_move() {
        let f0 = vec![prim(-3.0, 3)];
        let b0 = track(None, &f0);
        let f1 = vec![prim(13.0, 3)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks[0].moves, "{:?}", b1.matches);
        let f2 = vec![prim(51.0, 3)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        assert!(b2.tracks[0].moves, "{:?}", b2.matches);
        let f3 = vec![prim(400.0, 3)];
        let b3 = track(Some((&f2, Some(&b2))), &f3);
        assert!(!b3.tracks[0].moves);
    }

    /// gfxmodels3: a portrait sliding through the wide side, its end faded
    /// by a strip whose split moves along the art (portrait.rs), slides:
    /// every picture in between reads the art at one place; a quad that
    /// reads another cell of its texture in place does not move.
    #[test]
    fn art_cut_elsewhere_slides() {
        // the art: 256 wide, mirrored (u 256 at its left end), at x0
        let v = |x: f32, y: f32, x0: f32, a: u8| -> Vec<u8> {
            let mut b = vec![0u8; 24];
            b[0..4].copy_from_slice(&x.to_le_bytes());
            b[4..8].copy_from_slice(&y.to_le_bytes());
            b[12..16].copy_from_slice(&(256.0 - (x - x0)).to_le_bytes());
            b[16..20].copy_from_slice(&y.to_le_bytes());
            b[20..24].copy_from_slice(&[128, 128, 128, a]);
            b
        };
        let quad = |l: f32, r: f32, x0: f32, al: u8, ar: u8| -> Vec<u8> { [v(l, 0.0, x0, al), v(r, 0.0, x0, ar), v(l, 256.0, x0, al), v(r, 256.0, x0, ar)].concat() };
        // rest, the left strip (fading, or none), the right strip (none)
        let portrait = |x0: f32, cut: f32| -> Draw {
            let mut d = prim(0.0, 12);
            let left = if cut > x0 { quad(x0, cut, x0, 0, 200) } else { quad(x0, x0, x0, 0, 200) };
            d.vertices = [quad(cut.max(x0), x0 + 256.0, x0, 200, 200), left, quad(x0 + 256.0, x0 + 256.0, x0, 0, 200)].concat();
            d.indices = (0..3).flat_map(|q| [0u32, 1, 2, 2, 1, 3].map(|i| i + 4 * q)).collect();
            d
        };
        let f0 = vec![portrait(-88.0, -88.0)];
        let b0 = track(None, &f0);
        // the end comes into the side: a strip appears, cut 24 in
        let f1 = vec![portrait(-72.0, -48.0)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks[0].moves, "{:?}", b1.matches);
        // settling: the cut stays at the 400's edge while the art moves
        let f2 = vec![portrait(-56.0, -32.0)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        let f3 = vec![portrait(-22.0, 0.0)];
        let b3 = track(Some((&f2, Some(&b2))), &f3);
        let f4 = vec![portrait(-14.0, 0.0)];
        let b4 = track(Some((&f3, Some(&b3))), &f4);
        for (f, b, x0) in [(&f1, &b1, -80.0f32), (&f4, &b4, -18.0)] {
            assert!(b.tracks[0].moves, "{:?}", b.matches);
            let at = draws_at(f, b, 0.5);
            for k in 0..12 {
                let (p, u) = (vertex_pos(&at[0].vertices, k), vertex_uv(&at[0].vertices, k));
                assert!((u[0] - (256.0 - (p[0] - x0))).abs() < 1e-3, "vertex {k}: {p:?} {u:?}");
            }
        }
        let _ = b2;
        // another cell, in place
        let mut g0 = portrait(0.0, 0.0);
        let mut g1 = portrait(0.0, 0.0);
        for k in 0..12 {
            g1.vertices[k * 24 + 12..k * 24 + 16].copy_from_slice(&(vertex_uv(&g0.vertices, k)[0] + 32.0).to_le_bytes());
        }
        g0.vertices[23] = 200;
        let (a, b) = (vec![g0], vec![g1]);
        let bb = track(Some((&a, None)), &b);
        let at = draws_at(&b, &bb, 0.5);
        assert_eq!(at[0].vertices[12..16], b[0].vertices[12..16]);
    }

    /// gfxleft: a shaded draw placed by c25's translation (x) and c26's
    /// (y), drawn with `blend`.
    fn model(x: f32, y: f32, blend: u32) -> Draw {
        let mut d = prim(0.0, 3);
        let mut shading = vec![0u32; SHADED_FLOATS + 64];
        let row = |shading: &mut Vec<u32>, u: usize, v: [f32; 4]| (0..4).for_each(|c| shading[u * 4 + c] = v[c].to_bits());
        row(&mut shading, 25, [1.0, 0.0, 0.0, x]);
        row(&mut shading, 26, [0.0, 1.0, 0.0, y]);
        row(&mut shading, 27, [0.0, 0.0, 1.0, 0.0]);
        let key = crate::scene::Key { blend: Some(blend), mask: 0xF, depth_test: true, depth_func: 0, depth_write: false, cull: 0, lighting: false, procedural: false, topology: 0, writes_depth: false };
        d.shaded = Some(Arc::new(Shaded {
            program: {
                // one program for every model (drawn the same way)
                static PROGRAM: std::sync::OnceLock<Arc<Vec<u32>>> = std::sync::OnceLock::new();
                PROGRAM.get_or_init(|| Arc::new(vec![])).clone()
            },
            shading,
            draw: vec![],
            tables: Arc::new(vec![]),
            vertices: Arc::new(vec![0; 64]),
            indices: Arc::new(vec![0, 1, 2, 2, 1, 3]),
            textures: [None, None, None],
            mips: Default::default(),
            key,
            viewport: [0, 0, 240, 400],
            scissor: [0, 0, 240, 400],
            depth_range: (0.0, 1.0),
            blend_color: [0.0; 4],
            placing: (0b111 << 25, 0),
            loose: false,
        }));
        d
    }

    /// mapjit: a tree clump of the map (one mesh drawn many times, each copy
    /// at its own place, rows 25-27), the camera's row (4) shared.
    fn clump(x: f32, camera: f32) -> Draw {
        let mut d = model(x, 0.0, 0x01010000);
        let s = Arc::get_mut(d.shaded.as_mut().unwrap()).unwrap();
        (0..4).for_each(|c| s.shading[4 * 4 + c] = [1.0, 0.0, 0.0, camera][c].to_bits());
        s.placing = (0b111 << 25 | 1 << 4, 0);
        d
    }

    /// mapjit: the map scrolls, a clump enters the view (listed first, as
    /// the game lists them by place) and one leaves: every clump still in
    /// view is drawn at its own place in the pictures in between (only the
    /// camera moves), and the new one does not slide from another's place.
    #[test]
    fn clumps_entering_keep_the_others_in_place() {
        let places = [100.0, 150.0, 200.0, 250.0, 300.0, 350.0];
        let f0: Vec<Draw> = places.iter().map(|&x| clump(x, 0.0)).collect();
        let b0 = track(None, &f0);
        let now = [50.0, 100.0, 150.0, 200.0, 250.0, 300.0];
        let f1: Vec<Draw> = now.iter().map(|&x| clump(x, 6.0)).collect();
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        let at = draws_at(&f1, &b1, 0.5);
        for (d, &x) in at.iter().zip(now.iter()) {
            let s = d.shaded.as_ref().unwrap();
            assert_eq!(f32::from_bits(s.shading[25 * 4 + 3]), x, "a clump slid from another's place");
            assert_eq!(f32::from_bits(s.shading[4 * 4 + 3]), 3.0, "the camera moves half way");
        }
        // (the clump that left counts as gone here: these test draws have no
        // program to place them, so it covers "the whole screen"; on the map
        // the leaving clumps are drawn moving out with the camera, castlecam)
    }

    /// gfxleft: a new copy of a moving draw (the crit cut-in panel's second
    /// layers, one frame) moves with it, and the frame keeps its pictures
    /// in between; a copy gone does not hold the frame.
    #[test]
    fn copies_move_with_their_draw() {
        let f0 = vec![model(0.0, 0.0, 0x76760000)];
        let b0 = track(None, &f0);
        let f1 = vec![model(10.0, 0.0, 0x76760000), model(10.0, 0.0, 0x76760000)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks[0].moves && b1.tracks[1].moves);
        assert_eq!(b1.vanished, 0);
        let at = draws_at(&f1, &b1, 0.5);
        for d in &at {
            assert_eq!(f32::from_bits(d.shaded.as_ref().unwrap().shading[25 * 4 + 3]), 5.0);
        }
        let f2 = vec![model(20.0, 0.0, 0x76760000)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        assert_eq!(b2.vanished, 0);
        assert!(b2.tracks[0].moves);
    }

    /// gfxleft: a model gone while the rest moves leaves the frame held
    /// when it covered part of the screen (its box is unknown here: a
    /// program that cannot be run counts as the whole screen).
    #[test]
    fn a_model_gone_holds_the_frame() {
        let f0 = vec![model(0.0, 0.0, 0x76760000), model(0.0, 0.0, 0x80760000)];
        let b0 = track(None, &f0);
        let f1 = vec![model(10.0, 0.0, 0x76760000)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert_eq!(b1.vanished, 1);
    }

    /// a jump (a camera cut) is drawn as it is and counts.
    #[test]
    fn jumps_stay() {
        let f0 = vec![prim(0.0, 3)];
        let b0 = track(None, &f0);
        let f1 = vec![prim(100.0, 3)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert_eq!(b1.matches.jumped, 1);
        assert!(!b1.tracks[0].moves);
        // going on as it went moves, however fast
        let f2 = vec![prim(200.0, 3)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        assert!(b2.tracks[0].moves, "{:?}", b2.matches);
        // gfxmodels3: or settling after it (a snap, then a little more)
        let f3 = vec![prim(102.0, 3)];
        let b3 = track(Some((&f1, Some(&b1))), &f3);
        assert!(b3.tracks[0].moves, "{:?}", b3.matches);
    }

    /// gfxmap: a window opening over art that stays where it is (a bigger
    /// cut each frame) opens in between too; a cell of a flip-book does not
    /// move.
    #[test]
    fn windows_open_over_fixed_art() {
        // the art at uv = position - 10, a quad from (c - h) to (c + h)
        let window = |h: f32| -> Draw {
            let v = |x: f32, y: f32| -> Vec<u8> {
                let mut b = vec![0u8; 24];
                b[0..4].copy_from_slice(&x.to_le_bytes());
                b[4..8].copy_from_slice(&y.to_le_bytes());
                b[12..16].copy_from_slice(&(x - 10.0).to_le_bytes());
                b[16..20].copy_from_slice(&(y - 10.0).to_le_bytes());
                b[20..24].copy_from_slice(&[255, 255, 255, 255]);
                b
            };
            let mut d = prim(0.0, 4);
            d.vertices = [v(200.0 - h, 120.0 - h), v(200.0 + h, 120.0 - h), v(200.0 - h, 120.0 + h), v(200.0 + h, 120.0 + h)].concat();
            d
        };
        let f0 = vec![window(10.0)];
        let b0 = track(None, &f0);
        let f1 = vec![window(30.0)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks[0].moves, "{:?}", b1.matches);
        let at = draws_at(&f1, &b1, 0.5);
        for k in 0..4 {
            let (p, u) = (vertex_pos(&at[0].vertices, k), vertex_uv(&at[0].vertices, k));
            assert!((p[0] - 200.0).abs() > 19.9 && (p[0] - 200.0).abs() < 20.1, "vertex {k}: {p:?}");
            assert!((u[0] - (p[0] - 10.0)).abs() < 1e-3, "vertex {k}: {p:?} {u:?}");
        }
    }

    /// gfxmap: an emitter's particles moving from slot to slot (a ring born,
    /// one gone): a particle matched with another particle is drawn as it is
    /// now, its own rows not moved part of the way.
    #[test]
    fn particles_matched_with_others_stay() {
        // rows 1 (colour, alpha in w) and 2 (size) are the particle's, row 5
        // the camera's
        let ring = |alpha: f32, size: f32, camera: f32| -> Draw {
            let mut d = model(0.0, 0.0, 0x8076_0000);
            let s = Arc::get_mut(d.shaded.as_mut().unwrap()).unwrap();
            let row = |shading: &mut Vec<u32>, u: usize, v: [f32; 4]| (0..4).for_each(|c| shading[u * 4 + c] = v[c].to_bits());
            row(&mut s.shading, 1, [1.0, 0.6, 0.2, alpha]);
            row(&mut s.shading, 2, [size, size, 0.0, 0.0]);
            row(&mut s.shading, 5, [camera, 0.0, 0.0, 0.0]);
            s.placing = (0b100110, 0b100110);
            s.loose = true;
            d
        };
        // (no number a whole sixty-fourth: those step, as flip-book cells)
        let f0 = vec![ring(0.997, 33.31, 100.1), ring(0.503, 48.27, 100.1)];
        let b0 = track(None, &f0);
        // the same rings a frame on, the camera moving: they move
        let f1 = vec![ring(0.987, 33.76, 101.1), ring(0.493, 48.72, 101.1)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks.iter().all(|t| t.moves), "{:?}", b1.matches);
        // a ring born in the first slot: the slots hold other rings now
        let f2 = vec![ring(0.003, 20.13, 102.1), ring(0.977, 34.21, 102.1)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        let at = draws_at(&f2, &b2, 0.5);
        for (n, d) in at.iter().enumerate() {
            let s = d.shaded.as_deref().unwrap();
            let now = f2[n].shaded.as_deref().unwrap();
            assert_eq!(s.shading[4..12], now.shading[4..12], "slot {n}: its particle as now");
            // the camera still moves part of the way
            assert!((f32::from_bits(s.shading[20]) - 101.6).abs() < 1e-3, "slot {n}: camera {}", f32::from_bits(s.shading[20]));
        }
    }
    /// vein2: one Dragon Vein in view: its core is a program drawn once, one
    /// draw of several particles (slots: rows 1-2 and 11-12) that move up a
    /// slot as a ring is born; the camera's row (85) is the same numbers as
    /// the frame's other particle program's (a ring). The core's particles
    /// are drawn as they are now in the pictures in between of the respawn,
    /// the camera still moves part of the way.
    #[test]
    fn a_lone_particle_program_keeps_its_particles() {
        let row = |shading: &mut Vec<u32>, u: usize, v: [f32; 4]| (0..4).for_each(|c| shading[u * 4 + c] = v[c].to_bits());
        let ring = |camera: f32| -> Draw {
            let mut d = model(0.0, 0.0, 0x8076_0000);
            let s = Arc::get_mut(d.shaded.as_mut().unwrap()).unwrap();
            row(&mut s.shading, 1, [1.0, 0.6, 0.2, 0.7]);
            row(&mut s.shading, 2, [20.3, 20.3, 0.0, 0.0]);
            row(&mut s.shading, 85, [camera, 663.4, 549.9, 0.0]);
            // (eft: the particles' rows are read by the texture coordinates too)
            s.placing = (1 << 1 | 1 << 2 | 1 << 85, 1 << 1 | 1 << 2 | 1 << 85);
            s.loose = true;
            d
        };
        let core = |a: (f32, f32), b: (f32, f32), camera: f32| -> Draw {
            let mut d = ring(camera);
            let s = Arc::get_mut(d.shaded.as_mut().unwrap()).unwrap();
            static PROGRAM: std::sync::OnceLock<Arc<Vec<u32>>> = std::sync::OnceLock::new();
            s.program = PROGRAM.get_or_init(|| Arc::new(vec![1])).clone();
            row(&mut s.shading, 1, [1.0, 0.95, 0.5, a.0]);
            row(&mut s.shading, 2, [a.1, a.1, 0.0, 0.0]);
            row(&mut s.shading, 11, [1.0, 0.95, 0.5, b.0]);
            row(&mut s.shading, 12, [b.1, b.1, 0.0, 0.0]);
            let rows = 1 << 1 | 1 << 2 | 1 << 11 | 1 << 12 | 1 << 85;
            s.placing = (rows, rows);
            d
        };
        let f0 = vec![ring(100.1), core((0.997, 33.31), (0.563, 47.41), 100.1)];
        let b0 = track(None, &f0);
        let f1 = vec![ring(101.1), core((0.987, 33.76), (0.553, 47.86), 101.1)];
        let b1 = track(Some((&f0, Some(&b0))), &f1);
        assert!(b1.tracks[1].moves, "{:?}", b1.matches);
        // a ring born in the first slot, the second takes the first's ring
        let f2 = vec![ring(102.1), core((0.003, 20.13), (0.977, 34.21), 102.1)];
        let b2 = track(Some((&f1, Some(&b1))), &f2);
        let at = draws_at(&f2, &b2, 0.5);
        let (s, now) = (at[1].shaded.as_deref().unwrap(), f2[1].shaded.as_deref().unwrap());
        for u in [1, 2, 11, 12] {
            assert_eq!(s.shading[u * 4..u * 4 + 4], now.shading[u * 4..u * 4 + 4], "row {u}: its particle as now");
        }
        assert!((f32::from_bits(s.shading[85 * 4]) - 101.6).abs() < 1e-3, "camera {}", f32::from_bits(s.shading[85 * 4]));
    }
}
