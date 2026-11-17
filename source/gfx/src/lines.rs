//! gfx-lines: prim line states (shape 4) drawn natively.
//!
//! prim::Context draws a line state with its own ShaderObj, the one at
//! context +0x29E4 for shape 4: `shaders/line.shbin`, a vertex program and
//! the CTR SDK's line geometry shader (`dmp_Line.width` in c67).
//! RenderCall (0x001267B8) hands the state's indices to Pica::GeoDraw
//! (0x00130300), which runs the geometry shader once per index in point
//! mode and writes 0 to the geometry bool uniforms (b15) after each draw.
//!
//! What the programs do, from their disassembly (dev/gfx-lines/pdis.py):
//!
//! - Vertex program: `pos = ViewProj * (World * (aPos.xyz, 1))`,
//!   `colour = uColor * aColor`, `tex = aTexCoord` (no texture matrix).
//! - Geometry program, per index: b15 is clear on the first invocation of
//!   a draw (the hardware sets it after), which resets the pair counter
//!   r7.w and computes r15 = 1 / c67.xy. The next index completes a pair
//!   with the one kept in r8-r10, so the indices are GL_LINES: (0,1),
//!   (2,3), ... An odd last index draws nothing.
//! - A pair: both ends in front of the near plane (z > 0, the PICA keeps
//!   -w <= z <= 0) draw nothing; one end in front is moved along the line
//!   to z = 0 (position, colour and texture coordinate interpolated).
//!   The direction `d = B.xy * A.w - A.xy * B.w` picks the axis:
//!   `|d.x| * w/W > |d.y| * w/H` (the line is longer across clip x, in
//!   pixels) offsets both ends by `(0, +-end.w * w/W)`, otherwise by
//!   `(-+end.w * w/H, 0)`: half the width in pixels on the other axis,
//!   so the line is `w` pixels wide. It emits the quad A+a, A-a, B+b, B-b
//!   as two triangles, (A+a, A-a, B+b) and (B-b, A-a, B+b), with the
//!   winding flags set so both face the same way.
//! - c67 = (H/w, W/w, W*H, 2/w), from ShaderObj::SetLineSize (0x00131668)
//!   called by UpdateState with DispSize::GetW/GetH of the display (W 400
//!   upper, 320 lower; H 240) and the width at context +0x2428, indexed by
//!   the state's w1 bits 16-19. A width <= 0 leaves c67 as it was.
//!
//! `expand` does the same on the CPU and returns a triangle list in clip
//! space, divided by w (every line the game draws is in an orthographic
//! camera, w = 1; see dev/reports/gfx-lines.md), which the 2D pipeline then
//! draws with identity world and view-projection matrices.

use std::sync::atomic::{AtomicU64, Ordering};

use zakuro_core::System;

/// prim::Context +0x2428: the line widths (floats) the state's w1 bits
/// 16-19 index.
const CTX_LINE_WIDTHS: u32 = 0x2428;

/// line states drawn natively, the lines (pairs) in them, and lines that
/// drew nothing (both ends in front of the near plane, or zero width).
pub static STATES: AtomicU64 = AtomicU64::new(0);
pub static LINES: AtomicU64 = AtomicU64::new(0);
pub static CULLED: AtomicU64 = AtomicU64::new(0);
/// quads with a corner at w <= 0 after clipping, dropped (none seen).
pub static BEHIND: AtomicU64 = AtomicU64::new(0);

/// FATES_GFX_LINES=0 leaves line states to the island path (comparisons).
pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_LINES").map_or(true, |v| v != "0")
}

/// the state's line width in pixels, as UpdateState hands it to
/// ShaderObj::SetLineSize.
pub fn width(system: &mut System, prim: u32, w1: u32) -> f32 {
    let table = crate::prim::r32(system, prim + CTX_LINE_WIDTHS);
    f32::from_bits(crate::prim::r32(system, table + ((w1 >> 16) & 0xF) * 4))
}

/// DispSize::GetW / GetH (0x001306E4 / 0x001306C4): the display's landscape
/// size, 400 x 240 upper, 320 x 240 lower.
pub fn disp_size(disp: u32) -> (f32, f32) {
    if disp == 0 { (400.0, 240.0) } else { (320.0, 240.0) }
}

#[derive(Clone, Copy)]
struct V {
    clip: [f32; 4],
    uv: [f32; 2],
    rgba: [f32; 4],
}

