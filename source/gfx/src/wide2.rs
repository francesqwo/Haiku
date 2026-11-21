//! wide2: development trace of the upper screen's 2D states, for finding
//! what a full-screen effect is made of (fades, banners, the Dragon Vein
//! activation). FATES_WIDE2_TRACE=a,b,c (headless frame numbers) prints
//! every state of the upper display list of those frames: its callback or
//! texture, colour, blend, camera, and its first vertices in camera units
//! (world matrix applied), which is where `overrides/widescreen.c` decides
//! what covers the full 400.

use zakuro_core::System;

use crate::prim::{self, r16, r32, r8, Decoded};

fn wanted() -> &'static [u64] {
    static LIST: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        std::env::var("FATES_WIDE2_TRACE").unwrap_or_default().split(',').filter_map(|n| n.trim().parse().ok()).collect()
    })
}

pub fn trace(system: &mut System, prim_ctx: u32, disp: u32, states: &[u32], decoded: &[Decoded]) {
    let list = wanted();
    if list.is_empty() || disp != 0 {
        return;
    }
    let frame = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    static DONE: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    if !list.contains(&frame) || DONE.lock().unwrap().contains(&frame) {
        return;
    }
    DONE.lock().unwrap().push(frame);
    eprintln!("wide2-trace frame {frame}: {} states", states.len());
    for (i, (&st, d)) in states.iter().zip(decoded).enumerate() {
        let w0 = r32(system, st);
        let w1 = r32(system, st + 4);
        let w2 = r32(system, st + 8);
        let pri = r16(system, st + 0x14) as i16;
        let (_, cb) = prim::state_callback(system, prim_ctx, st);
        let vt = if cb != 0 { r32(system, cb) } else { 0 };
        let on_begin = if vt != 0 { r32(system, vt) } else { 0 };
        let cam_index = if w0 & 0x1E000 == 0 { (w0 & 0x1F) >> 3 } else { (w0 & 0x1FFFF) >> 13 };
        let cameras = r32(system, prim_ctx + 0x2458);
        let camera = r32(system, cameras + cam_index * 4);
        let cam = if camera != 0 {
            let kind = r8(system, camera + 0x41);
            let top = r8(system, camera + 0x5B0);
            let l = f32::from_bits(r32(system, camera + 0x58));
            let r = f32::from_bits(r32(system, camera + 0x5C));
            let vp: Vec<i16> = (0..4).map(|k| r16(system, camera + 0x3F8 + 2 * k) as i16).collect();
            format!("cam {camera:08X} t{kind} top{top} lr {l:.0},{r:.0} vp {vp:?}")
        } else {
            "cam -".into()
        };
        match d {
            Decoded::Draw(dr) => {
                let w = &dr.world;
                let verts: Vec<String> = dr
                    .vertices
                    .chunks_exact(24)
                    .take(8)
                    .map(|c| {
                        let g = |k: usize| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap());
                        let (x, y, z) = (g(0), g(1), g(2));
                        let wx = w[0] * x + w[1] * y + w[2] * z + w[3];
                        let wy = w[4] * x + w[5] * y + w[6] * z + w[7];
                        format!("({wx:.1},{wy:.1} uv {:.3},{:.3} #{:02X}{:02X}{:02X}{:02X})", g(3), g(4), c[20], c[21], c[22], c[23])
                    })
                    .collect();
                let tex = dr.textures[0].map(|(t, s)| format!("{:08X} {}x{} f{} wrap{}", t.addr, t.width, t.height, t.format, s.wrap)).unwrap_or("-".into());
                eprintln!(
                    "  {i:3} st {st:08X} {w0:08X} {w1:08X} {w2:08X} pri {pri} cb {on_begin:08X} {cam} tex {tex} col {:?} blend {:?}/{:?} ops {:?} comb {}/{} sc {:?} vp {:?} n {} v {}",
                    dr.color.map(|c| (c * 255.0).round() as u8),
                    dr.blend.eq,
                    dr.blend.func,
                    dr.combiner.stages[0].op_rgb,
                    dr.combiner.stages[0].comb_rgb,
                    dr.combiner.stages[0].comb_a,
                    dr.scissor,
                    dr.viewport,
                    dr.indices.len(),
                    verts.join(" ")
                );
            }
            Decoded::Island(k) => eprintln!("  {i:3} st {st:08X} {w0:08X} {w1:08X} {w2:08X} pri {pri} island {k:?} cb {cb:08X} {cam}"),
            Decoded::Skip => eprintln!("  {i:3} st {st:08X} {w0:08X} {w1:08X} {w2:08X} pri {pri} skip"),
        }
    }
}

// ---- full-width layers of the 3D telops, extended into the wide sides ----
//
// The telops (Tlp_*.arc: phase banners, the Dragon Vein activations,
// chapter title cards, cut-ins) are H3D models drawn by a frustum camera
// that GameEffect::SetupTelopCamera (0x0017B2DC) sets up for the 400x240
// screen. Their backgrounds (a black or tinted wash, a vignette, the
// Dragon Vein's wallpaper) are planes exactly the size of that view. With
// the camera widened (overrides/widescreen.c) everything keeps its size,
// centred, and those planes end at the 5:3 edges.
//
// A plane whose drawn extent covers the original view (its two edges, and
// the full height), but not the wide sides, is a full-width layer: it is
// drawn again for each side it leaves uncovered, with shade.vert told (Shading.pad, see
// render/shaded.rs) to mirror the plane's last sliver at that edge across
// the side (from the plane's own end, which may lie a little past the
// view's edge). The side then shows the plane's own edge column, through the
// plane's own material, blending and animation: a wash or vignette carries
// on in the colour it has at the edge, and artwork in the middle keeps its
// size. Planes that stop short of an edge (ornaments, the dragon, the
// title) are left alone, so nothing is stretched.

use std::sync::Arc;
use zakuro_gpu::shader::{self, ShaderUnit, Vec4, INPUT_REGISTERS};

use crate::scene::Shaded;

/// Shading word offsets (shade.vert's Shading block).
const INTEGERS: usize = 384;
const BOOLS: usize = 400;
const ENTRY: usize = 401;
/// the two pad words carry the side extension: 1, and the edge
pub(crate) const PAD: usize = 402; // wide-ui: pub(crate)
const SEMANTICS: usize = 404;
const ATTRIBUTES: usize = 436;
const DEFAULTS: usize = 500;

/// the most vertices a draw may have to be looked at: the telops' planes
/// have a handful; meshes with many are never full-view planes.
const MOST_VERTICES: usize = 16;

/// FATES_WIDE2_EXTEND=0 turns the side extension off (for comparisons).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_WIDE2_EXTEND").map_or(true, |v| v != "0"))
}

fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_WIDE2_LOG").is_some())
}

