//! wide2: full-width layers of the upper screen's 3D telops (the phase
//! banners' shade, the Dragon Vein's wallpaper, chapter title cards and
//! their fades), extended into the wide sides on the emulated GPU, the
//! Compatibility renderer. The native renderer does the same in
//! fates-gfx's wide2.rs, which describes the layers; here the draw's
//! vertices, shaded on the CPU, get the sides as more triangles of the
//! same draw:
//!
//! - a plane ending on the original view's edge: its outer strip (the last
//!   12 units of the 400) widened from there to the screen's edge;
//! - a plane a little larger than the view: its column just inside its
//!   end spread, mirrored, across the rest of the side.
//!
//! The plane itself is cut where its sides start. A draw is looked at only
//! when the screen is widened, it draws the whole upper screen's viewport,
//! and it has few vertices (`wants_cpu`): it is shaded on the CPU to see
//! whether it is such a layer, which for a handful of vertices costs
//! little, and only a layer is then drawn from the CPU's vertices; every
//! other draw goes on to the GPU as before.

use super::{assemble, Vertex, REG_GEOSTAGE_CONFIG, REG_PRIMITIVE_CONFIG};

/// the most vertices a draw may have to be looked at.
const MOST_VERTICES: usize = 16;
/// how near a plane's ends must be to the view's edges, and how near its
/// top and bottom to the screen's, in normalised device units.
const EDGE_TOLERANCE: f32 = 0.008;
const HEIGHT_TOLERANCE: f32 = 0.02;
/// how much larger than the view a plane may be.
const OVERSIZE: f32 = 1.25;
/// where an edge plane's outer strip starts, and the column a larger
/// plane's side repeats, as shares of its end (fates-gfx wide2.rs).
const OUTER_STRIP: f32 = 0.94;
const SPREAD_COLUMN: f32 = 0.985;

fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_WIDE2_LOG").is_some())
}

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_WIDE2_EXTEND").map_or(true, |v| v != "0"))
}

/// whether a draw is one to look at: shaded on the CPU then.
pub(super) fn wants_cpu(registers: &[u32], viewport: (f32, f32, f32, f32), vertices: usize) -> bool {
    enabled()
        && crate::wide::factor() > 1.001
        && vertices <= MOST_VERTICES * 2
        && viewport.2 == 240.0
        && viewport.3 == 400.0
        && registers[REG_GEOSTAGE_CONFIG] & 0x3 != 2
}

/// the long axis (the PICA's y, along the upper screen's 400) of a vertex
/// in normalised device units, its short axis, and w.
fn ndc(v: &Vertex) -> (f32, f32, f32) {
    (v.clip[1] / v.clip[3], v.clip[0] / v.clip[3], v.clip[3])
}

/// the polygon's part on the side of `y = at * w` that `keep` says (true:
/// at and beyond, on at's own side of the middle).
fn clip_at(polygon: &[Vertex], at: f32, high: bool) -> Vec<Vertex> {
    let distance = |v: &Vertex| {
        let d = v.clip[1] - at * v.clip[3];
        if high { d } else { -d }
    };
    let mut out = Vec::with_capacity(polygon.len() + 2);
    for i in 0..polygon.len() {
        let (a, b) = (&polygon[i], &polygon[(i + 1) % polygon.len()]);
        let (da, db) = (distance(a), distance(b));
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a.lerp(b, da / (da - db)));
        }
    }
    out
}

/// a triangle list of the polygon, a fan.
fn fan(polygon: &[Vertex], out: &mut Vec<Vertex>) {
    for i in 2..polygon.len() {
        out.extend([polygon[0], polygon[i - 1], polygon[i]]);
    }
}

/// the draw with its sides, when it is a full-width layer: the registers
/// to place it with (a triangle list) and its vertices, three a triangle.
pub(super) fn extend(registers: &[u32], shaded: &[Vertex], order: Option<&[usize]>) -> Option<(Vec<u32>, Vec<Vertex>)> {
    let k = crate::wide::factor();
    if k <= 1.001 || !enabled() {
        return None;
    }
    let count = order.map_or(shaded.len(), <[usize]>::len);
    let at = |i: usize| order.map_or(i, |order| order[i]);
    let topology = (registers[REG_PRIMITIVE_CONFIG] >> 8) & 0x3;
    // 3, as a geometry shader would leave them, is a list without one
    if count == 0 {
        return None;
    }
    let triangles = assemble(topology, count);
    // the plane's extent
    let (mut lo, mut hi, mut bottom, mut top) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    let (mut near, mut far) = (f32::MAX, f32::MIN);
    for i in 0..count {
        let (l, s, w) = ndc(&shaded[at(i)]);
        if w <= 1e-5 {
            return None;
        }
        lo = lo.min(l);
        hi = hi.max(l);
        bottom = bottom.min(s);
        top = top.max(s);
        near = near.min(w);
        far = far.max(w);
    }
    let edge = 1.0 / k;
    if log_on() && hi - lo > edge {
        eprintln!("wide2-emu extend {count} long {lo:.4}..{hi:.4} short {bottom:.3}..{top:.3} w {near}..{far} topology {topology}");
    }
    let full = lo <= -edge + EDGE_TOLERANCE
        && hi >= edge - EDGE_TOLERANCE
        && bottom <= -1.0 + HEIGHT_TOLERANCE
        && top >= 1.0 - HEIGHT_TOLERANCE
        && lo >= -edge * OVERSIZE
        && hi <= edge * OVERSIZE
        && bottom >= -OVERSIZE
        && top <= OVERSIZE
        && far - near <= 0.01 * far.abs();
    if !full {
        return None;
    }
    // each side the plane leaves uncovered: its end, whether the plane
    // ends on the view's edge, and where its side starts
    let mut sides = Vec::new();
    for (end, high) in [(hi, true), (lo, false)] {
        if end.abs() < 1.0 {
            let on_edge = (end.abs() - edge).abs() < EDGE_TOLERANCE;
            let from = end * if on_edge { OUTER_STRIP } else { SPREAD_COLUMN };
            sides.push((end, high, on_edge, from));
        }
    }
    if sides.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(triangles.len() * 9);
    for &(a, b, c) in &triangles {
        let triangle = [shaded[at(a)], shaded[at(b)], shaded[at(c)]];
        // the plane, cut where its sides start
        let mut cut = triangle.to_vec();
        for &(_, high, _, from) in &sides {
            cut = clip_at(&cut, from, !high);
        }
        fan(&cut, &mut out);
        for &(end, high, on_edge, from) in &sides {
            // the triangle moved: y' = from + (y - from) * widen (the strip
            // widened), or y' = from - (y - from) * spread (the column
            // spread, mirrored), then what lands on the side kept
            let widen = if on_edge {
                (1.0 - from.abs()) / (end.abs() - from.abs()) * 1.01
            } else {
                -((1.0 - end.abs()) * 200.0 * k).clamp(8.0, 512.0)
            };
            let mut moved: Vec<Vertex> = triangle
                .iter()
                .map(|v| {
                    let mut v = *v;
                    let f = from * v.clip[3];
                    v.clip[1] = f + (v.clip[1] - f) * widen;
                    v
                })
                .collect();
            if widen < 0.0 {
                // mirrored: wound the same way again
                moved.swap(1, 2);
            }
            let side = clip_at(&moved, from, high);
            fan(&side, &mut out);
        }
    }
    let mut placing = registers.to_vec();
    placing[REG_PRIMITIVE_CONFIG] &= !(0x3 << 8);
    Some((placing, out))
}