fn lerp(a: &V, b: &V, ka: f32, kb: f32) -> V {
    V {
        clip: std::array::from_fn(|i| ka * a.clip[i] + kb * b.clip[i]),
        uv: std::array::from_fn(|i| ka * a.uv[i] + kb * b.uv[i]),
        rgba: std::array::from_fn(|i| ka * a.rgba[i] + kb * b.rgba[i]),
    }
}

fn row(m: &[f32], r: usize, v: [f32; 4]) -> f32 {
    m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2] + m[r * 4 + 3] * v[3]
}

/// The line state's vertices (24 bytes each, prim's format) and indices,
/// as the line programs draw them: a triangle list in prim's vertex format
/// whose positions are clip space divided by w.
pub fn expand(vertices: &[u8], indices: &[u32], chunk: usize, world: &[f32; 12], view_proj: &[f32; 16], width: f32, disp: u32) -> (Vec<u8>, Vec<u32>) {
    STATES.fetch_add(1, Ordering::Relaxed);
    let mut out = Vec::new();
    let mut out_indices = Vec::new();
    if !(width > 0.0) {
        CULLED.fetch_add((indices.len() / 2) as u64, Ordering::Relaxed);
        return (out, out_indices);
    }
    let (w_disp, h_disp) = disp_size(disp);
    // r15 = 1 / c67.xy
    let (sx, sy) = (width / h_disp, width / w_disp);
    let vertex = |i: u32| -> Option<V> {
        let o = i as usize * 24;
        let c = vertices.get(o..o + 24)?;
        let f = |k: usize| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap());
        let p = [f(0), f(1), f(2), 1.0];
        let w = [row(world, 0, p), row(world, 1, p), row(world, 2, p), 1.0];
        let clip = std::array::from_fn(|r| row(view_proj, r, w));
        Some(V { clip, uv: [f(3), f(4)], rgba: [c[20] as f32, c[21] as f32, c[22] as f32, c[23] as f32] })
    };
    let push = |out: &mut Vec<u8>, v: &V| -> Option<u32> {
        let w = v.clip[3];
        if !(w > 0.0) {
            return None;
        }
        let at = (out.len() / 24) as u32;
        for k in 0..3 {
            out.extend_from_slice(&(v.clip[k] / w).to_le_bytes());
        }
        out.extend_from_slice(&v.uv[0].to_le_bytes());
        out.extend_from_slice(&v.uv[1].to_le_bytes());
        out.extend(v.rgba.map(|c| c.round().clamp(0.0, 255.0) as u8));
        Some(at)
    };
    // each Pica::GeoDraw starts its pairs afresh (b15 clear)
    for draw in indices.chunks(chunk.max(2)) {
        for pair in draw.chunks_exact(2) {
            let (Some(mut a), Some(mut b)) = (vertex(pair[0]), vertex(pair[1])) else {
                continue;
            };
            let (a_front, b_front) = (a.clip[2] > 0.0, b.clip[2] > 0.0);
            if a_front && b_front {
                CULLED.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if a_front {
                let t = 1.0 / (a.clip[2] - b.clip[2]);
                a = lerp(&a, &b, -t * b.clip[2], t * a.clip[2]);
                a.clip[2] = 0.0;
            } else if b_front {
                let t = 1.0 / (b.clip[2] - a.clip[2]);
                b = lerp(&b, &a, -t * a.clip[2], t * b.clip[2]);
                b.clip[2] = 0.0;
            }
            let dx = b.clip[0] * a.clip[3] - a.clip[0] * b.clip[3];
            let dy = b.clip[1] * a.clip[3] - a.clip[1] * b.clip[3];
            let (oa, ob) = if dx.abs() * sy > dy.abs() * sx {
                ([0.0, a.clip[3] * sy], [0.0, b.clip[3] * sy])
            } else {
                ([-a.clip[3] * sx, 0.0], [-b.clip[3] * sx, 0.0])
            };
            let shift = |v: &V, o: [f32; 2], s: f32| {
                let mut v = *v;
                v.clip[0] += s * o[0];
                v.clip[1] += s * o[1];
                v
            };
            let corners = [shift(&a, oa, 1.0), shift(&a, oa, -1.0), shift(&b, ob, 1.0), shift(&b, ob, -1.0)];
            let mut at = [0u32; 4];
            let mut ok = true;
            let mark = out.len();
            for (k, c) in corners.iter().enumerate() {
                match push(&mut out, c) {
                    Some(i) => at[k] = i,
                    None => ok = false,
                }
            }
            if !ok {
                out.truncate(mark);
                BEHIND.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            LINES.fetch_add(1, Ordering::Relaxed);
            // (A+a, A-a, B+b), (B-b, A-a, B+b), both turned the way the
            // first one faces when A is left of B (the geometry program's
            // winding flags make them face alike)
            let [ap, am, bp, bm] = at;
            let ccw = |p: u32, q: u32, r: u32, out: &[u8]| {
                let xy = |i: u32| {
                    let o = i as usize * 24;
                    (f32::from_le_bytes(out[o..o + 4].try_into().unwrap()), f32::from_le_bytes(out[o + 4..o + 8].try_into().unwrap()))
                };
                let (p, q, r) = (xy(p), xy(q), xy(r));
                (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0) >= 0.0
            };
            for (p, q, r) in [(ap, am, bp), (bm, am, bp)] {
                if ccw(p, q, r, &out) {
                    out_indices.extend([p, q, r]);
                } else {
                    out_indices.extend([p, r, q]);
                }
            }
        }
    }
    (out, out_indices)
}

/// Where a line's edge falls exactly on pixel centres (a width of 1 puts
/// both edges on half pixels), the fill rule picks the row. The emulated
/// GPU rasterises in the PICA's portrait framebuffer, the native one in the
/// upright picture, turned a quarter: along the screen's vertical axis
/// (clip x) the two rules take opposite edges, and the native line came
/// out one row higher (dev/reports/gfx-lines.md). Moving the line down by
/// 1/64 of a 240-line pixel (down the screen is -clip x: the native target's
/// viewport turns clip space over) makes the native rule take the rows the
/// emulated one takes, at 1x (above Vulkan's 1/256 sub-pixel step) and at
/// every render scale, without moving any edge across a pixel centre it
/// was not on.
pub const TIE_NUDGE: f32 = 2.0 / 240.0 / 64.0;

pub fn nudge(vertices: &mut [u8]) {
    for v in vertices.chunks_exact_mut(24) {
        let x = f32::from_le_bytes(v[0..4].try_into().unwrap()) - TIE_NUDGE;
        v[0..4].copy_from_slice(&x.to_le_bytes());
    }
}

/// the counters, for the session's summary line.
pub fn summary() -> String {
    format!(
        "gfx-lines: line states drawn natively {}, lines {}, not drawn (in front of the near plane or no width) {}, dropped (behind the eye) {}",
        STATES.load(Ordering::Relaxed),
        LINES.load(Ordering::Relaxed),
        CULLED.load(Ordering::Relaxed),
        BEHIND.load(Ordering::Relaxed)
    )
}

pub const IDENTITY_34: [f32; 12] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];
pub const IDENTITY_44: [f32; 16] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// dev, FATES_GFX_LINES_TRACE=N: the first N line states, with the
/// headless frame, the state words and the vertices in clip space.
pub fn trace(system: &mut System, state: u32, vertices: &[u8], indices: &[u32], world: &[f32; 12], view_proj: &[f32; 16], width: f32, disp: u32, cull: u8, textured: bool) {
    static LEFT: std::sync::OnceLock<std::sync::atomic::AtomicI64> = std::sync::OnceLock::new();
    let left = LEFT.get_or_init(|| std::sync::atomic::AtomicI64::new(crate::perf::var("FATES_GFX_LINES_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0)));
    if left.fetch_sub(1, Ordering::Relaxed) <= 0 {
        return;
    }
    let words: Vec<String> = (0..6).map(|k| format!("{:08X}", crate::prim::r32(system, state + 4 * k))).collect();
    let frame = zakuro_core::native_gfx::HEADLESS_FRAME.load(Ordering::Relaxed);
    eprintln!(
        "gfx-lines-trace frame {frame} disp {disp} state {state:08X} [{}] indices {} width {width} cull {cull} textured {textured} world {world:?} vp {view_proj:?}",
        words.join(" "),
        indices.len()
    );
    for &i in indices.iter().take(8) {
        let o = i as usize * 24;
        let Some(c) = vertices.get(o..o + 24) else { continue };
        let f = |k: usize| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap());
        let p = [f(0), f(1), f(2), 1.0];
        let w = [row(world, 0, p), row(world, 1, p), row(world, 2, p), 1.0];
        let clip: [f32; 4] = std::array::from_fn(|r| row(view_proj, r, w));
        eprintln!("gfx-lines-trace   index {i} pos {:?} uv {:?} rgba {:?} clip {clip:?}", &p[..3], [f(3), f(4)], &c[20..24]);
    }
}

#[cfg(test)]
mod tests {
    /// `expand` against the geometry shader's own bytecode: lines-vectors.txt
    /// holds random line draws (orthographic and perspective, near-plane
    /// crossings, odd counts, both displays, several widths) and the
    /// triangles shaders/line.shbin emits for them, run by
    /// dev/gfx-lines/gsrun.py (a PICA interpreter with the hardware's
    /// point-mode rules: registers kept, b15 set after each invocation).
    #[test]
    fn expand_matches_the_geometry_shader() {
        // public: the vectors come from the game's own shader, so they are
        // not in the public repository; the test runs where they are
        let Ok(text) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lines-vectors.txt")) else {
            eprintln!("lines-vectors.txt is not here (made from the game): skipped");
            return;
        };
        let mut lines = text.lines();
        let nums = |l: &str| l.split_whitespace().map(|x| x.parse::<f32>().unwrap()).collect::<Vec<f32>>();
        let mut cases = 0;
        let mut triangles = 0;
        while let Some(head) = lines.next() {
            let h: Vec<&str> = head.split_whitespace().collect();
            let (disp, width, nv, nt): (u32, f32, usize, usize) = (h[1].parse().unwrap(), h[2].parse().unwrap(), h[3].parse().unwrap(), h[4].parse().unwrap());
            let world: [f32; 12] = nums(lines.next().unwrap()).try_into().unwrap();
            let vp: [f32; 16] = nums(lines.next().unwrap()).try_into().unwrap();
            let mut bytes = Vec::new();
            for _ in 0..nv {
                let v = nums(lines.next().unwrap());
                for x in &v[..5] {
                    bytes.extend_from_slice(&x.to_le_bytes());
                }
                bytes.extend(v[5..9].iter().map(|c| *c as u8));
            }
            let expected: Vec<Vec<f32>> = (0..nt).map(|_| nums(lines.next().unwrap())).collect();
            let indices: Vec<u32> = (0..nv as u32).collect();
            let (out, idx) = super::expand(&bytes, &indices, 0x2000, &world, &vp, width, disp);
            assert_eq!(idx.len() / 3, nt, "case {cases}: triangle count");
            let vert = |i: u32| -> [f32; 9] {
                let o = i as usize * 24;
                let f = |k: usize| f32::from_le_bytes(out[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
                [f(0), f(1), f(2), f(3), f(4), out[o + 20] as f32, out[o + 21] as f32, out[o + 22] as f32, out[o + 23] as f32]
            };
            let close = |a: &[f32], b: &[f32]| {
                (0..3).all(|k| (a[k] - b[k]).abs() <= 2e-4 * (1.0 + b[k].abs()))
                    && (3..5).all(|k| (a[k] - b[k]).abs() <= 1e-3 * (1.0 + b[k].abs()))
                    && (5..9).all(|k| (a[k] - b[k]).abs() <= 1.0)
            };
            let mut used = vec![false; nt];
            for t in idx.chunks_exact(3) {
                let got = [vert(t[0]), vert(t[1]), vert(t[2])];
                let found = expected.iter().enumerate().position(|(e, want)| {
                    !used[e] && {
                        let w: Vec<&[f32]> = want.chunks_exact(9).collect();
                        let perms = [[0, 1, 2], [1, 2, 0], [2, 0, 1], [0, 2, 1], [2, 1, 0], [1, 0, 2]];
                        perms.iter().any(|p| (0..3).all(|k| close(&got[k], w[p[k]])))
                    }
                });
                match found {
                    Some(e) => used[e] = true,
                    None => panic!("case {cases}: triangle {got:?} not among the shader's {expected:?}"),
                }
                // both triangles of a line face the same way
                let (p, q, r) = (got[0], got[1], got[2]);
                let area = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
                assert!(area >= -1e-9, "case {cases}: clockwise triangle");
            }
            cases += 1;
            triangles += nt;
        }
        assert!(cases >= 80 && triangles > 300);
    }
}