/// the CPU shader unit for a draw: its program decoded once per program
/// content, its uniforms the draw's.
/// the shader unit for a draw's program, with its uniforms.
///
/// perf3: run for every shaded draw of few vertices each frame. It hashed
/// the whole 4224-word program and copied a fresh unit (17 KB) for every
/// call; now the last unit made on this thread is kept with its program's
/// `Arc` (held, so the address cannot be reused by another program) and only
/// its uniforms are rewritten when the next draw has the same program, which
/// draws of one model do. Other programs go through the hashed cache as
/// before. The unit run is the same either way.
fn with_unit<R>(s: &Shaded, f: impl FnOnce(&ShaderUnit) -> R) -> R {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    static UNITS: std::sync::Mutex<Option<HashMap<u64, ShaderUnit>>> = std::sync::Mutex::new(None);
    thread_local! {
        static LAST: std::cell::RefCell<Option<(Arc<Vec<u32>>, ShaderUnit)>> = const { std::cell::RefCell::new(None) };
    }
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        if !last.as_ref().is_some_and(|(p, _)| Arc::ptr_eq(p, &s.program)) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.program.hash(&mut h);
            let key = h.finish();
            let mut units = UNITS.lock().unwrap();
            let units = units.get_or_insert_with(HashMap::new);
            if units.len() > 256 {
                units.clear();
            }
            let base = units.entry(key).or_insert_with(|| {
                let mut u = ShaderUnit::new();
                let n = shader::PROGRAM_SIZE.min(s.program.len());
                u.program[..n].copy_from_slice(&s.program[..n]);
                let d = (s.program.len() - n).min(shader::DESCRIPTOR_SIZE);
                u.descriptors[..d].copy_from_slice(&s.program[n..n + d]);
                u.prepare();
                u
            });
            *last = Some((s.program.clone(), base.clone()));
        }
        let u = &mut last.as_mut().unwrap().1;
        for (i, f) in u.float_uniforms.iter_mut().enumerate() {
            *f = std::array::from_fn(|c| f32::from_bits(s.shading[i * 4 + c]));
        }
        for i in 0..4 {
            let w = &s.shading[INTEGERS + i * 4..INTEGERS + i * 4 + 3];
            u.int_uniforms[i] = [w[0] as u8, w[1] as u8, w[2] as i32 as i8 as u8, 0];
        }
        u.bool_uniforms = s.shading[BOOLS] as u16;
        u.entry_point = s.shading[ENTRY];
        f(u)
    })
}

/// one input register of vertex `index`, as shade.vert's input_register
/// reads it.
fn input(s: &Shaded, r: usize, index: u32) -> Vec4 {
    let a = &s.shading[ATTRIBUTES + r * 4..ATTRIBUTES + r * 4 + 4];
    let mut v: Vec4 = std::array::from_fn(|c| f32::from_bits(s.shading[DEFAULTS + r * 4 + c]));
    let count = (a[2] >> 16) as usize;
    if count == 0 {
        return v;
    }
    let ty = (a[2] >> 8) & 0xFF;
    let size = match ty {
        3 => 4,
        2 => 2,
        _ => 1,
    };
    let at = a[0] as usize + index as usize * a[1] as usize + (a[2] & 0xFF) as usize;
    let bytes = &s.vertices;
    for (c, out) in v.iter_mut().enumerate().take(count.min(4)) {
        let o = at + c * size;
        let Some(b) = bytes.get(o..o + size) else { return v };
        *out = match ty {
            0 => b[0] as i8 as f32,
            1 => b[0] as f32,
            2 => i16::from_le_bytes([b[0], b[1]]) as f32,
            _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        };
    }
    v
}

/// the draw's vertices on the screen, (long axis, short axis) in
/// normalised device units, and their w (their distance from the eye); None when there are too many to look at or
/// one is behind the eye.
pub(crate) fn projected(s: &Shaded) -> Option<Vec<(f32, f32, f32)>> { // wide-ui: pub(crate)
    projected_most(s, MOST_VERTICES)
}

/// gfxleft: projected, with up to `most` vertices (more: an even sample of them).
pub(crate) fn projected_most(s: &Shaded, most: usize) -> Option<Vec<(f32, f32, f32)>> {
    clip_most(s, most)?.into_iter().map(|(y, x, w)| (w > 1e-5).then(|| (y / w, x / w, w))).collect()
}

/// transblur: projected_most's vertices before the divide: (long axis,
/// short axis, w) in clip space, those behind the eye too (None as there:
/// too many, or none)
pub(crate) fn clip_most(s: &Shaded, most: usize) -> Option<Vec<(f32, f32, f32)>> {
    // perf3: for the extension's test (at most 16 vertices) the distinct
    // indices are gathered with an early stop at the 17th: it runs for every
    // shaded draw of every frame, and copying and sorting a terrain's or a
    // field's tens of thousands of indices to learn it has more than 16
    // was a tenth of the game's thread with the free camera out
    let mut used: Vec<u32> = if most == MOST_VERTICES {
        let mut few: Vec<u32> = Vec::with_capacity(MOST_VERTICES + 1);
        for &i in s.indices.iter() {
            if !few.contains(&i) {
                if few.len() == MOST_VERTICES {
                    return None;
                }
                few.push(i);
            }
        }
        few.sort_unstable();
        few
    } else {
        let mut all = s.indices.to_vec();
        all.sort_unstable();
        all.dedup();
        all
    };
    if used.is_empty() {
        return None;
    }
    if used.len() > most {
        if most == MOST_VERTICES {
            return None;
        }
        let step = used.len() as f32 / most as f32;
        used = (0..most).map(|k| used[(k as f32 * step) as usize]).collect();
    }
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = used.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let outputs = with_unit(s, |unit| shader::run_vertices(unit, &inputs)); // perf3
    let sem = |o: &[Vec4; 16], which: usize| -> f32 {
        let slot = s.shading[SEMANTICS + which];
        if slot == u32::MAX || slot >= 64 {
            0.0
        } else {
            o[(slot >> 2) as usize][(slot & 3) as usize]
        }
    };
    outputs
        .iter()
        .map(|o| {
            let (x, y, w) = (sem(o, 0), sem(o, 1), sem(o, 3));
            (y, x, w)
        })
        .collect::<Vec<_>>()
        .into()
}

/// castlecam: up to `most` of a draw's vertices (an even sample) run
/// through its program: their clip x (long axis), y and w, behind the eye
/// too; None without vertices.
pub(crate) fn clip_points(s: &Shaded, most: usize) -> Option<Vec<[f32; 4]>> {
    let mut used: Vec<u32> = s.indices.to_vec();
    used.sort_unstable();
    used.dedup();
    if used.is_empty() {
        return None;
    }
    if used.len() > most {
        let step = used.len() as f32 / most as f32;
        used = (0..most).map(|k| used[(k as f32 * step) as usize]).collect();
    }
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = used.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let outputs = with_unit(s, |unit| shader::run_vertices(unit, &inputs));
    let sem = |o: &[Vec4; 16], which: usize| -> f32 {
        let slot = s.shading[SEMANTICS + which];
        if slot == u32::MAX || slot >= 64 { 0.0 } else { o[(slot >> 2) as usize][(slot & 3) as usize] }
    };
    Some(outputs.iter().map(|o| [sem(o, 1), sem(o, 0), sem(o, 2), sem(o, 3)]).collect())
}

