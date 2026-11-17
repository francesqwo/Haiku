//! gfx2d3: the edges of 2D quads cut from an atlas, sampled as the console
//! samples them.
//!
//! The battle's damage numbers, "MISS!!", "NO DAMAGE" and much of the UI
//! are quads that each show one cell of a texture atlas, drawn with
//! linear filtering. The cells touch: the "1" of the damage atlas runs
//! from u = 18 to 34 and the "2" from 34 to 53. At the console's 1x the
//! pixels along a quad's edge sample half a texel inside it, where linear
//! filtering takes nothing from the next cell. At 8x the outermost render
//! pixels sample 1/16 of a texel inside, so they take almost half of the
//! neighbouring cell's edge texel: a dark line along the quad's sides,
//! fading over four pixels (the "black lines" on the battle text).
//!
//! The fix gives every vertex the texture rectangle of its quad, as the
//! texture coordinates the shader samples (after the texture matrix), and
//! the 2D fragment shader clamps unit 0's coordinate to half a texel
//! inside it. That is what the console's pixel centres do. Only edges
//! that are cut from the atlas are clamped: where two quads of a draw
//! meet in both texture and position (the "1" and "2" of "12" come from
//! touching cells and are drawn touching), the edge is left alone, so the
//! texture stays continuous across it.
//!
//! A triangle takes part only when its three corners are corners of one
//! texture rectangle (both triangles of a quad are). Fans and strips that
//! map a picture any other way (gauges, wedges) are not clamped.

use std::collections::HashMap;

/// No clamp: the rectangle every vertex gets when its draw or its
/// triangle does not take part.
pub const OPEN: [f32; 4] = [-1.0e30, -1.0e30, 1.0e30, 1.0e30];

/// On unless FATES_GFX_EDGE_CLAMP=0.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !crate::perf::var("FATES_GFX_EDGE_CLAMP").is_ok_and(|v| v == "0"))
}

fn f(v: &[u8], k: usize) -> f32 {
    f32::from_le_bytes([v[k], v[k + 1], v[k + 2], v[k + 3]])
}

/// Per vertex, the texture rectangle (u0, v0, u1, v1, in the shader's
/// texture coordinates) to clamp to, OPEN where nothing is clamped.
/// `vertices` are prim's 24-byte vertices, `tex` the draw's texture matrix
/// (3 rows of 4).
pub fn rects(vertices: &[u8], indices: &[u32], tex: &[f32; 12]) -> Vec<[f32; 4]> {
    let n = vertices.len() / 24;
    let mut out = vec![OPEN; n];
    if n == 0 || indices.len() < 3 || n > 1 << 16 {
        return out;
    }
    let uv: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let (u, v) = (f(vertices, i * 24 + 12), f(vertices, i * 24 + 16));
            [tex[0] * u + tex[1] * v + tex[3], tex[4] * u + tex[5] * v + tex[7]]
        })
        .collect();
    let pos = |i: usize| -> [i64; 3] { [0, 4, 8].map(|k| (f(vertices, i * 24 + k) * 64.0).round() as i64) };
    // the triangles that are a rectangle's corners, with their rectangle
    let mut tris: Vec<([usize; 3], [f32; 4])> = Vec::with_capacity(indices.len() / 3);
    for t in indices.chunks_exact(3) {
        let i = [t[0] as usize, t[1] as usize, t[2] as usize];
        if i.iter().any(|&k| k >= n) {
            continue;
        }
        let us = i.map(|k| uv[k][0]);
        let vs = i.map(|k| uv[k][1]);
        let r = [
            us.iter().cloned().fold(f32::MAX, f32::min),
            vs.iter().cloned().fold(f32::MAX, f32::min),
            us.iter().cloned().fold(f32::MIN, f32::max),
            vs.iter().cloned().fold(f32::MIN, f32::max),
        ];
        let tol = |a: f32, b: f32| (a - b).abs() <= 1e-5 * (1.0 + a.abs().max(b.abs()));
        let corner = |k: usize| (tol(us[k], r[0]) || tol(us[k], r[2])) && (tol(vs[k], r[1]) || tol(vs[k], r[3]));
        if r[2] - r[0] > 0.0 && r[3] - r[1] > 0.0 && (0..3).all(corner) {
            tris.push((i, r));
        }
    }
    if tris.is_empty() {
        return out;
    }
    // edges shared by two rectangles: the same texture coordinate on the
    // max side of one and the min side of the other, between the same two
    // positions
    type Key = (u8, u32, [i64; 3], [i64; 3]);
    let mut sides: HashMap<Key, u8> = HashMap::new();
    let edge_keys = |i: &[usize; 3], r: &[f32; 4]| -> Vec<(Key, u8)> {
        let mut keys = Vec::with_capacity(4);
        for axis in 0..2usize {
            for (side, value) in [(1u8, r[axis]), (2u8, r[axis + 2])] {
                let mut ps: Vec<[i64; 3]> = i.iter().filter(|&&k| (uv[k][axis] - value).abs() <= 1e-5 * (1.0 + value.abs())).map(|&k| pos(k)).collect();
                if ps.len() != 2 {
                    continue;
                }
                ps.sort();
                keys.push(((axis as u8, value.to_bits(), ps[0], ps[1]), side));
            }
        }
        keys
    };
    for (i, r) in &tris {
        for (key, side) in edge_keys(i, r) {
            *sides.entry(key).or_insert(0) |= side;
        }
    }
    // a vertex's rectangle; a vertex shared by triangles of different
    // rectangles, or by one that takes no part, is left open
    let mut assigned: Vec<Option<Option<[f32; 4]>>> = vec![None; n];
    let mut in_rect = vec![false; n];
    for (i, r) in &tris {
        let mut c = *r;
        for (key, side) in edge_keys(i, r) {
            if sides.get(&key) == Some(&3) {
                let k = key.0 as usize + if side == 2 { 2 } else { 0 };
                c[k] = OPEN[k];
            }
        }
        for &k in i {
            in_rect[k] = true;
            assigned[k] = match assigned[k] {
                None => Some(Some(c)),
                Some(Some(prev)) if prev == c || same_rect(&prev, r) => Some(Some(merge(prev, c))),
                _ => Some(None),
            };
        }
    }
    // an edge one triangle of a quad found shared is open for the whole
    // quad: the triangle without that edge may hold the provoking vertex
    for _ in 0..2 {
        for (i, r) in &tris {
            let mut c = Some(*r);
            for &k in i {
                c = match (c, assigned[k]) {
                    (Some(c), Some(Some(a))) if same_rect(&a, r) => Some(merge(c, a)),
                    _ => None,
                };
            }
            if let Some(c) = c {
                for &k in i {
                    assigned[k] = Some(Some(c));
                }
            }
        }
    }
    let corners: std::collections::HashSet<[usize; 3]> = tris.iter().map(|(i, _)| *i).collect();
    for t in indices.chunks_exact(3) {
        // a vertex also in a triangle that is no rectangle's corners
        let i = [t[0] as usize, t[1] as usize, t[2] as usize];
        if i.iter().any(|&k| k >= n) {
            continue;
        }
        if !corners.contains(&i) {
            for &k in &i {
                if in_rect[k] {
                    assigned[k] = Some(None);
                }
            }
        }
    }
    for (k, a) in assigned.into_iter().enumerate() {
        if let Some(Some(c)) = a {
            out[k] = c;
        }
    }
    out
}

