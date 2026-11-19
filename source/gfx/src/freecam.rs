//! gfxcastle: the battle free camera's sky poles and distance haze.
//!
//! The battle backdrop is the field's sky dome (FieldSky, "sky/Btl_...": a
//! 512x512 panorama on a 97-vertex dome, made to be seen near the horizon
//! from the game's cameras). cameras3 centred it on the free camera's eye,
//! so it never runs out, but two things the game never shows remained:
//!
//! - looking up or down, the panorama's top and bottom rows are pinched
//!   into the dome's poles (a smear);
//! - flying away from the fight, the battle's terrain ends and its edge
//!   shows against the dome's lower band.
//!
//! While a free view is drawn, each draw of the dome and of the field's
//! meshes is followed by a **haze overlay**: the same mesh through the same
//! vertex program, untextured, its colour the haze's (the battle's own fog
//! colour, which the game already fades the distance into), its alpha worked
//! out per vertex in shade.vert (`SHADING_MODE`):
//!
//! - on the dome, by the elevation of the direction (in the dome's own
//!   frame, its up from its bone): clear near the horizon as the game shows
//!   it, into the haze from 35 degrees up (fully by 62), and from just below
//!   the horizon (fully by 25 down), so the poles are a smooth gradient and
//!   below the horizon there is no rocky band for the terrain's edge to
//!   show against;
//! - on the field's meshes, by the distance from the eye: from half the
//!   haze's reach to all of it (0.6 of the dome's radius; a battle's terrain
//!   reaches about 0.45 of it from its middle), so the far terrain and its
//!   edge melt into the same haze.
//!
//! The overlay draws only where its mesh drew (depth equal, no depth write),
//! so alpha-tested leaves stay leaves; additive and see-through draws, which
//! write no depth, get none.
//!
//! **The game's own camera is untouched.** A free view is told from the
//! game's by the camera itself: the battle's free camera puts its eye and
//! target in the field camera for its Update and the game's back after
//! (cameras.c apply_view), so the view the meshes are drawn with differs
//! from the eye and target the camera holds only while a free view is
//! drawn. The strength follows how far the view is from the game's (eye
//! and direction, with a dead zone), so the hand-over back to the game's
//! camera fades it out, and with the game's own view no draw is added.
//!
//! `FATES_FREECAM_HAZE=0` turns it off; `FATES_FREECAM_TRACE=lo-hi[,lo-hi]`
//! logs the views and the dome by the game's frame.

use std::sync::Arc;

use zakuro_core::System;

use crate::prim::{r32, Draw};

/// FieldWorld (its camera at +0x34, its sky at +0x10)
const FIELD_WORLD_PTR: u32 = 0x006B_E404;

/// shade.vert's Shading words the haze takes (the two output semantic slots
/// no output uses; wide2 has the block's `pad`): the mode (low byte), the
/// strength (next byte) and for the distance its full reach (high half);
/// the dome's up, ten bits a component.
const SHADING_MODE: usize = 404 + 22;
const SHADING_PARAM: usize = 404 + 23;
/// raster.frag's Draw words: the combiner stages (source, operand,
/// combiner, constant, scale, then three spare), misc (fog and buffer, buffer
/// colour, alpha test, texture units), flags (depth mode, lighting)
const STAGE_WORDS: usize = 8;
const DRAW_MISC: usize = 48;
const DRAW_FLAGS: usize = 64;

/// shade.vert's haze modes
const HAZE_SKY: u32 = 1;
const HAZE_FAR: u32 = 2;

/// where the distance haze is full, as a fraction of the dome's radius (it
/// starts at half of that). FATES_FREECAM_FOG=f
fn haze_reach() -> f32 {
    crate::perf::var("FATES_FREECAM_FOG").ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|f| *f > 0.05).unwrap_or(0.6)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct View {
    /// gfxcastle: the projection (trace)
    pub proj: [f32; 16],
    /// 0 the game's own view, 1 a free one
    pub strength: f32,
    /// the eye the meshes are drawn from, and the dome's radius (0.9 of the
    /// game's far plane, FieldSky::OnUpdate)
    pub eye: [f32; 3],
    pub dome: f32,
}

