//! gfx-terrain: 2D drawn inside a 3D scene pass, natively.
//!
//! Battle maps draw their unit sprites, HP gauges, icons, panels and the
//! cursor (map::UnitRender, IconRender, PanelRender, GridRender,
//! GameCursor) from the scene's own node callbacks, between its meshes,
//! through the prim layer's "command mode": prim::Context::BeginCommand
//! (0x00415F84) remembers where the state vector (prim context +0x2C, 24
//! bytes a state) ends (+0x28) and switches the RenderState to mode 0;
//! the prim calls append states; prim::Context::EndCommand (0x004159A4)
//! renders the states appended since, in order, exactly as RenderTarget
//! renders a display list (UpdateState, the callback's OnBegin,
//! RenderCall, OnEnd), between ResetCombiner calls, into the scene's
//! command list.
//!
//! Natively, EndCommand's states are decoded with the 2D layer's decoder
//! (prim.rs) at the moment it runs and go into the scene island's draws in
//! call order, between the meshes, so they depth-test against the scene's
//! depth like the emulated draws. The original always runs (the states
//! need its bookkeeping); in a frame drawn natively in full its PICA words
//! only reach the emulated framebuffer, which is not shown.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use zakuro_core::System;

use crate::prim::{self, Decoded as PrimDecoded};
use crate::{system, with, Gfx};

/// The table overrides/gfx_scene2d.c calls through,
/// `fates_gfx_hooks_scene2d` there.
#[repr(C)]
pub struct Hooks {
    /// prim::Context::BeginCommand(context, RenderState&), before it runs
    pub begin: Option<unsafe extern "C" fn(*mut c_void, u32, u32)>,
    /// prim::Context::EndCommand(context), before it runs
    pub end: Option<unsafe extern "C" fn(*mut c_void, u32)>,
}

/// the RenderState the running command mode draws for
static RENDER_STATE: AtomicU32 = AtomicU32::new(0);

/// With the native renderer and its 3D path on, and FATES_GFX_SCENE2D not 0.
pub fn install(hooks: &mut Hooks) {
    if crate::mode_from_env() == crate::Mode::Off || std::env::var("FATES_GFX_SCENE2D").is_ok_and(|v| v == "0") {
        return;
    }
    hooks.begin = Some(hook_begin);
    hooks.end = Some(hook_end);
}

unsafe extern "C" fn hook_begin(_ctx: *mut c_void, prim_ctx: u32, render_state: u32) {
    let _ = prim_ctx;
    RENDER_STATE.store(render_state, Ordering::Relaxed);
}

unsafe extern "C" fn hook_end(ctx: *mut c_void, prim_ctx: u32) {
    let system = unsafe { system(ctx) };
    let state = RENDER_STATE.swap(0, Ordering::Relaxed);
    with(|g| g.scene2d_end(system, prim_ctx, state));
}

/// mapsprites: whether a texture the map draws in its 3D scene is pixel
/// art, sampled sharp at every zoom (render.rs). GameCursor's hand (a
/// 32x32 ETC1A4 billboard, 40 map units wide) is drawn linear at every
/// camera distance, which at 8x is a bilinear blow-up; the other textured
/// 2D here are the move, attack and danger panels (I/A masks, 16x16 to
/// 256x256, tinted and stretched over whole tiles), which are smooth on
/// purpose and stay as the game asks. So: a colour texture (not one of the
/// intensity/alpha formats 5-11), not repeated, at most 128 texels a side.
/// `FATES_GFX_SHARP_SPRITES=0` leaves them all as the game asks.
pub fn pixel_art(t: &prim::TexRef, s: &prim::Sampling) -> bool {
    !(5..=11).contains(&t.format) && s.wrap < 2 && s.wrap_t < 2 && t.width <= 128 && t.height <= 128 && crate::perf::var("FATES_GFX_SHARP_SPRITES").map_or(true, |v| v != "0")
}

/// counts for FATES_GFX_REPORT-style tracing (FATES_GFX_SCENE2D_TRACE)
static TRACED: AtomicU32 = AtomicU32::new(0);

