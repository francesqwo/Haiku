//! gfx-2d: talk portraits whose art runs past the 3DS screen's edge.
//!
//! A talk scene places its portraits (FaceCallback prim states, 256x256
//! textures) partly off the console's 400-wide screen: Elise on the left in
//! Chapter 3's map talk is a quad from x = -28 to 228, Corrin on the right
//! one from 172 to 428. The artists cut those pictures at the texture's edge,
//! where the console's screen hid it. On the widened screen the 2D layout
//! keeps the 400 in the middle and the sides show what lies past it, so the
//! cut showed as a hard vertical edge through hair and arms.
//!
//! Such an end is faded out instead: the quad is split a little inside its
//! end, and the strip between has the vertex alpha going from the state's to
//! zero at the end (the portrait combiner multiplies the vertex alpha in).
//! The fade stays in the side, outside the original 400, so the console's
//! picture is untouched, and the art keeps its size: nothing is stretched.
//!
//! FATES_GFX_PORTRAIT_FADE=0 turns it off.
//!
//! Portraits are also sampled smoothly when the picture is drawn above the
//! console's resolution (`smooth`). FaceCallback::SetupTexture binds the
//! face's layers nearest-filtered, and talk and event portraits are drawn
//! magnified (about 1.4 to 1.8 times in the event scenes): on the console's
//! 240 lines that is a slight blockiness, at 8x it is blocks of 10 to 15
//! pixels with stair-stepped outlines, which the user saw as glitchy faces.
//! Bilinear magnification keeps the art's own lines; the layers' edges did
//! not fringe in the talk and creator portraits checked.
//! FATES_GFX_PORTRAIT_SMOOTH=0 keeps them nearest (as the console).

use crate::prim::Draw;
use crate::wide2::{clip, lerp_v, quads, write_v, LONG, V};

/// how far in from the end the fade reaches, in units of the 400, at most
/// (it never reaches inside the original screen).
const FADE: f32 = 24.0;

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_GFX_PORTRAIT_FADE").map_or(true, |v| v != "0"))
}

/// whether portraits are magnified smoothly: above scale 1, unless
/// FATES_GFX_PORTRAIT_SMOOTH=0.
pub fn smooth(system: &zakuro_core::System) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_GFX_PORTRAIT_SMOOTH").map_or(true, |v| v != "0")) && system.gpu.scale() > 1
}

/// gfxmodels3: whether every portrait quad carries both its strips (see
/// fade_cut); FATES_GFX_PORTRAIT_STEADY=0 for the old way.
fn steady() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_GFX_PORTRAIT_STEADY").map_or(true, |v| v != "0"))
}

/// fades the ends of a portrait's quads that lie in the wide sides. `k` is
/// the upper screen's widening.
pub fn fade_cut(d: &mut Draw, k: f32) {
    if k <= 1.001 || !enabled() {
        return;
    }
    let Some(qs) = quads(d) else { return };
    let edge = 1.0 / k;
    let unit = edge / 200.0;
    let mut out: Vec<[V; 4]> = Vec::with_capacity(qs.len() * 3);
    let mut changed = false;
    // gfxmodels3: every quad carries both strips, zero wide where its end
    // needs no fade (inside the 400, or past the screen's edge): the draw
    // keeps one vertex count while a portrait slides through the side, so
    // pictures in between match it with the frame before's. A count that
    // changed (6 -> 12 as the level-up portrait's end came on screen, 12 ->
    // 6 as it settled) left the body unmatched: held at the newer frame,
    // with its face layers moving apart from it, at the slide's second
    // step. FATES_GFX_PORTRAIT_STEADY=0 adds strips only where they fade.
    let steady = steady();
    for q in qs {
        if q.iter().any(|v| (clip(d, v.p)[3] - 1.0).abs() > 1e-3) {
            out.push(q);
            if steady {
                // not one of ours to fade: two strips of nothing all the same
                let mut none = q[0];
                none.c[3] = 0;
                out.extend([[none; 4], [none; 4]]);
                changed = true;
            }
            continue;
        }
        let mut rest = q;
        let mut strips = Vec::new();
        for sign in [-1.0f32, 1.0] {
            let c: Vec<[f32; 4]> = rest.iter().map(|v| clip(d, v.p)).collect();
            let l: [f32; 4] = std::array::from_fn(|i| c[i][LONG]);
            let across: [f32; 4] = std::array::from_fn(|i| c[i][1 - LONG]);
            let end = if sign < 0.0 { l.iter().copied().fold(f32::MAX, f32::min) } else { l.iter().copied().fold(f32::MIN, f32::max) };
            let past = sign * end - edge;
            let mut at_end: Vec<usize> = (0..4).filter(|&i| (l[i] - end).abs() < unit * 0.5).collect();
            let mut far: Vec<usize> = (0..4).filter(|i| !at_end.contains(i)).collect();
            // an end in the side, short of the screen's edge
            let fades = past > unit * 0.5 && sign * end < 1.0;
            if at_end.len() != 2 || far.len() != 2 {
                if steady {
                    let mut none = rest[0];
                    none.c[3] = 0;
                    strips.push([none; 4]);
                }
                continue;
            }
            if !fades && !steady {
                continue;
            }
            at_end.sort_by(|&a, &b| across[a].total_cmp(&across[b]));
            far.sort_by(|&a, &b| across[a].total_cmp(&across[b]));
            let mut strip = rest;
            let reach = if fades { past.min(unit * FADE) } else { 0.0 };
            for j in 0..2 {
                let (e, f) = (at_end[j], far[j]);
                let span = l[f] - l[e];
                if span.abs() <= reach || reach <= 0.0 {
                    // gfxmodels3 (steady): no fade here, the strip is the
                    // end's edge, nothing wide
                    let mut outer = rest[e];
                    outer.c[3] = 0;
                    strip[e] = outer;
                    strip[f] = rest[e];
                    continue;
                }
                let inner = lerp_v(&rest[e], &rest[f], -sign * reach / span);
                let mut outer = rest[e];
                outer.c[3] = 0;
                strip[e] = outer;
                strip[f] = inner;
                rest[e] = inner;
            }
            strips.push(strip);
        }
        if !strips.is_empty() {
            changed = true;
        }
        out.push(rest);
        out.extend(strips);
    }
    if !changed {
        return;
    }
    let lowest = *d.indices[..6].iter().min().unwrap();
    let pattern: Vec<u32> = d.indices[..6].iter().map(|&i| i - lowest).collect();
    let mut vertices = Vec::with_capacity(out.len() * 96);
    let mut indices = Vec::with_capacity(out.len() * 6);
    for (n, q) in out.iter().enumerate() {
        for v in q {
            write_v(v, &mut vertices);
        }
        indices.extend(pattern.iter().map(|&i| i + 4 * n as u32));
    }
    d.vertices = vertices;
    d.indices = indices;
}