/// skybg: dev, every vertex of a draw: its first four input registers and
/// its outputs (position x, y, z, w; texture 0 u, v, w; 1 u, v; 2 u, v), as the GPU runs them.
pub(crate) fn dev_vertices(s: &Shaded) -> Option<Vec<([Vec4; 4], [f32; 11])>> {
    let mut used: Vec<u32> = s.indices.to_vec();
    used.sort_unstable();
    used.dedup();
    if used.is_empty() {
        return None;
    }
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = used.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let outputs = with_unit(s, |unit| shader::run_vertices(unit, &inputs));
    let sem = |o: &[Vec4; 16], which: usize| -> f32 {
        let slot = s.shading[SEMANTICS + which];
        if slot == u32::MAX || slot >= 64 { f32::NAN } else { o[(slot >> 2) as usize][(slot & 3) as usize] }
    };
    Some(inputs.iter().zip(outputs.iter()).map(|(i, o)| ([i[0], i[1], i[2], i[3]], [sem(o, 0), sem(o, 1), sem(o, 2), sem(o, 3), sem(o, 8), sem(o, 9), sem(o, 21), sem(o, 10), sem(o, 11), sem(o, 12), sem(o, 13)])).collect())
}

/// mapjit: dev, the given vertices run through the draw's own program, as
/// `projected` places them: (long axis, short axis, w); behind the eye left
/// out.
pub(crate) fn project_points(s: &Shaded, indices: &[u32]) -> Vec<(f32, f32, f32)> {
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = indices.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let outputs = with_unit(s, |unit| shader::run_vertices(unit, &inputs));
    let sem = |o: &[Vec4; 16], which: usize| -> f32 {
        let slot = s.shading[SEMANTICS + which];
        if slot == u32::MAX || slot >= 64 { 0.0 } else { o[(slot >> 2) as usize][(slot & 3) as usize] }
    };
    outputs.iter().filter_map(|o| {
        let (x, y, w) = (sem(o, 0), sem(o, 1), sem(o, 3));
        (w > 1e-5).then(|| (y / w, x / w, w))
    }).collect()
}

/// perf3: the largest w (distance along the view) of a draw's vertices
/// when it uses at most `most` of them, run through its own program as
/// `projected` runs them; None with more, or when one is behind the eye.
pub(crate) fn farthest_w(s: &Shaded, most: usize) -> Option<f32> {
    let mut few: Vec<u32> = Vec::with_capacity(most.min(64) + 1);
    for &i in s.indices.iter() {
        if !few.contains(&i) {
            if few.len() == most {
                return None;
            }
            few.push(i);
        }
    }
    if few.is_empty() {
        return None;
    }
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = few.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let outputs = with_unit(s, |unit| shader::run_vertices(unit, &inputs));
    let slot = s.shading[SEMANTICS + 3];
    if slot == u32::MAX || slot >= 64 {
        return None;
    }
    let mut far = 0.0f32;
    for o in &outputs {
        let w = o[(slot >> 2) as usize][(slot & 3) as usize];
        if !(w > 1e-5) {
            return None;
        }
        far = far.max(w);
    }
    Some(far)
}

/// how near, in normalised device units of the wide screen, a plane's
/// ends must be to the original view's edges: about one unit of the 400.
const EDGE_TOLERANCE: f32 = 0.008;

/// how much larger than the view a background may be.
const OVERSIZE: f32 = 1.25;

/// wide3: how much wider than the view a camera-facing background may be,
/// wherever it sits along the long axis.
const WIDER: f32 = 1.5;

/// wide3: how far past the screen's top and bottom a camera-facing
/// background may reach (normalised device units): the story talks' event
/// pictures pan down a picture almost twice the screen's height.
const TALLER: f32 = 2.5;

/// how near a background's top and bottom must be to the screen's.
const HEIGHT_TOLERANCE: f32 = 0.02;

/// adds each full-width layer's two side extensions right after it. `k`
/// is the widening of the upper screen.
pub fn extend(made: &mut Vec<(crate::prim::Draw, [Option<u64>; 3])>, k: f32) {
    if k <= 1.001 || !enabled() || made.is_empty() {
        return;
    }
    let edge = 1.0 / k;
    let mut out = Vec::with_capacity(made.len() + 2);
    for (mut draw, keys) in made.drain(..) {
        // gfxmodels3: an invisible mask reaching past the 400 reaches the
        // wide screen's edges (the crit cut-in's closing jaws)
        if let Some(longer) = draw.shaded.as_deref().and_then(|s| lengthen_mask(s, edge)) {
            draw.shaded = Some(Arc::new(longer));
        }
        let extra = draw.shaded.as_deref().and_then(|s| {
            let points = projected(s)?;
            let lo = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
            let hi = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
            let top = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
            let bottom = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
            // across the whole view, both ways: a background, not a band
            // or a piece of artwork
            let full = lo <= -edge + EDGE_TOLERANCE && hi >= edge - EDGE_TOLERANCE && bottom <= -1.0 + HEIGHT_TOLERANCE && top >= 1.0 - HEIGHT_TOLERANCE;
            // and made for that view: no more than a quarter larger, and
            // facing the eye (one distance), as the telops' planes are; a
            // scene's ground or walls that end inside the screen are not
            let near = points.iter().map(|p| p.2).fold(f32::MAX, f32::min);
            let far = points.iter().map(|p| p.2).fold(f32::MIN, f32::max);
            // wide3: along the long axis the plane may sit off centre (the
            // support talks' room is 532 units wide, panned so it ends 4
            // units past one edge of the 400 and 128 past the other): it
            // is bounded by its width, and the side it leaves open is
            // extended (one already past the screen's edge needs nothing)
            // and up and down it may reach well past the screen (a talk's
            // picture panned down the screen, the crit cut-in's shade)
            let full = full && (hi - lo) <= 2.0 * edge * WIDER && bottom >= -TALLER && top <= TALLER && far - near <= 0.01 * far.abs();
            if log_on() {
                eprintln!(
                    "wide2-extend frame {} {} verts long {lo:.4}..{hi:.4} (edge {edge:.4}) short {bottom:.3}..{top:.3} tex {:?} full {full} key {:?} depth {near:.4}..{far:.4}", // gfxmodels3: the key and depths
                    zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
                    points.len(),
                    s.textures.map(|t| t.map(|(t, _)| (t.addr, t.width, t.height, t.format))),
                    s.key,
                );
            }
            // each side it does not already cover, from its own end
            let mut sides = Vec::new();
            if full && hi < 1.0 {
                sides.push(side(s, hi, k, (hi - edge).abs() < EDGE_TOLERANCE));
            }
            if full && lo > -1.0 {
                sides.push(side(s, lo, k, (lo + edge).abs() < EDGE_TOLERANCE));
            }
            (!sides.is_empty()).then_some(sides)
        });
        if let Some(sides) = extra {
            // the plane itself stops where its sides start, so a
            // translucent one is not laid twice over the strip between
            let mut main = draw.clone();
            if let Some(m) = main.shaded.as_deref() {
                let mut m = m.clone_shaded();
                for side in &sides {
                    m.scissor = cut(m.scissor, side.scissor, side_is_high(side));
                }
                main.shaded = Some(Arc::new(m));
            }
            out.push((main, keys));
            for s in sides {
                let mut d = draw.clone();
                d.shaded = Some(Arc::new(s));
                out.push((d, keys));
            }
        } else {
            out.push((draw, keys));
        }
    }
    *made = out;
}