#[derive(Default)]
pub struct FreeCam {
    /// the gfx frame and camera the view below is for
    frame: u64,
    camera: u32,
    view: Option<View>,
    /// the haze's colour this frame (the dome's fog colour), RGBA8
    haze_frame: u64,
    haze_color: Option<u32>,
    /// overlays drawn
    pub overlays: u64,
    /// perf3: overlays left out, their mesh nearer than the haze
    pub culled: u64,
    /// perf3: the cull's answer for a draw placed as before (its vertices,
    /// its indices' count, every float uniform, which hold the camera and
    /// the model's place, and where the haze starts): whether it needs an
    /// overlay. A still camera asks the same each frame; anything moved
    /// asks again.
    near_cache: std::collections::HashMap<u64, bool>,
}

/// perf3: the most vertices a mesh may use for its overlay to be left out
/// when it is all nearer than the haze (see freecam_haze).
const CULL_VERTICES: usize = 64;

fn cull_on() -> bool {
    !crate::perf::var("FATES_FREECAM_CULL").is_ok_and(|v| v == "0")
}

fn on() -> bool {
    !crate::perf::var("FATES_FREECAM_HAZE").is_ok_and(|v| v == "0")
}

fn trace_on() -> bool {
    let Ok(v) = crate::perf::var("FATES_FREECAM_TRACE") else { return false };
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    v.split(',').filter_map(|r| r.split_once('-').and_then(|(a, b)| Some((a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?)))).any(|(lo, hi)| game >= lo && game <= hi)
}

fn f(system: &mut System, a: u32) -> f32 {
    f32::from_bits(r32(system, a))
}