impl Gfx {
    /// EndCommand is about to render the states appended since
    /// BeginCommand: decoded into the scene island's draws when the scene
    /// is one this frame draws natively.
    fn scene2d_end(&mut self, system: &mut System, prim_ctx: u32, render_state: u32) {
        let from = prim::r32(system, prim_ctx + 0x28) as i32;
        if from < 0 || render_state == 0 {
            return;
        }
        let scene = prim::r32(system, render_state + 0xC);
        let Some(disp) = self.islands.as_ref().filter(|i| i.slots.iter().any(|(o, _)| *o == scene)).map(|i| i.disp) else {
            // gfx-gaps: dev, a command mode outside the native scene islands
            if crate::perf::var_os("FATES_GFX_SCENE2D_MISSED").is_some() {
                let n = (prim::r32(system, prim_ctx + 0x30).wrapping_sub(prim::r32(system, prim_ctx + 0x2C)) / 0x18).saturating_sub(from as u32);
                eprintln!("gfx-scene2d missed: scene {scene:08X} render state {render_state:08X} states {n} islands {:?} frame {}", self.islands.as_ref().map(|i| i.slots.clone()), self.stats.frames);
            }
            return;
        };
        let tables = *self.tables.get_or_insert_with(|| prim::Tables::read(system));
        let (start, end) = (prim::r32(system, prim_ctx + 0x2C), prim::r32(system, prim_ctx + 0x30));
        let count = end.wrapping_sub(start) / 0x18;
        let mut made = Vec::new();
        // gfx-gaps: UnitIcon::BatchCallback states: where their draws go
        // (unit_icon.rs records them while EndCommand runs them)
        self.unit_pending.clear();
        let base = self.islands.as_ref().and_then(|i| i.draws.get(&scene)).map_or(0, |v| v.len());
        let trace = crate::perf::var_os("FATES_GFX_SCENE2D_TRACE").is_some_and(|v| v.to_str().and_then(|v| v.parse::<u64>().ok()).is_none_or(|n| self.stats.frames >= n)) && count > from as u32 && TRACED.fetch_add(1, Ordering::Relaxed) < 40; // gfx-perf: cached env
        for i in from as u32..count.min(from as u32 + 4096) {
            let state = start + i * 0x18;
            let decoded = prim::decode(system, &tables, prim_ctx, disp as u32, state);
            if trace {
                let w: Vec<String> = (0..6).map(|k| format!("{:08X}", prim::r32(system, state + 4 * k))).collect();
                let kind = match &decoded {
                    PrimDecoded::Draw(_) => "draw".to_string(),
                    PrimDecoded::Skip => "skip".to_string(),
                    PrimDecoded::Island(i) => format!("island {i:?}"),
                };
                eprintln!("gfx-scene2d   state {i}: {} {kind}", w.join(" "));
            }
            if let PrimDecoded::Island(prim::Island::Callback(crate::unit_icon::BATCH_ON_BEGIN)) = decoded {
                self.unit_pending.push_back(crate::UnitSlot { scene, at: base + made.len(), state, prim_ctx, disp: disp as u32 });
            }
            if let PrimDecoded::Draw(mut d) = decoded {
                // mapsprites: the map's pixel art (the cursor hand) sampled sharp
                if let Some((t, s)) = d.textures[0].as_mut() {
                    s.sharp = pixel_art(t, s);
                }
                // mapsprites: dev, each texture the map's 2D draws use, once
                if let Some((t, s)) = d.textures[0].filter(|_| crate::perf::var_os("FATES_GFX_SCENE2D_TEXLOG").is_some()) {
                    static SEEN: std::sync::Mutex<Vec<(u32, u32, u32, u32, bool)>> = std::sync::Mutex::new(Vec::new());
                    let k = (t.addr, t.width, t.height, t.format, s.linear_mag);
                    let mut seen = SEEN.lock().unwrap();
                    if !seen.contains(&k) {
                        seen.push(k);
                        let v: Vec<[f32; 3]> = d.vertices.chunks_exact(24).take(4).map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
                        eprintln!("gfx-scene2d texture {:08X} {}x{} format {} {:?} frame {} n {} verts {:?}", t.addr, t.width, t.height, t.format, s, self.stats.frames, d.indices.len(), v);
                    }
                }
                made.push((d, [None; 3]));
            }
        }
        if trace {
            eprintln!("gfx-scene2d disp {disp} scene {scene:08X} states {}..{count}: {} draws", from, made.len());
            for (d, _) in &made {
                eprintln!(
                    "gfx-scene2d   tex {:X?} n {} depth {} {} {} blend {:?} vp {:?}",
                    d.textures.map(|t| t.map(|t| (t.0.addr, t.0.width, t.0.height, t.0.format, t.1.linear_mag as u8, t.1.sharp as u8))), // mapsprites: + mag, sharp
                    d.indices.len(),
                    d.depth_test,
                    d.depth_func,
                    d.depth_write,
                    d.blend,
                    d.viewport
                );
                // gfx-gaps: the first vertices and indices
                let v: Vec<[f32; 3]> = d.vertices.chunks_exact(24).take(8).map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
                eprintln!("gfx-scene2d     verts {} {:?} idx {:?} world {:?}", d.vertices.len() / 24, v, &d.indices[..d.indices.len().min(12)], d.world);
            }
        }
        if let Some(islands) = self.islands.as_mut() {
            islands.draws.entry(scene).or_default().extend(made);
        }
    }
}