// ---- invisible masks lengthened to the wide edges (gfxmodels3) ----
//
// The crit cut-in (Tlp_CutIn) closes over the face panel with two "jaws":
// untextured quads drawn with blending ZERO, ONE (nothing of them shows)
// that write a nearer depth, so the panel and its lightning (depth test
// less) do not draw where they lie. They are rectangles turned with the
// jaws, made long enough for the 400: at 21:9 their ends, and with the
// turn their corners, stop inside the wide sides, and the panel showed
// past them, a white wedge and then a white strip at the left edge that
// the console never shows (the panel itself slides off past the 400).
//
// Such a mask (invisible, writing depth, facing the camera, four corners)
// whose end already lies past the 400's edge is lengthened along its own
// sides until that end is past the wide screen's edge too: its edges stay
// on their lines, so inside the 400 nothing changes, and since nothing of
// it is drawn, nothing is stretched. The end the console never showed was
// arbitrary. FATES_WIDE2_MASKS=0 turns it off.

/// how far past the wide screen's edge a lengthened mask's end goes
/// (normalised device units): pictures in between turn the mask a little
/// from where its frame has it.
const MASK_PAST: f32 = 1.2;

fn masks_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_WIDE2_MASKS").map_or(true, |v| v != "0"))
}

/// whether nothing of a draw shows: no colour writes, or blending ZERO, ONE.
fn invisible(key: &crate::scene::Key) -> bool {
    key.mask == 0 || key.blend.is_some_and(|b| (b >> 16) & 0xF == 0 && (b >> 20) & 0xF == 1 && (b >> 24) & 0xF == 0 && (b >> 28) & 0xF == 1)
}

/// the mask lengthened, see above; None when the draw is not such a mask
/// or needs nothing.
fn lengthen_mask(s: &Shaded, edge: f32) -> Option<Shaded> {
    if !masks_on() || !invisible(&s.key) || !s.key.depth_write || s.textures.iter().any(|t| t.is_some()) {
        return None;
    }
    let mut used: Vec<u32> = s.indices.to_vec(); // perf3: Arc
    used.sort_unstable();
    used.dedup();
    if used.len() != 4 {
        return None;
    }
    let inputs: Vec<[Vec4; INPUT_REGISTERS]> = used.iter().map(|&i| std::array::from_fn(|r| input(s, r, i))).collect();
    let clip = |inputs: &[[Vec4; INPUT_REGISTERS]]| -> Option<Vec<(f32, f32, f32)>> {
        let outputs = with_unit(s, |unit| shader::run_vertices(unit, inputs)); // perf3
        let sem = |o: &[Vec4; 16], which: usize| -> f32 {
            let slot = s.shading[SEMANTICS + which];
            if slot == u32::MAX || slot >= 64 { 0.0 } else { o[(slot >> 2) as usize][(slot & 3) as usize] }
        };
        outputs.iter().map(|o| {
            let (x, y, w) = (sem(o, 0), sem(o, 1), sem(o, 3));
            (w > 1e-5).then(|| (y / w, x / w, w))
        }).collect()
    };
    let points = clip(&inputs)?;
    // facing the camera: one distance
    let near = points.iter().map(|p| p.2).fold(f32::MAX, f32::min);
    let far = points.iter().map(|p| p.2).fold(f32::MIN, f32::max);
    if far - near > 0.01 * far.abs() {
        return None;
    }
    let lo = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let hi = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    if !(lo < -edge || hi > edge) {
        return None;
    }
    // the input register that places it: a float one whose x moves the
    // clip position
    let position = (0..INPUT_REGISTERS).find(|&r| {
        let a = &s.shading[ATTRIBUTES + r * 4..ATTRIBUTES + r * 4 + 4];
        if (a[2] >> 16) < 2 || (a[2] >> 8) & 0xFF != 3 {
            return false;
        }
        let mut moved = inputs.clone();
        for v in moved.iter_mut() {
            v[r][0] += 1.0;
        }
        clip(&moved).is_some_and(|m| m.iter().zip(&points).any(|(a, b)| (a.0 - b.0).abs() + (a.1 - b.1).abs() > 1e-4))
    })?;
    // the rectangle: vertex 0, its corner across (furthest), and its two
    // neighbours; the sides "along" are the pair nearer the long axis
    let d2 = |a: usize, b: usize| (points[a].0 - points[b].0).powi(2) + (points[a].1 - points[b].1).powi(2);
    let across = (1..4).max_by(|&a, &b| d2(0, a).total_cmp(&d2(0, b)))?;
    let others: Vec<usize> = (1..4).filter(|&i| i != across).collect();
    let slant = |a: usize, b: usize| (points[b].0 - points[a].0).abs() / d2(a, b).sqrt().max(1e-6);
    let (n0, n1) = if slant(0, others[0]) >= slant(0, others[1]) { (others[0], others[1]) } else { (others[1], others[0]) };
    // the two sides along: 0 - n0 and n1 - across
    let sides = [(0usize, n0), (n1, across)];
    if slant(0, n0) < 0.5 {
        return None;
    }
    // ends: the low end (smaller long) and the high end of each side
    let ends: Vec<(usize, usize)> = sides.iter().map(|&(a, b)| if points[a].0 <= points[b].0 { (a, b) } else { (b, a) }).collect();
    let mut factor = [0.0f32; 4];
    let mut changed = false;
    // the low ends past the 400 go past the screen's low edge, and the
    // high ones past its high edge
    let low_past = ends.iter().all(|&(l, _)| points[l].0 < -edge);
    let high_past = ends.iter().all(|&(_, h)| points[h].0 > edge);
    if low_past {
        let t = ends.iter().map(|&(l, h)| ((points[l].0 + MASK_PAST) / (points[h].0 - points[l].0)).max(0.0)).fold(0.0f32, f32::max);
        for &(l, _) in &ends {
            factor[l] = t;
        }
        changed |= t > 0.0;
    }
    if high_past {
        let t = ends.iter().map(|&(l, h)| ((MASK_PAST - points[h].0) / (points[h].0 - points[l].0)).max(0.0)).fold(0.0f32, f32::max);
        for &(_, h) in &ends {
            factor[h] = t;
        }
        changed |= t > 0.0;
    }
    if !changed {
        return None;
    }
    // each end moved along its side by its factor of the side, in the
    // mesh's own units (the camera's projection keeps lines and ratios on a
    // plane facing it)
    let mut vertices = s.vertices.as_ref().clone();
    let a = &s.shading[ATTRIBUTES + position * 4..ATTRIBUTES + position * 4 + 4];
    let count = ((a[2] >> 16) as usize).min(4);
    let partner = |v: usize| ends.iter().find_map(|&(l, h)| if l == v { Some(h) } else if h == v { Some(l) } else { None });
    for v in 0..4 {
        if factor[v] == 0.0 {
            continue;
        }
        let other = partner(v)?;
        let at = a[0] as usize + used[v] as usize * a[1] as usize + (a[2] & 0xFF) as usize;
        for c in 0..count.min(3) {
            let moved = inputs[v][position][c] + factor[v] * (inputs[v][position][c] - inputs[other][position][c]);
            vertices.get_mut(at + c * 4..at + c * 4 + 4)?.copy_from_slice(&moved.to_le_bytes());
        }
    }
    if log_on() {
        eprintln!(
            "wide2-mask frame {} long {lo:.3}..{hi:.3} (edge {edge:.3}) lengthened {:?}",
            zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
            factor
        );
    }
    let mut out = s.clone_shaded();
    out.vertices = Arc::new(vertices);
    Some(out)
}