/// Whether `a` (possibly with open edges) is the rectangle `r`.
fn same_rect(a: &[f32; 4], r: &[f32; 4]) -> bool {
    (0..4).all(|k| a[k] == r[k] || a[k] == OPEN[k])
}

/// The two triangles of one quad may each see a different edge shared:
/// an edge is open when either opened it.
fn merge(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [0, 1, 2, 3].map(|k| if a[k] == OPEN[k] || b[k] == OPEN[k] { OPEN[k] } else { a[k] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(v: &mut Vec<u8>, i: &mut Vec<u32>, x: [f32; 2], u: [f32; 2]) {
        let b = (v.len() / 24) as u32;
        for (py, pv) in [(0.0f32, 0.0f32), (27.0, 27.0)] {
            for (px, pu) in [(x[0], u[0]), (x[1], u[1])] {
                for f in [px, py, 0.0, pu, pv] {
                    v.extend_from_slice(&f.to_le_bytes());
                }
                v.extend_from_slice(&[255; 4]);
            }
        }
        i.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
    }

    const ID: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    #[test]
    fn a_lone_glyph_is_clamped_on_every_side() {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        quad(&mut v, &mut i, [-18.0, -2.0], [18.0, 34.0]);
        let r = rects(&v, &i, &ID);
        assert!(r.iter().all(|r| *r == [18.0, 0.0, 34.0, 27.0]));
    }

    #[test]
    fn touching_glyphs_keep_their_shared_edge_open() {
        // the "12" of the damage atlas: cells 18-34 and 34-53, drawn touching
        let (mut v, mut i) = (Vec::new(), Vec::new());
        quad(&mut v, &mut i, [-18.0, -2.0], [18.0, 34.0]);
        quad(&mut v, &mut i, [-2.0, 17.0], [34.0, 53.0]);
        let r = rects(&v, &i, &ID);
        assert_eq!(r[0], [18.0, 0.0, 1.0e30, 27.0]);
        assert_eq!(r[4], [-1.0e30, 0.0, 53.0, 27.0]);
    }

    #[test]
    fn cells_that_touch_only_in_the_atlas_are_clamped() {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        quad(&mut v, &mut i, [-18.0, -2.0], [18.0, 34.0]);
        quad(&mut v, &mut i, [10.0, 29.0], [34.0, 53.0]);
        let r = rects(&v, &i, &ID);
        assert_eq!(r[0], [18.0, 0.0, 34.0, 27.0]);
        assert_eq!(r[4], [34.0, 0.0, 53.0, 27.0]);
    }

    #[test]
    fn a_fan_is_not_clamped() {
        let mut v = Vec::new();
        for (x, y) in [(0.5f32, 0.5f32), (0.0, 0.0), (1.0, 0.0), (1.0, 0.7)] {
            for f in [x, y, 0.0, x, y] {
                v.extend_from_slice(&f.to_le_bytes());
            }
            v.extend_from_slice(&[255; 4]);
        }
        let r = rects(&v, &[0, 1, 2, 0, 2, 3], &ID);
        assert!(r.iter().all(|r| *r == OPEN));
    }
}
