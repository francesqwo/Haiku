//! gfx3: wide-screen fixes to 2D layers of the upper screen.
//!
//! **Corner vignettes.** The unit viewer (the detailed status's 3D model,
//! opened from the lower screen's magnifier) darkens the picture's corners
//! with one 2D state of four black quads, 128x120 each, at the corners of
//! the 400x240 screen, textured with a 64x64 alpha gradient. The upper 2D
//! camera keeps its 400 in the middle of a widened screen (overrides/
//! widescreen.c), so at 21:9 the dark corners sat at the 5:3 edges, inside
//! the picture, and the wide sides past them were not darkened: a dark,
//! blurred band down the screen on each side of the unit, with lighter
//! scenery beyond (the user's "black blurry effect"). Each corner piece is
//! moved, not stretched, to the true corner of the wide screen, so the
//! darkening is where the 3DS has it: at the screen's corners.
//!
//! FATES_GFX3_CORNERS=0 turns it off.

use crate::wide2::{clip, quads, write_v, LONG};

fn corners_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX3_CORNERS").map_or(true, |v| v != "0"))
}

/// a 2D draw of the upper screen, `k` its widening: when it is a corner
/// vignette (every quad dark, on one end of the 400 only, at the top or
/// the bottom, shorter than half the screen), its quads are moved out to
/// the wide screen's ends. Whether it was.
pub fn corners(d: &mut crate::prim::Draw, k: f32) -> bool {
    if k <= 1.001 || !corners_on() || d.textures[0].is_none() {
        return false;
    }
    let Some(qs) = quads(d) else { return false };
    if qs.len() < 2 || qs.len() > 8 {
        return false;
    }
    if !qs.iter().flatten().all(|v| v.c[0] < 16 && v.c[1] < 16 && v.c[2] < 16 && v.c[3] > 0) {
        return false;
    }
    let edge = 1.0 / k;
    let tol = edge / 200.0 * 1.5;
    let mut shifts = Vec::with_capacity(qs.len());
    for q in &qs {
        let c: Vec<[f32; 4]> = q.iter().map(|v| clip(d, v.p)).collect();
        if c.iter().any(|c| (c[3] - 1.0).abs() > 1e-3) {
            return false;
        }
        let lo = c.iter().map(|c| c[LONG]).fold(f32::MAX, f32::min);
        let hi = c.iter().map(|c| c[LONG]).fold(f32::MIN, f32::max);
        let across_lo = c.iter().map(|c| c[1 - LONG]).fold(f32::MAX, f32::min);
        let across_hi = c.iter().map(|c| c[1 - LONG]).fold(f32::MIN, f32::max);
        let at_lo = (lo + edge).abs() <= tol;
        let at_hi = (hi - edge).abs() <= tol;
        let at_top_or_bottom = across_lo <= -1.0 + 0.02 || across_hi >= 1.0 - 0.02;
        let short = hi - lo <= edge * 0.75 && across_hi - across_lo <= 1.25;
        if at_lo == at_hi || !at_top_or_bottom || !short {
            return false;
        }
        shifts.push(if at_lo { -(1.0 - edge) } else { 1.0 - edge });
    }
    // a move along the screen's long axis in clip space as a move of the
    // vertices: the camera is orthographic, so clip space is linear in them
    let p0 = qs[0][0].p;
    let base = clip(d, p0);
    let dx = clip(d, [p0[0] + 1.0, p0[1], p0[2]]);
    let dy = clip(d, [p0[0], p0[1] + 1.0, p0[2]]);
    let (a, b) = (dx[LONG] - base[LONG], dy[LONG] - base[LONG]);
    let (c, e) = (dx[1 - LONG] - base[1 - LONG], dy[1 - LONG] - base[1 - LONG]);
    let det = a * e - b * c;
    if det.abs() < 1e-9 {
        return false;
    }
    let mut vertices = Vec::with_capacity(d.vertices.len());
    for (q, s) in qs.iter().zip(&shifts) {
        // solve [a b; c e] (px, py) = (s, 0)
        let px = s * e / det;
        let py = -s * c / det;
        for v in q {
            let mut v = *v;
            v.p[0] += px;
            v.p[1] += py;
            write_v(&v, &mut vertices);
        }
    }
    d.vertices = vertices;
    true
}