/// A side is drawn in one of two ways (Shading.pad[0] says which, pad[1]
/// holds the plane's end e there, signed, in normalised device units of
/// the long axis):
///
/// - A plane ending on the view's edge was made for the 3DS screen: its
///   last few texels are a border the console never showed (the Dragon
///   Vein wallpaper's are black, inside a baked vignette). Its outer strip,
///   the last 12 units of the 400 (from c = 0.94 e to e), is drawn again
///   widened across [c, screen edge], over the plane's own strip there. The
///   vignette carries on, darkening outward to the plane's border colour
///   at the screen's edge, and the artwork inside c keeps its size.
///   pad[0] is the widening, negative.
/// - A plane larger than the view (the phase banners' shade) shows its
///   own end in the side already: its column just inside its end
///   (c = 0.985 e, past the rim its last texels may have) is spread
///   across the rest of the side from c on, mirrored about c. pad[0] is how many times it is spread, positive:
///   about one unit of the 400 across the whole side (far larger spreads
///   lose the GPU's precision).
///
/// Either way the rest of the plane lands inside c, which the scissor cuts,
/// the plane itself is cut at c, so nothing is laid twice, and the side
/// keeps the plane's own material, blending and animation.
fn widening(edge: f32, k: f32, over: bool) -> f32 {
    static S: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    if let Some(s) = *S.get_or_init(|| std::env::var("FATES_WIDE2_SPREAD").ok().and_then(|v| v.parse().ok())) {
        return if over { -s } else { s };
    }
    let e = edge.abs();
    if over {
        let c = e * OUTER_STRIP;
        // a little past the screen's edge, so no column is left uncovered
        -((1.0 - c) / (e - c) * 1.01)
    } else {
        let unit = 1.0 / (200.0 * k);
        ((1.0 - e) / unit).clamp(8.0, 512.0)
    }
}

/// where a plane ending on the view's edge starts its outer strip, as a
/// share of its end: 12 units of the 400 in.
const OUTER_STRIP: f32 = 0.94;
/// the column a larger plane's side repeats, as a share of its end: 3
/// units of the 400 in, past the texels at the very end (the phase
/// banners' shade has a lighter rim there).
const SPREAD_COLUMN: f32 = 0.985;

/// whether a side is the one at the high end of the long axis (its
/// scissor starts past the middle row).
fn side_is_high(side: &Shaded) -> bool {
    f32::from_bits(side.shading[PAD + 1]) > 0.0
}

/// a plane's scissor cut where its side's starts.
fn cut(plane: [i32; 4], side: [i32; 4], high: bool) -> [i32; 4] {
    let [x, y, w, h] = plane;
    let (mut y0, mut y1) = (y, y + h);
    if high {
        y1 = y1.min(side[1]);
    } else {
        y0 = y0.max(side[1] + side[3]);
    }
    [x, y0, w, (y1 - y0).max(0)]
}

impl Shaded {
    /// a copy, for a draw drawn again with changes.
    fn clone_shaded(&self) -> Shaded {
        Shaded {
            program: self.program.clone(),
            shading: self.shading.clone(),
            draw: self.draw.clone(),
            tables: self.tables.clone(),
            vertices: self.vertices.clone(),
            indices: self.indices.clone(),
            textures: self.textures,
            mips: self.mips,
            key: self.key,
            viewport: self.viewport,
            scissor: self.scissor,
            depth_range: self.depth_range,
            blend_color: self.blend_color,
            placing: self.placing,
            loose: self.loose,
        }
    }
}

/// the draw again, across the side beyond `edge`, scissored to it; `over`
/// for a plane ending on the view's edge (see `widening`).
fn side(s: &Shaded, edge: f32, k: f32, over: bool) -> Shaded {
    let mut shading = s.shading.clone();
    shading[PAD] = widening(edge, k, over).to_bits();
    shading[PAD + 1] = edge.to_bits();
    let mut key = s.key;
    // the mirror turns the triangles over
    key.cull = 0;
    // the side's framebuffer rows (the long axis; row 0 is device -1),
    // from the row nearest its start, where the plane's own stop
    let [_, vy, _, vh] = s.viewport;
    let from = edge * if over { OUTER_STRIP } else { SPREAD_COLUMN };
    let row = vy as f32 + (from + 1.0) * 0.5 * vh as f32;
    let [sx, sy, sw, sh] = s.scissor;
    let (mut y0, mut y1) = (sy, sy + sh);
    if edge > 0.0 {
        y0 = y0.max(row.round() as i32);
    } else {
        y1 = y1.min(row.round() as i32);
    }
    let scissor = [sx, y0, sw, (y1 - y0).max(0)];
    let mut out = s.clone_shaded();
    out.shading = shading;
    out.key = key;
    out.scissor = scissor;
    out
}

/// shade.vert's last position line with the side extension before it
/// (render/shaded.rs patches it in before the upright turn), the two ways
/// `widening` describes.
pub const SHADE_VERT_SIDE: &str = "if (pad[0] != 0u) {
        float e = uintBitsToFloat(pad[1]) * position.w;
        float widen = uintBitsToFloat(pad[0]);
        if (widen < 0.0) {
            float c = e * 0.94;
            position.y = c + (position.y - c) * -widen;
        } else {
            float c = e * 0.985;
            position.y = c - (position.y - c) * widen;
        }
    }
    gl_Position = vec4(position.xy, -position.z, position.w);";

/// whether a draw is a side extension `extend` made.
pub(crate) fn is_side(d: &crate::prim::Draw) -> bool { // wide-ui: pub(crate)
    d.shaded.as_deref().is_some_and(|s| s.shading.get(PAD).is_some_and(|&p| p != 0))
}

/// the side extensions' textures, softened: the column a side repeats
/// is blurred down the screen, so the side carries the plane's tones
/// (its vignette, its tint) without streaks from the detail that column
/// happens to cross. Each softened texture is made once, beside the
/// original in the renderer's decoded textures, under its own key.
pub fn soften(decoded: &mut std::collections::HashMap<(crate::prim::TexRef, u64), Arc<crate::render::Decoded>>, made: &mut [(crate::prim::Draw, [Option<u64>; 3])]) {
    for (d, keys) in made.iter_mut() {
        if !is_side(d) {
            continue;
        }
        let Some(s) = d.shaded.as_deref() else { continue };
        for unit in 0..3 {
            let (Some((tex, _)), Some(key)) = (s.textures[unit], keys[unit]) else { continue };
            let soft = key ^ 0x5769_6465_3253_6F66; // "Wide2Sof"
            if !decoded.contains_key(&(tex, soft)) {
                let Some(base) = decoded.get(&(tex, key)).cloned() else { continue };
                // hdpacks: a pack's picture blurred at the game's size
                let base = crate::hd::at_size(&base, tex.width, tex.height).map_or(base, Arc::new);
                decoded.insert((tex, soft), Arc::new(blurred(&base)));
            }
            keys[unit] = Some(soft);
        }
    }
}