fn v3(system: &mut System, a: u32) -> [f32; 3] {
    [f(system, a), f(system, a + 4), f(system, a + 8)]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// a unit vector as shade.vert reads it back: ten bits a component.
fn pack_axis(v: [f32; 3]) -> u32 {
    let q = |x: f32| (((x.clamp(-1.0, 1.0) + 1.0) * 511.5).round() as u32).min(1023);
    q(v[0]) | q(v[1]) << 10 | q(v[2]) << 20
}

/// the field camera's view for this draw, when the RenderState draws with it
/// (battles, the castle's view, cutscenes).
fn read_view(system: &mut System, state: u32) -> Option<View> {
    let camera = r32(system, state + 8);
    let world = r32(system, zakuro_core::region::us(FIELD_WORLD_PTR));
    if camera == 0 || world == 0 || camera != r32(system, world + 0x34) {
        return None;
    }
    let bind = r32(system, camera + 0x5C8) != 0;
    let base = if bind { camera + 0x618 } else { camera + 4 };
    let (game_eye, game_target) = (v3(system, base), v3(system, base + 12));
    // the view matrix the meshes are drawn with (3x4, world to view)
    let m: [f32; 12] = std::array::from_fn(|i| f(system, camera + 0x98 + 4 * i as u32));
    let t = [m[3], m[7], m[11]];
    let eye = [
        -(m[0] * t[0] + m[4] * t[1] + m[8] * t[2]),
        -(m[1] * t[0] + m[5] * t[1] + m[9] * t[2]),
        -(m[2] * t[0] + m[6] * t[1] + m[10] * t[2]),
    ];
    // it looks down its -z
    let ahead = [-m[8], -m[9], -m[10]];
    let to = sub(game_target, game_eye);
    let reach = len(to).max(1.0);
    let game_ahead = [to[0] / reach, to[1] / reach, to[2] / reach];
    let moved = len(sub(eye, game_eye)) / (0.5 * reach);
    let turned = dot(ahead, game_ahead).clamp(-1.0, 1.0).acos() / 0.35;
    if !(moved.is_finite() && turned.is_finite()) {
        return None;
    }
    // a dead zone: the view matrix worked back to an eye is not exact
    let strength = smooth((moved.max(turned) - 0.05) / 0.95);
    let far = f(system, camera + if bind { 0x648 } else { 0x34 });
    let dome = if far.is_finite() && far > 0.0 { 0.9 * far } else { 0.0 };
    let proj: [f32; 16] = std::array::from_fn(|i| f(system, camera + 0x1A8 + 4 * i as u32));
    Some(View { proj, strength, eye, dome })
}

/// the haze overlay of a draw: the same mesh and vertex program, the
/// combiners giving the haze's colour (stage 0's constant) and the vertex
/// program's alpha, blended over where the mesh drew.
fn overlay(s: &crate::scene::Shaded, colour: u32, mode: u32, param: u32) -> Option<crate::scene::Shaded> {
    let k = s.key;
    // where the mesh drew: its depth again. A mesh that writes none (see-
    // through, additive) or writes its own gets no overlay
    let depth_func = match (k.depth_test, k.depth_func) {
        (false, _) | (true, 1) => 1,
        (true, 4) | (true, 5) | (true, 2) => 2,
        (true, 6) | (true, 7) => 2,
        _ => return None,
    };
    if !k.depth_write && depth_func != 1 || k.writes_depth || k.mask & 0x7 == 0 {
        return None;
    }
    if s.draw.len() < DRAW_FLAGS + 4 || s.shading.len() < SHADING_PARAM + 1 {
        return None;
    }
    let mut o = s.clone();
    for stage in 0..6 {
        let at = stage * STAGE_WORDS;
        // stage 0: the constant's colour, the primary colour's alpha (the
        // vertex program's haze); the rest pass it on
        let (source, constant) = if stage == 0 { (0x0000_000E, colour | 0xFF00_0000) } else { (0x000F_000F, 0) };
        o.draw[at..at + 5].copy_from_slice(&[source, 0, 0, constant, 0]);
    }
    // no fog, no combiner buffer, no alpha test, no textures
    o.draw[DRAW_MISC] = 0;
    o.draw[DRAW_MISC + 2] = 0;
    o.draw[DRAW_MISC + 3] &= !0x407;
    o.draw[DRAW_FLAGS + 1] = 0;
    o.textures = [None; 3];
    o.key.lighting = false;
    o.key.procedural = false;
    o.key.depth_test = true;
    o.key.depth_func = depth_func;
    o.key.depth_write = false;
    // the frame's alpha (which the bloom reads: the panorama's pole glows)
    // goes the same way, toward 1, as near the horizon (no glow)
    // source alpha over (PICA blend: add; colour src alpha, one minus src
    // alpha; alpha the constant's alpha, one minus src alpha)
    o.key.blend = Some(0x7C76_0000);
    o.blend_color = [1.0; 4];
    o.shading[SHADING_MODE] = mode;
    o.shading[SHADING_PARAM] = param;
    Some(o)
}

impl crate::Gfx {
    /// gfxcastle: the haze over one DrawMesh call's draws while a free view
    /// is drawn (see the module).
    pub(crate) fn freecam_haze(&mut self, system: &mut System, model: u32, state: u32, mesh: u32, made: &mut Vec<(Draw, [Option<u64>; 3])>) {
        if !on() || made.is_empty() {
            return;
        }
        let frame = self.stats.frames;
        let camera = r32(system, state + 8);
        if self.freecam.frame != frame || self.freecam.camera != camera {
            self.freecam.frame = frame;
            self.freecam.camera = camera;
            self.freecam.view = read_view(system, state);
        }
        let Some(view) = self.freecam.view else { return };
        let world = r32(system, zakuro_core::region::us(FIELD_WORLD_PTR));
        let sky_actor = if world != 0 { r32(system, world + 0x10) } else { 0 };
        let sky = sky_actor != 0 && sky_actor.wrapping_add(0x80) == model;
        if trace_on() {
            self.freecam_trace(model, mesh, sky, &view, made);
        }
        if view.strength <= 0.0 || view.dome <= 0.0 {
            return;
        }
        if self.freecam.haze_frame != frame {
            self.freecam.haze_frame = frame;
            self.freecam.haze_color = None;
        }
        let strength = ((view.strength * 255.0).round() as u32).min(255);
        let (mode, param) = if sky {
            // the dome's up in its own frame: its bone's second row (the
            // dome is the game's scale and turn, centred on the eye)
            let bone = crate::h3d::mesh(system, mesh).and_then(|m| m.submeshes.first().and_then(|s| s.bones.first().copied())).unwrap_or(0);
            let matrices = r32(system, model + 0x24 + 0x14);
            let row: [f32; 3] = std::array::from_fn(|i| f(system, matrices + bone as u32 * 0x30 + 16 + 4 * i as u32));
            let l = len(row);
            let up = if l.is_finite() && l > 1e-6 { [row[0] / l, row[1] / l, row[2] / l] } else { [0.0, 1.0, 0.0] };
            (HAZE_SKY | strength << 8, pack_axis(up))
        } else {
            let reach = ((haze_reach() * view.dome).round() as u32).clamp(1, 0xFFFF);
            (HAZE_FAR | strength << 8 | reach << 16, 0)
        };
        // perf3: a small mesh (few vertices: grass, stones, the soldiers'
        // pieces, hundreds of them once the free camera backs out) wholly
        // nearer than where the distance haze starts would get an overlay of
        // alpha 0 everywhere: none is drawn. Its vertices are run through
        // its program as the GPU runs them (wide2::farthest_w), with a margin
        // for the pictures in between's moving eye. Larger meshes (the
        // terrain) and the dome always get theirs. FATES_FREECAM_CULL=0 keeps
        // every overlay.
        let near_end = if sky || !cull_on() { 0.0 } else { 0.45 * haze_reach() * view.dome };
        let mut out = Vec::with_capacity(made.len() * 2);
        for (d, keys) in made.drain(..) {
            let needed = near_end <= 0.0
                || !d.shaded.as_deref().is_some_and(|s| {
                    let floats = &s.shading[..s.shading.len().min(96 * 4)];
                    let key = crate::cache::hash_words(floats) ^ (Arc::as_ptr(&s.vertices) as u64).rotate_left(17) ^ (s.indices.len() as u64).rotate_left(41) ^ (near_end.to_bits() as u64).rotate_left(7);
                    if let Some(&near) = self.freecam.near_cache.get(&key) {
                        return near;
                    }
                    let near = crate::wide2::farthest_w(s, CULL_VERTICES).is_some_and(|w| w < near_end);
                    if self.freecam.near_cache.len() > 8192 {
                        self.freecam.near_cache.clear();
                    }
                    self.freecam.near_cache.insert(key, near);
                    near
                });
            if !needed {
                self.freecam.culled += 1;
            }
            let added = d.shaded.as_deref().filter(|_| needed).and_then(|s| {
                // the battle's own fog colour, as the dome has it; a battle
                // without fog: the dome's texture's average
                let fog = s.draw.len().checked_sub(4).map(|at| s.draw[at]);
                let fogged = s.draw.get(DRAW_MISC).is_some_and(|m| m & 7 == 5);
                if sky {
                    let colour = match (fogged, fog) {
                        (true, Some(c)) => Some(c & 0xFF_FFFF),
                        _ => s.textures[0].and_then(|(t, _)| keys[0].and_then(|k| self.decoded.get(&(t, k)))).map(|t| average(&t.rgba)),
                    };
                    if self.freecam.haze_color.is_none() {
                        self.freecam.haze_color = colour;
                    }
                }
                let colour = self.freecam.haze_color?;
                // skybg: a dome drawn with its cube map (skybg.rs) is the
                // real sky to the zenith and the nadir, no pinched poles to
                // hide: no haze over it (the field's distance haze stays)
                if sky && crate::skybg::is_cube(s) && !crate::skybg::sky_haze() {
                    return None;
                }
                overlay(s, colour, mode, param)
            });
            out.push((d.clone(), keys));
            if let Some(o) = added {
                let mut od = d;
                od.shaded = Some(Arc::new(o));
                out.push((od, [None; 3]));
                self.freecam.overlays += 1;
            }
        }
        *made = out;
    }

    fn freecam_trace(&mut self, model: u32, mesh: u32, sky: bool, view: &View, made: &[(Draw, [Option<u64>; 3])]) {
        let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
        let first = made.iter().find_map(|(d, _)| d.shaded.clone());
        let Some(s) = first else { return };
        eprintln!(
            "freecam g{game} model {model:08X} mesh {mesh:08X} sky {sky} strength {:.3} eye {:?} dome {:.0} draws {} indices {} misc {:08X} fog {:08X} key {:?}",
            view.strength,
            view.eye,
            view.dome,
            made.len(),
            s.indices.len(),
            s.draw.get(DRAW_MISC).copied().unwrap_or(0),
            s.draw.last().copied().unwrap_or(0),
            s.key
        );
        if sky {
            let fog: Vec<String> = (96..128).map(|i| {
                let at = (zakuro_gpu::native3d::FOG_TABLE + i) * 2;
                format!("{:.3}", s.tables.get(at).copied().unwrap_or(-1.0))
            }).collect();
            eprintln!(
                "freecam   sky fog table 96..127 {} step127 {:.4} proj z {} {} depth map {} {} depth range {:?}",
                fog.join(" "),
                s.tables.get((zakuro_gpu::native3d::FOG_TABLE + 127) * 2 + 1).copied().unwrap_or(0.0),
                view.proj[10],
                view.proj[11],
                f32::from_bits(s.shading.get(428).copied().unwrap_or(0)),
                f32::from_bits(s.shading.get(429).copied().unwrap_or(0)),
                s.depth_range
            );
        }
    }
}

/// the average colour of RGBA8 texels, RGB8 (0xBBGGRR).
fn average(rgba: &[u8]) -> u32 {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in rgba.chunks_exact(4).step_by(7) {
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
        n += 1;
    }
    let n = n.max(1);
    (r / n) as u32 | ((g / n) as u32) << 8 | ((b / n) as u32) << 16
}