/// dev, FATES_GFX3_TRACE=frame: the upper screen's untextured shaded draws
/// of that headless frame, where they are (normalised device units of the
/// wide screen) and how they blend. FATES_GFX3_DROP=1 leaves them out.
pub fn trace_made(made: &mut Vec<(crate::prim::Draw, [Option<u64>; 3])>, k: f32) {
    let frame = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let traced = crate::perf::var("FATES_GFX3_TRACE").ok().and_then(|v| v.parse::<u64>().ok()) == Some(frame);
    let drop = crate::perf::var("FATES_GFX3_DROP").is_ok_and(|v| v == "1");
    let drop_tex: Vec<u32> = crate::perf::var("FATES_GFX3_DROPTEX").map(|v| v.split(',').filter_map(|a| a.trim().parse().ok()).collect()).unwrap_or_default();
    if !traced && !drop && drop_tex.is_empty() {
        return;
    }
    made.retain(|(d, _)| {
        let Some(s) = d.shaded.as_deref() else { return true };
        let tex = s.textures[0].map(|t| t.0.addr);
        if tex.is_some_and(|a| drop_tex.contains(&a)) {
            return false;
        }
        if s.textures.iter().any(|t| t.is_some()) && !s.loose {
            return true;
        }
        if traced {
            let p = crate::wide2::projected_most(s, 64).unwrap_or_default();
            let lo = p.iter().map(|p| p.0).fold(f32::MAX, f32::min);
            let hi = p.iter().map(|p| p.0).fold(f32::MIN, f32::max);
            let b = p.iter().map(|p| p.1).fold(f32::MAX, f32::min);
            let t = p.iter().map(|p| p.1).fold(f32::MIN, f32::max);
            eprintln!(
                "gfx3 trace {frame}: tex {tex:?} loose {} v{} i{} blend {:08x} dw {} long {lo:.3}..{hi:.3} short {b:.3}..{t:.3} (edge {:.3}) vp {:?} sc {:?} program #{:08x}",
                s.loose,
                s.vertices.len(),
                s.indices.len(),
                s.key.blend.unwrap_or(u32::MAX),
                s.key.depth_write,
                1.0 / k,
                s.viewport,
                s.scissor,
                s.program.iter().take(4096).fold(0u32, |h, &w| h.rotate_left(5) ^ w)
            );
        }
        !(drop && tex.is_none())
    });
}

// **The skill activation's vignette.** A skill's activation (Astra, and
// every other banner) darkens the picture toward the banner's corner with
// a purple-black wash: an untextured plane of 32 vertices, a grid whose
// vertex colours make the gradient, facing the telop camera and as big as
// the 3DS's view (3 % larger). With the camera widened it ends at the 5:3
// edges, a hard vertical edge inside the picture with the wide side
// beyond it untouched (the user: "the vignette under the astra skill
// shouldn't be there for ultrawide"). At a wide aspect it is not drawn;
// the banner (icon, name, line) and the activation's particles stay, and
// at 5:3 nothing changes. FATES_GFX3_SKILL_WASH=1 keeps it (comparisons),
// FATES_GFX3_SKILL_LOG=1 logs each plane left out.

/// the fewest and most vertices of the wash's grid: planes of 4-16 are
/// wide2's telop layers, extended into the sides.
const WASH_VERTICES: std::ops::RangeInclusive<usize> = 17..=64;
/// how much larger than the view, along the long axis and up and down.
const WASH_LARGER: f32 = 1.10;

pub fn skill_wash(made: &mut Vec<(crate::prim::Draw, [Option<u64>; 3])>, k: f32) {
    static KEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if k <= 1.001 || *KEEP.get_or_init(|| crate::perf::var("FATES_GFX3_SKILL_WASH").is_ok_and(|v| v == "1")) {
        return;
    }
    let edge = 1.0 / k;
    made.retain(|(d, _)| {
        let Some(s) = d.shaded.as_deref() else { return true };
        if s.loose || s.textures.iter().any(|t| t.is_some()) || s.key.depth_write || s.key.blend.is_none() {
            return true;
        }
        let mut used: Vec<u32> = s.indices.to_vec(); // perf3 made indices an Arc
        used.sort_unstable();
        used.dedup();
        if !WASH_VERTICES.contains(&used.len()) {
            return true;
        }
        let Some(p) = crate::wide2::projected_most(s, 64) else { return true };
        if p.len() != used.len() {
            return true;
        }
        let lo = p.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        let hi = p.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let bottom = p.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let top = p.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        let near = p.iter().map(|p| p.2).fold(f32::MAX, f32::min);
        let far = p.iter().map(|p| p.2).fold(f32::MIN, f32::max);
        let tol = 0.008;
        // the 3DS's view, a little larger, facing the eye
        let across = lo <= -edge + tol && lo >= -edge * WASH_LARGER && hi >= edge - tol && hi <= edge * WASH_LARGER;
        let tall = bottom <= -1.0 + 0.02 && bottom >= -WASH_LARGER && top >= 1.0 - 0.02 && top <= WASH_LARGER;
        let flat = far - near <= 0.01 * far.abs();
        let wash = across && tall && flat;
        if wash && crate::perf::var("FATES_GFX3_SKILL_LOG").is_ok_and(|v| v == "1") {
            eprintln!(
                "gfx3 skill wash left out: frame {} v{} long {lo:.3}..{hi:.3} short {bottom:.3}..{top:.3} blend {:08x}",
                zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
                used.len(),
                s.key.blend.unwrap_or(0)
            );
        }
        !wash
    });
}