/// a texture's largest size blurred along t (the screen's height) a lot
/// and along s a little; smaller sizes as they were.
fn blurred(d: &crate::render::Decoded) -> crate::render::Decoded {
    let (w, h) = (d.width as usize, d.height as usize);
    let mut rgba = d.rgba.clone();
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return crate::render::Decoded { width: d.width, height: d.height, rgba, levels: d.levels };
    }
    // radii in texels: a 256-tall texture spans about the screen's height
    let rv = (h / 24).max(1);
    let rh = (w / 256).max(1);
    // moflexhd: running sums, each texel's the same sum over the same
    // window as adding the window up for it (a movie's picture, softened
    // anew 30 or 60 times a second, took 8 ms at 512x256 and 200 ms at
    // 1024x1024 that way)
    let pass = |src: &[u8], dst: &mut [u8], along_t: bool, r: usize| box_pass(src, dst, w, h, along_t, r);
    let mut tmp = rgba[..w * h * 4].to_vec();
    // two box passes down, one across: close to a gaussian
    pass(&rgba[..w * h * 4].to_vec(), &mut tmp, true, rv);
    let mut tmp2 = tmp.clone();
    pass(&tmp, &mut tmp2, true, rv);
    pass(&tmp2, &mut rgba[..w * h * 4], false, rh);
    crate::render::Decoded { width: d.width, height: d.height, rgba, levels: d.levels }
}

// ---- the 2D layer's full-width layers ----
//
// A textured 2D quad state on the upper screen's orthographic camera whose
// quads together reach both edges of the original 400 (the menus' header
// and footer bars, full-screen wallpapers) is extended the same way as the
// telops' planes: each quad touching an edge is cut 12 units in, and a new
// quad carries that strip, stretched, from there to the screen's edge. The
// new quads are a draw of their own right after the plane's, with the
// plane's state, blending and colour, sampling a softened copy of its
// texture: blurred across for a band (a bar's trims and grain carry on as
// lines), down the screen for a full-height layer (its tones carry on
// without streaks).

/// a 2D vertex: position, texture coordinates, colour.
#[derive(Clone, Copy)]
pub(crate) struct V { // wide-ui: pub(crate)
    pub(crate) p: [f32; 3],
    pub(crate) uv: [f32; 2],
    pub(crate) c: [u8; 4],
}

pub(crate) fn read_v(b: &[u8]) -> V { // wide-ui: pub(crate)
    let f = |k: usize| f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap());
    V { p: [f(0), f(1), f(2)], uv: [f(3), f(4)], c: [b[20], b[21], b[22], b[23]] }
}

pub(crate) fn write_v(v: &V, out: &mut Vec<u8>) { // wide-ui: pub(crate)
    for x in v.p.iter().chain(v.uv.iter()) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&v.c);
}

pub(crate) fn lerp_v(a: &V, b: &V, t: f32) -> V { // wide-ui: pub(crate)
    let l = |x: f32, y: f32| x + (y - x) * t;
    V {
        p: std::array::from_fn(|i| l(a.p[i], b.p[i])),
        uv: std::array::from_fn(|i| l(a.uv[i], b.uv[i])),
        c: std::array::from_fn(|i| l(a.c[i] as f32, b.c[i] as f32).round().clamp(0.0, 255.0) as u8),
    }
}

/// the clip-space position of a 2D vertex: view-projection times world.
pub(crate) fn clip(d: &crate::prim::Draw, p: [f32; 3]) -> [f32; 4] { // wide-ui: pub(crate)
    let w = &d.world;
    let q = [
        w[0] * p[0] + w[1] * p[1] + w[2] * p[2] + w[3],
        w[4] * p[0] + w[5] * p[1] + w[6] * p[2] + w[7],
        w[8] * p[0] + w[9] * p[1] + w[10] * p[2] + w[11],
        1.0,
    ];
    let m = &d.view_proj;
    std::array::from_fn(|i| m[i * 4] * q[0] + m[i * 4 + 1] * q[1] + m[i * 4 + 2] * q[2] + m[i * 4 + 3] * q[3])
}

/// the draw's quads, when it is made of them: four vertices each, drawn
/// as two triangles of those four.
pub(crate) fn quads(d: &crate::prim::Draw) -> Option<Vec<[V; 4]>> { // gfx-2d: pub(crate)
    let n = d.vertices.len() / 24;
    if n == 0 || n % 4 != 0 || d.indices.len() != n / 4 * 6 {
        return None;
    }
    let mut out = Vec::with_capacity(n / 4);
    for q in 0..n / 4 {
        let base = (q * 4) as u32;
        if !d.indices[q * 6..q * 6 + 6].iter().all(|&i| i >= base && i < base + 4) {
            return None;
        }
        out.push(std::array::from_fn(|k| read_v(&d.vertices[(q * 4 + k) * 24..(q * 4 + k + 1) * 24])));
    }
    Some(out)
}

// ---- wallpapers DrawRect stretched ----
//
// overrides/widescreen.c stretches a rectangle across the full width of the
// upper 2D camera (fades, fills, wallpapers) across the wide screen. For a
// textured one, with this renderer on, it also says so here (the prim
// state, the quad, the centre and the widening): the quad is put back to
// its own size before the frame is drawn, and extended like any other
// full-width layer, so the wallpaper's artwork is not stretched. The
// emulated GPU, which hears nothing, keeps the stretch.

/// the rectangles noted since the upper screen's last frame, and those of
/// the frame being drawn.
static NOTED: std::sync::Mutex<Vec<(u32, u32, f32, f32)>> = std::sync::Mutex::new(Vec::new());
static CURRENT: std::sync::Mutex<Vec<(u32, u32, f32, f32)>> = std::sync::Mutex::new(Vec::new());

/// overrides/widescreen.c's fates_wide2_rect_hook.
pub extern "C" fn rect_hook(state: u32, quad: u32, centre: f32, k: f32) {
    if let Ok(mut n) = NOTED.lock() {
        if n.len() < 4096 {
            n.push((state, quad, centre, k));
        }
    }
}

/// the upper screen's frame is being drawn: the rectangles noted for it.
pub fn frame_rects() {
    let taken = std::mem::take(&mut *NOTED.lock().unwrap());
    *CURRENT.lock().unwrap() = taken;
}

/// puts the stretched quads of `state` back to their own size; whether
/// there were any.
fn unstretch(d: &mut crate::prim::Draw, state: u32) -> bool {
    let current = CURRENT.lock().unwrap();
    let mut any = false;
    for &(s, quad, centre, k) in current.iter().filter(|r| r.0 == state) {
        let _ = s;
        let at = quad as usize * 4 * 24;
        if k <= 1.0 || d.vertices.len() < at + 4 * 24 {
            continue;
        }
        for v in 0..4 {
            let o = at + v * 24;
            let x = f32::from_le_bytes(d.vertices[o..o + 4].try_into().unwrap());
            d.vertices[o..o + 4].copy_from_slice(&(centre + (x - centre) / k).to_le_bytes());
        }
        any = true;
    }
    any
}

/// extends a 2D draw's full-width quads into the sides (see above): the
/// draw is cut where its sides start, and the sides come back as a draw
/// to go right after it. `k` is the upper screen's widening, `state` the
/// prim::State it was decoded from.
pub fn extend_2d(d: &mut crate::prim::Draw, k: f32, state: u32) -> Option<crate::prim::Draw> {
    if crate::gfx3::corners(d, k) {
        return None; // gfx3: a corner vignette, moved to the wide screen's corners
    }
    if k <= 1.001 || !enabled() {
        return None;
    }
    let (tex, sampling) = d.textures[0]?;
    // a wallpaper DrawRect stretched: as the game drew it, first
    let restretch = d.vertices.clone();
    let unstretched = unstretch(d, state);
    let Some(mut qs) = quads(d) else {
        if log_on() {
            eprintln!("wide2-2d not quads: {} vertices {} indices tex {:?}", d.vertices.len() / 24, d.indices.len(), (tex.addr, tex.width, tex.height, tex.format));
        }
        if unstretched {
            d.vertices = restretch;
        }
        return None;
    };
    let edge = 1.0 / k;
    let unit = edge / 200.0;
    // the long axis on the screen of every vertex; the 2D camera is
    // orthographic, so w is 1 throughout
    let mut long = Vec::with_capacity(qs.len());
    let (mut bottom, mut top) = (f32::MAX, f32::MIN);
    for q in &qs {
        let mut l = [0f32; 4];
        for (i, v) in q.iter().enumerate() {
            let c = clip(d, v.p);
            if (c[3] - 1.0).abs() > 1e-3 {
                if log_on() {
                    eprintln!("wide2-2d not orthographic: w {} tex {:?}", c[3], d.textures[0].map(|(t, _)| (t.addr, t.width, t.height, t.format)));
                }
                if unstretched {
                    d.vertices = restretch;
                }
                return None;
            }
            l[i] = c[LONG];
            bottom = bottom.min(c[1 - LONG]);
            top = top.max(c[1 - LONG]);
        }
        long.push(l);
    }
    let lo = long.iter().flatten().copied().fold(f32::MAX, f32::min);
    let hi = long.iter().flatten().copied().fold(f32::MIN, f32::max);
    let tol = unit * 1.5;
    // across the original 400, ending on its edges or a little past them
    // (wallpapers are often drawn a few units larger), short of the screen's
    let full = lo <= -edge + tol && hi >= edge - tol && lo >= -edge * OVERSIZE && hi <= edge * OVERSIZE;
    if log_on() {
        eprintln!(
            "wide2-2d frame {} {} quads long {lo:.4}..{hi:.4} (edge {edge:.4}) tex {:?} full {full}",
            zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
            qs.len(),
            d.textures[0].map(|(t, _)| (t.addr, t.width, t.height, t.format)),
        );
    }
    if !full {
        if unstretched {
            // not one after all: as the stretch had it
            d.vertices = restretch;
        }
        return None;
    }
    let strip = unit * 12.0;
    // wide-ui: with the sides framed (Artwork, Black), a layer ending on
    // the 400's edge is drawn as the game made it up to there, and its
    // sides start there (one reaching past the edge, a phase banner's
    // 420-wide shade, keeps its strip: what it has past the 400 is not
    // the 5:3 picture)
    let framed = crate::wide_ui::sides() != crate::wide_ui::Sides::Extend;
    // a layer about the screen's height softens down it, a band across
    let tall = bottom <= -0.9 && top >= 0.9;
    let mut extra = Vec::new();
    for (q, l) in qs.iter_mut().zip(&long) {
        let qlo = l.iter().copied().fold(f32::MAX, f32::min);
        let qhi = l.iter().copied().fold(f32::MIN, f32::max);
        if qhi - qlo < unit * 18.0 { // wide-ui: 1.5 times the 12-unit strip, whatever the strip
            continue;
        }
        for (end, sign) in [(qlo, -1.0f32), (qhi, 1.0f32)] {
            if sign * end < edge - tol || sign * end >= 1.0 {
                continue;
            }
            let strip = if framed && (sign * end - edge).abs() <= tol { 0.0 } else { strip }; // wide-ui
            // the quad's two vertices at this end and the two at the
            // other, paired across the long axis
            let mut at_end: Vec<usize> = (0..4).filter(|&i| (l[i] - end).abs() < unit * 0.5).collect();
            let mut far: Vec<usize> = (0..4).filter(|i| !at_end.contains(i)).collect();
            if at_end.len() != 2 || far.len() != 2 {
                continue;
            }
            let across: Vec<f32> = q.iter().map(|v| clip(d, v.p)[1 - LONG]).collect();
            at_end.sort_by(|&a, &b| across[a].total_cmp(&across[b]));
            far.sort_by(|&a, &b| across[a].total_cmp(&across[b]));
            let mut side = *q;
            let mut cut = *q;
            for j in 0..2 {
                let (e, f) = (at_end[j], far[j]);
                let span = l[f] - l[e];
                // where the strip starts, and just past the screen's edge
                let t_in = -sign * strip / span;
                let t_out = (sign * (1.0 + unit * 2.0) - l[e]) / span;
                let inner = lerp_v(&q[e], &q[f], t_in);
                let mut outer = lerp_v(&q[e], &q[f], t_out);
                outer.uv = q[e].uv;
                outer.c = q[e].c;
                let mut side_inner = inner;
                if !tall {
                    // wide3: a band carries on as one column from well
                    // inside its end, the same across the whole side: its
                    // trims and tone continue as lines. Its last texels
                    // were the 3DS screen's edge (the Armory's header bar
                    // fades out there), and the softened copy's blur
                    // across mixes in what lies past them in the texture,
                    // so a strip from the very end came out translucent,
                    // lighter at the seam and at the screen's edge.
                    let col = lerp_v(&q[e], &q[f], -sign * (unit * BAND_COLUMN).min(span.abs() * 0.5) / span);
                    outer.uv = col.uv;
                    outer.c = col.c;
                    side_inner.uv = col.uv;
                    side_inner.c = col.c;
                }
                side[e] = outer;
                side[f] = side_inner;
                cut[e] = inner;
            }
            *q = cut;
            extra.push(side);
        }
    }
    if extra.is_empty() {
        if unstretched {
            d.vertices = restretch;
        }
        return None;
    }
    let lowest = *d.indices[..6].iter().min().unwrap();
    let pattern: Vec<u32> = d.indices[..6].iter().map(|&i| i - lowest).collect();
    let pack = |list: &[[V; 4]]| {
        let mut vertices = Vec::with_capacity(list.len() * 96);
        let mut indices = Vec::with_capacity(list.len() * 6);
        for (n, q) in list.iter().enumerate() {
            for v in q {
                write_v(v, &mut vertices);
            }
            indices.extend(pattern.iter().map(|&i| i + 4 * n as u32));
        }
        (vertices, indices)
    };
    let mut sides = d.clone();
    (d.vertices, d.indices) = pack(&qs);
    (sides.vertices, sides.indices) = pack(&extra);
    sides.textures[0] = Some((soft_ref(tex, tall), sampling));
    Some(sides)
}

/// wide3: how far inside its end, in units of the 400, a band's side takes
/// its column: past the softened copy's blur across (two passes of a 512
/// texture's 8 texels) from the band's end.
const BAND_COLUMN: f32 = 20.0;

/// the format bits marking a texture's softened copy (`soft_ref`): down
/// the screen (t), or across it (s).
pub(crate) const SOFT_T: u32 = 0x4000_0000; // wide-ui: pub(crate)
pub(crate) const SOFT_S: u32 = 0x2000_0000; // wide-ui: pub(crate)

/// a texture's softened copy, as the renderer's texture lookups know it.
fn soft_ref(tex: crate::prim::TexRef, along_t: bool) -> crate::prim::TexRef {
    crate::prim::TexRef { format: tex.format | if along_t { SOFT_T } else { SOFT_S }, ..tex }
}

/// the texture a softened copy is made from, and which way it softens.
pub fn softened(tex: crate::prim::TexRef) -> Option<(crate::prim::TexRef, bool)> {
    let marks = tex.format & (SOFT_T | SOFT_S);
    (marks != 0).then(|| (crate::prim::TexRef { format: tex.format & !(SOFT_T | SOFT_S), ..tex }, marks & SOFT_T != 0))
}

/// the key of a softened copy of the texture decoded under (base, key),
/// made the first time it is asked for.
pub fn soft_key(
    decoded: &mut std::collections::HashMap<(crate::prim::TexRef, u64), Arc<crate::render::Decoded>>,
    marked: crate::prim::TexRef,
    base: crate::prim::TexRef,
    key: u64,
    along_t: bool,
) -> Option<u64> {
    let soft = key ^ 0x5769_6465_3253_6F66; // "Wide2Sof"
    if !decoded.contains_key(&(marked, soft)) {
        let d = decoded.get(&(base, key))?.clone();
        // hdpacks: a texture pack's picture is softened at the game's own
        // size (its smaller size that is), as the blur takes away what it
        // adds, and a 4x picture took 64 times as long (seconds) to blur
        let d = crate::hd::at_size(&d, base.width, base.height).map_or(d, Arc::new);
        let made = if along_t { blurred(&d) } else { blurred_across(&d) };
        decoded.insert((marked, soft), Arc::new(made));
    }
    Some(soft)
}

/// moflexhd: one box pass of the softened copies: each texel the mean
/// (rounded down) of the texels within r of it along t (down the texture)
/// or along s, the window cut at the texture's ends, channel by channel.
/// A running sum per column (along t) or per row (along s) gives each
/// texel's window sum with an add and a take, not 2r + 1 adds. A large
/// texture (an HD movie's picture) is cut into bands of rows that threads
/// of their own soften side by side, each band's sums started from its own
/// first window: the same texels either way.
pub(crate) fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, along_t: bool, r: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let parts = if w * h >= 256 * 1024 { (cores / 2).clamp(1, 4).min(h) } else { 1 };
    if parts <= 1 {
        box_rows(src, &mut dst[..w * h * 4], w, h, along_t, r, 0);
        return;
    }
    let per = h.div_ceil(parts);
    std::thread::scope(|scope| {
        let mut rest = &mut dst[..w * h * 4];
        let mut y0 = 0;
        while y0 < h {
            let y1 = (y0 + per).min(h);
            let (band, after) = rest.split_at_mut((y1 - y0) * w * 4);
            rest = after;
            let first = y0;
            scope.spawn(move || box_rows(src, band, w, h, along_t, r, first));
            y0 = y1;
        }
    });
}

/// box_pass for the rows from y0 on that `out` holds.
fn box_rows(src: &[u8], out: &mut [u8], w: usize, h: usize, along_t: bool, r: usize, y0: usize) {
    let rows = out.len() / (w * 4);
    if along_t {
        // the window of row y0: rows y0 - r to y0 + r, cut at the ends
        let mut sums = vec![0u32; w * 4];
        let (a, b) = (y0.saturating_sub(r), (y0 + r).min(h - 1));
        for row in src[a * w * 4..(b + 1) * w * 4].chunks_exact(w * 4) {
            for (sum, &v) in sums.iter_mut().zip(row) {
                *sum += v as u32;
            }
        }
        for y in y0..y0 + rows {
            let n = ((y + r).min(h - 1) - y.saturating_sub(r) + 1) as u32;
            for (o, &sum) in out[(y - y0) * w * 4..(y - y0 + 1) * w * 4].iter_mut().zip(&sums) {
                *o = (sum / n) as u8;
            }
            // the window moves down a row: the next row in, the first out
            if y + r + 1 < h {
                for (sum, &v) in sums.iter_mut().zip(&src[(y + r + 1) * w * 4..(y + r + 2) * w * 4]) {
                    *sum += v as u32;
                }
            }
            if y >= r {
                for (sum, &v) in sums.iter_mut().zip(&src[(y - r) * w * 4..(y - r + 1) * w * 4]) {
                    *sum -= v as u32;
                }
            }
        }
    } else {
        for y in y0..y0 + rows {
            let row = &src[y * w * 4..(y + 1) * w * 4];
            let o = &mut out[(y - y0) * w * 4..(y - y0 + 1) * w * 4];
            let mut sum = [0u32; 4];
            for texel in row[..(r.min(w - 1) + 1) * 4].chunks_exact(4) {
                for c in 0..4 {
                    sum[c] += texel[c] as u32;
                }
            }
            for x in 0..w {
                let n = ((x + r).min(w - 1) - x.saturating_sub(r) + 1) as u32;
                for c in 0..4 {
                    o[x * 4 + c] = (sum[c] / n) as u8;
                }
                if x + r + 1 < w {
                    for c in 0..4 {
                        sum[c] += row[(x + r + 1) * 4 + c] as u32;
                    }
                }
                if x >= r {
                    for c in 0..4 {
                        sum[c] -= row[(x - r) * 4 + c] as u32;
                    }
                }
            }
        }
    }
}

/// a texture's largest size blurred along s (across the screen): a band's
/// rows each carry on in their own average colour.
fn blurred_across(d: &crate::render::Decoded) -> crate::render::Decoded {
    let (w, h) = (d.width as usize, d.height as usize);
    let mut rgba = d.rgba.clone();
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return crate::render::Decoded { width: d.width, height: d.height, rgba, levels: d.levels };
    }
    static R: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let div = *R.get_or_init(|| std::env::var("FATES_WIDE2_SOFT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(64));
    if div == 0 {
        return crate::render::Decoded { width: d.width, height: d.height, rgba, levels: d.levels };
    }
    let r = (w / div).max(1);
    for _ in 0..2 {
        let src = rgba[..w * h * 4].to_vec();
        box_pass(&src, &mut rgba[..w * h * 4], w, h, false, r); // moflexhd: running sums
    }
    crate::render::Decoded { width: d.width, height: d.height, rgba, levels: d.levels }
}

/// which clip coordinate runs along the upper screen's 400: the PICA's y,
/// its framebuffer being portrait.
pub(crate) const LONG: usize = 1; // wide-ui: pub(crate)
