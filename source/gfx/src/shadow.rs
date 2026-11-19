//! gfx-shadow: the field shadow pass (ShadowScene) drawn natively, and the
//! shadow receivers' setup.
//!
//! How the game does it (USA SE v2, read from the disassembly):
//! - FieldShadow::Initialize (0x00198F34) makes a 256x256 RGBA8 ITexture,
//!   a ShadowScene (a SceneSystem whose nn::gr FrameBuffer at +0x2348 draws
//!   into that texture, sharing the main depth buffer), a ShadowGauss
//!   (+0x27C8) over the same texture, and patches the "ShadowMap" texture
//!   commands of the shadow binary (ReplaceShadowCommand) to name it.
//! - FieldShadow::Update fills the texture with the word 0x000000FF every
//!   frame (nngxAddMemoryFillCommand) and points camera 2 of the main
//!   scene's SceneState at the shadow camera (a projection and a view).
//! - FieldShadow::Draw puts the ShadowScene in the lower display's prim list.
//!   ShadowScene::OnBegin (0x001A4D10) binds the framebuffer, sets the
//!   viewport to +0x2388/+0x238C, draws each ShadowActor (+0x2394 count,
//!   +0x2398 list) through Model::DrawMesh, then ShadowGauss::Execute blurs
//!   the map in two passes. OnEnd binds the display's framebuffer again.
//! - A receiver is a model whose ModelState callback is FieldActorCallback
//!   (OnBegin 0x0020B664). With field shadows on (the byte at 0x006BE3C0),
//!   for a material whose unit 0 or 1 texture is named "ShadowMap..." it
//!   jumps into the shadow binary's command blob for that unit (the shadow
//!   texture), sets combiner stage 5 to `previous * (1 - texture)` and the
//!   unit's sampler, and turns that texture coordinate set into a camera
//!   projection (TextureMapState mapping 3, camera 1, see setup.rs).
//!
//! Natively: the pass's caster draws (DrawMesh, already native) go into an
//! offscreen target of the framebuffer's size; the blur runs on the result
//! (two 3-tap passes with bilinear taps, the game's weights and offsets);
//! the picture is kept as a *surface* keyed by the framebuffer's physical
//! address, in texture-memory row order, so any draw that samples that
//! address (the receivers) gets it instead of guest memory. Surfaces are
//! self-contained here so they can fold into gfx-post's framebuffer
//! surfaces (surfaces.rs) later.
//!
//! The emulated pass is skipped only when the native renderer draws the
//! whole lower display (its list compacted) and no receiver was drawn by
//! the emulated GPU the frame before; otherwise both draw it, so emulated
//! receivers (hybrid top screens) still find their shadows in memory.
//!
//! Env: FATES_GFX_SHADOW=0 turns this off; FATES_GFX_SHADOW_SCALE=n draws
//! the map at n times its size (default 1, the size parity is measured at);
//! FATES_GFX_SHADOW_DUMP=dir (with FATES_GFX_SHADOW_DUMP_EVERY, default 60)
//! writes native and emulated maps in shadow mode (see shadow_compare for
//! why the emulated one is not readable yet).

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;

use zakuro_core::System;

use crate::prim::{self, r16, r32, r8, Draw, TexRef};
use crate::render::{Decoded, Finish, Output, Start};
use crate::setup::{Objects, Setup};
use crate::Mode;

/// ShadowScene::OnBegin, the prim::Callback class of the shadow pass.
pub const ON_BEGIN: u32 = 0x001A_4D10;
/// FieldActorCallback::OnBegin, the receivers' ModelState callback.
pub const FIELD_ACTOR_ON_BEGIN: u32 = 0x0020_B664;

/// Literal-pool words of FieldActorCallback::OnBegin: the field-shadow
/// switch, the "Detail" and "ShadowMap" names, the border-colour headers.
const LIT_SHADOW_ON: u32 = 0x0020_BB28;
const LIT_DETAIL: u32 = 0x0020_BB2C;
const LIT_SHADOWMAP: u32 = 0x0020_BB30;
const LIT_BORDER0: u32 = 0x0020_BB38;
const LIT_BORDER1: u32 = 0x0020_BB3C;
/// Resource::GetShadowBinary's global, and the content name
/// FieldShadow::GetShadowMapContent looks up.
const LIT_SHADOW_BINARY: u32 = 0x004F_1360;
const LIT_CONTENT_NAME: u32 = 0x0019_9354;
/// ShadowGauss's constructor literals: the tap offsets in texels.
const LIT_GAUSS_OFFSETS: u32 = 0x001A_4C90;

/// The table overrides/gfx_shadow.c calls through, `fates_gfx_hooks_shadow`.
#[repr(C)]
pub struct Hooks {
    pub scene_begin: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    pub scene_end: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    pub gauss: Option<unsafe extern "C" fn(*mut c_void, u32) -> i32>,
    pub alter_buffer: Option<unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32)>,
    pub alter_callback: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    pub alter_clear: Option<unsafe extern "C" fn(*mut c_void)>,
    pub alter_execute: Option<unsafe extern "C" fn(*mut c_void, u32, u32)>,
}

pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_SHADOW").map_or(true, |v| v != "0") // gfx-perf: cached env
}

/// Fills the hook table (with the native renderer on).
pub fn install(hooks: &mut Hooks) {
    if crate::mode_from_env() == Mode::Off {
        return;
    }
    if enabled() {
        hooks.scene_begin = Some(hook_scene_begin);
        hooks.scene_end = Some(hook_scene_end);
        hooks.gauss = Some(hook_gauss);
    }
    hooks.alter_buffer = Some(crate::command_alter::hook_buffer);
    hooks.alter_callback = Some(crate::command_alter::hook_callback);
    hooks.alter_clear = Some(crate::command_alter::hook_clear);
    hooks.alter_execute = Some(crate::command_alter::hook_execute);
}

unsafe extern "C" fn hook_scene_begin(ctx: *mut c_void, scene: u32) {
    let system = unsafe { crate::system(ctx) };
    crate::with(|g| g.shadow_begin(system, scene));
}

unsafe extern "C" fn hook_scene_end(ctx: *mut c_void, scene: u32) {
    let system = unsafe { crate::system(ctx) };
    crate::with(|g| g.shadow_end(system, scene));
}

unsafe extern "C" fn hook_gauss(_ctx: *mut c_void, gauss: u32) -> i32 {
    crate::with(|g| g.shadow_gauss(gauss)).unwrap_or(0)
}

/// Where a ShadowScene draws.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub scene: u32,
    /// the FrameBuffer's colour buffer (as it holds it) and physical
    pub color: u32,
    pub color_phys: u32,
    /// PICA colour format (FrameBuffer +4)
    pub format: u32,
    /// the framebuffer's size (+0x40, +0x44) and the viewport OnBegin sets
    pub width: u32,
    pub height: u32,
    pub viewport: [u32; 2],
    pub casters: u32,
    /// the ShadowGauss and the texture it blurs
    pub gauss: u32,
    pub gauss_texture: Option<TexRef>,
}

impl Target {
    pub fn read(system: &mut System, scene: u32) -> Target {
        let fb = scene + 0x2348;
        let color = r32(system, fb);
        let gauss = r32(system, scene + 0x27C8);
        let gauss_texture = (gauss != 0).then(|| {
            let t = r32(system, gauss + 0x10);
            TexRef { addr: r32(system, t + 4), width: r16(system, t + 0xC) as u32, height: r16(system, t + 0xE) as u32, format: r8(system, t + 0x11) as u32 }
        });
        Target {
            scene,
            color,
            color_phys: phys(system, color),
            format: r8(system, fb + 4) as u32,
            width: r32(system, fb + 0x40),
            height: r32(system, fb + 0x44),
            viewport: [r32(system, scene + 0x2388), r32(system, scene + 0x238C)],
            casters: r32(system, scene + 0x2394),
            gauss,
            gauss_texture,
        }
    }
}

fn phys(system: &System, addr: u32) -> u32 {
    // already physical: VRAM, FCRAM
    if (0x1800_0000..0x1860_0000).contains(&addr) || (0x2000_0000..0x3000_0000).contains(&addr) {
        return addr;
    }
    zakuro_core::services::gsp::virtual_to_physical(system, addr)
}

/// an f32 as the PICA's float24.
fn f24(v: f32) -> u32 {
    if v == 0.0 {
        return 0;
    }
    let b = v.to_bits();
    let sign = b >> 31;
    let exp = ((b >> 23) & 0xFF) as i32 - 127 + 63;
    let mant = (b >> 7) & 0xFFFF;
    (sign << 23) | ((exp.clamp(0, 0x7F) as u32) << 16) | mant
}

/// The caster draws' target part: as the display's (setup::target: depth
/// map, scissor off), with OnBegin's viewport (Pica::Viewport(0, 0, w, h)).
pub fn target_setup(s: &mut Setup, t: &Target) {
    crate::setup::target(s, 1);
    s.set(0x041, f24(t.viewport[0] as f32 / 2.0));
    s.set(0x043, f24(t.viewport[1] as f32 / 2.0));
    s.set(0x068, 0);
}

/// One shadow pass as the native renderer records it.
pub struct Pass {
    pub target: Target,
    pub draws: Vec<Draw>,
    pub keys: Vec<[Option<u64>; 3]>,
    /// ShadowGauss::Execute ran
    pub gauss: bool,
    /// the native pass replaces the emulated one (casters and blur skipped)
    pub replace: bool,
    pub clear_depth: f32,
}

/// A rendered shadow map.
pub struct Surface {
    pub width: u32,
    pub height: u32,
    /// RGBA, rows in texture-memory order (as raster.frag's textures,
    /// decoded from guest memory without flipping)
    pub rgba: Arc<Decoded>,
    pub key: u64,
    pub frame: u64,
    pub casters: usize,
}

#[derive(Default)]
pub struct Shadows {
    pub current: Option<Pass>,
    /// by the colour buffer's physical address
    pub surfaces: HashMap<u32, Surface>,
    /// texture keys handed out for surfaces, oldest first (forgotten once
    /// no frame can still draw with them)
    handed: VecDeque<(TexRef, u64)>,
    /// surfaces waiting for the emulated picture (shadow mode)
    compare: Vec<u32>,
    /// a receiver was drawn by the emulated GPU, this frame and the last
    pub receivers_emulated: bool,
    pub receivers_emulated_last: bool,
    serial: u64,
    pub passes: u64,
    pub replaced: u64,
    pub casters: u64,
    pub compared: u64,
    pub diff_sum: f64,
    pub diff_worst: f64,
    dumped: u64,
    warned: bool,
}

impl Shadows {
    pub fn new() -> Shadows {
        // until a frame shows otherwise, receivers may be emulated
        Shadows { receivers_emulated_last: true, ..Default::default() }
    }

    /// once per rendered frame (Graphics::Render)
    pub fn frame(&mut self) {
        self.receivers_emulated_last = self.receivers_emulated;
        self.receivers_emulated = false;
    }
}

/// The colour a 32-bit memory fill of `word` gives a texel of `format`.
fn fill_color(word: u32, format: u32) -> [f32; 4] {
    let tile: Vec<u8> = std::iter::repeat_n(word.to_le_bytes(), 64).flatten().collect();
    let f = zakuro_gpu::texture::TextureFormat::from_raw(format);
    let t = zakuro_gpu::texture::sample_texel(&tile, f, 8, 0, 0);
    t.map(|c| c as f32 / 255.0)
}

/// the upright picture the renderer drew (h * s wide, w * s tall) in
/// texture-memory row order (w * s wide, h * s tall): turned a quarter,
/// framebuffer x running down the upright picture from its bottom (found
/// against the emulated picture: the plain transpose mirrors x).
fn memory_order(upright: &[u8], uw: usize, uh: usize) -> Vec<u8> {
    let (mw, mh) = (uh, uw);
    let mut out = vec![0u8; mw * mh * 4];
    for r in 0..mh {
        for c in 0..mw {
            let s = ((mw - 1 - c) * uw + r) * 4;
            let d = (r * mw + c) * 4;
            out[d..d + 4].copy_from_slice(&upright[s..s + 4]);
        }
    }
    out
}

/// ShadowGauss::Execute (0x00203F90) on a picture in window order (row 0
/// at framebuffer y = 0, the bottom): pass 1 writes green only
/// (TevSetGaussPass1(0x80, 0x53), colour mask 0 1 0 1), pass 2 writes all
/// (TevSetGaussPass2(0x40, 0x33): the stage 4 weight is 0x40 * 256 /
/// (0x40 + 0x33), stage 5's constant 0x99). The taps sample the shadow
/// texture itself, bilinear, clamped to a black border, at the offsets
/// ShadowGauss's constructor put on its two quads (in texels: pass 1 units
/// 1, 2 at (0.5, -1.5), (-0.5, 1.5); pass 2 units 0, 1 at (1.5, 0.5),
/// (-1.5, -0.5)); pass 2 reads the map as it was before it (a copy).
pub fn gauss(win: &mut [u8], w: usize, h: usize, texel: f32, offsets: [f32; 4]) {
    let sample = |img: &[u8], ch: usize, x: f32, y: f32| -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: i64, yi: i64| -> f32 {
            if xi < 0 || yi < 0 || xi >= w as i64 || yi >= h as i64 {
                return if ch == 3 { 1.0 } else { 0.0 };
            }
            img[(yi as usize * w + xi as usize) * 4 + ch] as f32 / 255.0
        };
        let (xi, yi) = (x0 as i64, y0 as i64);
        let top = at(xi, yi) * (1.0 - fx) + at(xi + 1, yi) * fx;
        let bot = at(xi, yi + 1) * (1.0 - fx) + at(xi + 1, yi + 1) * fx;
        top * (1.0 - fy) + bot * fy
    };
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // the constructor's literals: -1.5, 0.5, 1.5, -0.5
    let [a, b, c, d] = offsets;
    // pass 1: unit 0 at the texel, unit 1 at (b, a), unit 2 at (d, c)
    let src = win.to_vec();
    let c4 = 0x80 as f32 / 255.0;
    let c5 = 0x53 as f32 / 255.0;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let t0 = sample(&src, 0, fx, fy);
            let t1 = sample(&src, 0, fx + b * texel, fy + a * texel);
            let t2 = sample(&src, 0, fx + d * texel, fy + c * texel);
            let s4 = t1 * c4 + t0 * (1.0 - c4);
            let s5 = t2 * c5 + s4 * (1.0 - c5);
            let o = (y * w + x) * 4;
            win[o + 1] = q(s5);
            win[o + 3] = 255;
        }
    }
    // pass 2: unit 0 at (c, b), unit 1 at (a, d), unit 2's green at the texel
    let src = win.to_vec();
    let k = ((0x40u32 << 8) / (0x40 + 0x33)) as f32 / 255.0;
    let kc = 0x99 as f32 / 255.0;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let t0 = sample(&src, 0, fx + c * texel, fy + b * texel);
            let t1 = sample(&src, 0, fx + a * texel, fy + d * texel);
            let o = (y * w + x) * 4;
            let t2g = src[o + 1] as f32 / 255.0;
            let s4 = t1 * k + t0 * (1.0 - k);
            let v = q(s4 * kc + t2g * (1.0 - kc));
            win[o..o + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
}

impl crate::Gfx {
    /// ShadowScene::OnBegin, before the original.
    pub(crate) fn shadow_begin(&mut self, system: &mut System, scene: u32) {
        let target = Target::read(system, scene);
        let compacted = self.islands.as_ref().is_some_and(|i| i.compacted);
        let replace = self.mode == Mode::Native && compacted && !self.shadow.receivers_emulated_last;
        if !self.shadow.warned {
            self.shadow.warned = true;
            eprintln!("gfx-shadow: pass {target:X?}");
        }
        self.shadow.current = Some(Pass { target, draws: Vec::new(), keys: Vec::new(), gauss: false, replace, clear_depth: self.cleared.map_or(1.0, |c| c.1) });
    }

    /// a DrawMesh of the running shadow pass: drawn into the pass. None:
    /// not a caster.
    pub(crate) fn shadow_caster(&mut self, system: &mut System, model: u32, state: u32, layer: u32, mesh: u32, world: u32) -> Option<i32> {
        let target = self.shadow.current.as_ref()?.target;
        if r32(system, state + 0xC) != target.scene {
            return None;
        }
        let made = self.shade_draws(system, model, state, layer, mesh, world, 1, Some(&target));
        let pass = self.shadow.current.as_mut()?;
        for (d, k) in made {
            pass.draws.push(d);
            pass.keys.push(k);
        }
        self.shadow.casters += 1;
        Some(pass.replace as i32)
    }

    /// ShadowGauss::Execute: nonzero when the native pass replaces the
    /// emulated one.
    pub(crate) fn shadow_gauss(&mut self, _gauss: u32) -> i32 {
        match self.shadow.current.as_mut() {
            Some(pass) => {
                pass.gauss = true;
                pass.replace as i32
            }
            None => 0,
        }
    }

    /// ShadowScene::OnBegin returned: the pass drawn natively, its surface
    /// published for the receivers.
    pub(crate) fn shadow_end(&mut self, system: &mut System, _scene: u32) {
        let Some(pass) = self.shadow.current.take() else { return };
        let _t = crate::perf::scope(crate::perf::C::Shadow); // gfx-perf
        self.shadow.passes += 1;
        self.shadow.replaced += pass.replace as u64;
        let t = pass.target;
        if t.width == 0 || t.height == 0 || t.width > 1024 || t.height > 1024 {
            return;
        }
        let scale = std::env::var("FATES_GFX_SHADOW_SCALE").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).clamp(1, 8);
        // the pass starts from FieldShadow::Update's fill of the map
        let clear = fill_color(0x0000_00FF, t.format);
        // perf3: on the side queue, so the game's thread waits for this pass
        // only, not for the frames and pictures in between queued before it
        self.ensure_readback();
        let Some(renderer) = (if self.side.is_some() { self.side.as_mut() } else { self.renderer.as_mut() }) else { return };
        let (uw, uh) = (t.height * scale, t.width * scale);
        let t_render = crate::perf::scope(crate::perf::C::ShadowRender); // gfx-perf
        let upright = match renderer.render(2, uw, uh, scale as f32, Start::Clear(clear), pass.clear_depth, &pass.draws, &pass.keys, &self.decoded, Finish::Readback) {
            Ok(Output::Pixels(p)) => p,
            Ok(_) => return,
            Err(e) => {
                eprintln!("gfx-shadow: render failed, {e}");
                return;
            }
        };
        drop(t_render);
        let _t = crate::perf::scope(crate::perf::C::ShadowBlur); // gfx-perf
        let (mw, mh) = (uh as usize, uw as usize);
        let mut rgba = memory_order(&upright, uw as usize, uh as usize);
        if pass.gauss {
            // the blur works in window order (row 0 at the bottom)
            let row = mw * 4;
            let mut win = vec![0u8; rgba.len()];
            for r in 0..mh {
                win[(mh - 1 - r) * row..(mh - r) * row].copy_from_slice(&rgba[r * row..(r + 1) * row]);
            }
            let offsets: [f32; 4] = std::array::from_fn(|i| f32::from_bits(r32(system, zakuro_core::region::us(LIT_GAUSS_OFFSETS) + 4 * i as u32)));
            crate::blur::gauss(&mut win, mw, mh, scale as f32, offsets); // gfx-perf: the same, faster
            for r in 0..mh {
                rgba[r * row..(r + 1) * row].copy_from_slice(&win[(mh - 1 - r) * row..(mh - r) * row]);
            }
        }
        self.shadow.serial += 1;
        let key = 0x5AD0_0000_0000_0000 ^ self.shadow.serial;
        let surface = Surface {
            width: mw as u32,
            height: mh as u32,
            rgba: Arc::new(Decoded { width: mw as u32, height: mh as u32, rgba, levels: 1 }),
            key,
            frame: self.stats.frames,
            casters: pass.draws.len(),
        };
        self.shadow.surfaces.insert(t.color_phys, surface);
        if self.mode == Mode::Shadow && scale == 1 && std::env::var_os("FATES_GFX_SHADOW_COMPARE").is_some() {
            self.shadow.compare.push(t.color_phys);
        }
        // keys of surfaces a few frames old: no frame draws with them now
        while self.shadow.handed.len() > 16 {
            let Some(old) = self.shadow.handed.pop_front() else { break };
            self.decoded.remove(&old);
            if let Some(r) = self.renderer.as_mut() {
                r.forget_texture(old);
            }
            if let Some(r) = self.side.as_mut() {
                r.forget_texture(old); // perf3
            }
            // gfx-hfr / gfx-final: the presenter's device's renderers (frames
            // and pictures in between, per display)
            for r in self.gpu_renderers() {
                r.forget_texture(old);
            }
            self.hfr.forget_when_back(old); // rtpace: and those lent to the drawing thread
        }
    }

    /// a texture some draw samples: the native shadow map when its address
    /// is a surface's. `flip`: row 0 at t = 0 (the 2D layer) or at t = 1.
    pub(crate) fn shadow_texture(&mut self, system: &System, tex: TexRef, flip: bool) -> Option<u64> {
        if self.shadow.surfaces.is_empty() {
            return None;
        }
        let surface = self.shadow.surfaces.get(&phys(system, tex.addr))?;
        if tex.format != 0 {
            return None;
        }
        let key = surface.key ^ if flip { 0x00F1_1F00_F11F_0000 } else { 0 };
        if !self.decoded.contains_key(&(tex, key)) {
            let d = if flip {
                let row = surface.width as usize * 4;
                let h = surface.height as usize;
                let mut rgba = vec![0u8; surface.rgba.rgba.len()];
                for r in 0..h {
                    rgba[(h - 1 - r) * row..(h - r) * row].copy_from_slice(&surface.rgba.rgba[r * row..(r + 1) * row]);
                }
                Arc::new(Decoded { width: surface.width, height: surface.height, rgba, levels: 1 })
            } else {
                surface.rgba.clone()
            };
            self.decoded.insert((tex, key), d);
            self.shadow.handed.push_back((tex, key));
        }
        Some(key)
    }

    /// whether a DrawMesh the emulated GPU draws is a shadow receiver (so
    /// the emulated pass must still be drawn for it).
    pub(crate) fn shadow_note_emulated(&mut self, system: &mut System, model: u32, mesh: u32) {
        if self.shadow.receivers_emulated || self.shadow.surfaces.is_empty() {
            return;
        }
        let flags = r16(system, model + 8) as u32;
        if flags & 1 == 0 || flags & 0x20 != 0 {
            return;
        }
        let callback = r32(system, model + 4);
        let vtable = if callback != 0 { r32(system, callback) } else { 0 };
        if callback == 0 || zakuro_core::region::to_us(r32(system, vtable)) != FIELD_ACTOR_ON_BEGIN {
            return;
        }
        let index = r16(system, mesh) as u32;
        let res_model = r32(system, mesh + 0x2C);
        let material = r32(system, res_model + 0x34) + index * 0x2C;
        if receiver_unit(system, material).is_some() {
            self.shadow.receivers_emulated = true;
        }
    }

    /// shadow mode, FATES_GFX_SHADOW_COMPARE=1 (at the next frame) or
    /// =transfer (at the next display transfer): the native maps against
    /// what guest memory holds of the emulated ones. Today memory holds only
    /// FieldShadow's fill there: the emulated GPU keeps the drawn map on the
    /// host GPU (its receivers sample it there) and does not write it back
    /// on a sync over a range a fill superseded, so the maps are compared
    /// through the receivers' pixels instead (FATES_GFX_TEST_NOPOST).
    pub(crate) fn shadow_compare(&mut self, system: &mut System) {
        for addr in std::mem::take(&mut self.shadow.compare) {
            let Some(s) = self.shadow.surfaces.get(&addr) else { continue };
            let (w, h) = (s.width, s.height);
            let size = w * h * 4;
            let virt = prim::virt(system, addr);
            system.sync_gpu(virt, size); // gfx-bottom: by its virtual address
            let mut raw = vec![0u8; size as usize];
            system.memory.read_bytes(virt, &mut raw);
            let f = zakuro_gpu::texture::TextureFormat::Rgba8;
            let mut emu = vec![0u8; size as usize];
            for y in 0..h {
                for x in 0..w {
                    let o = ((y * w + x) * 4) as usize;
                    emu[o..o + 4].copy_from_slice(&zakuro_gpu::texture::sample_texel(&raw, f, w, x, y));
                }
            }
            let (mean, over) = crate::diff_stats(&s.rgba.rgba, &emu);
            self.shadow.compared += 1;
            self.shadow.diff_sum += mean;
            self.shadow.diff_worst = self.shadow.diff_worst.max(mean);
            let dark = |p: &[u8]| p.chunks_exact(4).filter(|c| c[0] > 8).count();
            eprintln!(
                "gfx-shadowmap frame {} casters {}: mean abs {mean:.3}, pixels off by >16: {:.3}%, shadowed native {} emulated {}",
                s.frame,
                s.casters,
                over * 100.0,
                dark(&s.rgba.rgba),
                dark(&emu)
            );
            if let Some(dir) = std::env::var_os("FATES_GFX_SHADOW_DUMP") {
                self.shadow.dumped += 1;
                let every = std::env::var("FATES_GFX_SHADOW_DUMP_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(60u64);
                if every <= 1 || self.shadow.dumped % every == 1 {
                    let dir = std::path::PathBuf::from(dir);
                    let _ = std::fs::create_dir_all(&dir);
                    // side by side, native left, opaque so it shows
                    let mut both = vec![0u8; (w * 2 * h * 4) as usize];
                    for y in 0..h as usize {
                        let row = w as usize * 4;
                        both[y * row * 2..y * row * 2 + row].copy_from_slice(&s.rgba.rgba[y * row..(y + 1) * row]);
                        both[y * row * 2 + row..(y + 1) * row * 2].copy_from_slice(&emu[y * row..(y + 1) * row]);
                    }
                    for p in both.chunks_exact_mut(4) {
                        p[3] = 255;
                    }
                    let _ = crate::write_png(&dir.join(format!("shadowmap-{:05}.png", s.frame)), &both, w * 2, h);
                }
            }
        }
    }
}

/// a NUL-terminated guest string starts with another.
fn starts_with(system: &mut System, name: u32, prefix: u32) -> bool {
    if name == 0 || prefix == 0 {
        return false;
    }
    for i in 0..256 {
        let p = r8(system, prefix + i);
        if p == 0 {
            return true;
        }
        if r8(system, name + i) != p {
            return false;
        }
    }
    false
}

fn cstr(system: &mut System, at: u32) -> Vec<u8> {
    let mut s = Vec::new();
    for i in 0..256 {
        let c = r8(system, at + i);
        if c == 0 {
            break;
        }
        s.push(c);
    }
    s
}

/// The texture unit (0 or 1) whose texture FieldActorCallback turns into
/// the shadow map, with field shadows on: unit 0 when its texture is named
/// "ShadowMap...", else unit 1 when its is.
pub fn receiver_unit(system: &mut System, material: u32) -> Option<u32> {
    let on = r32(system, zakuro_core::region::us(LIT_SHADOW_ON));
    if r8(system, on) != 1 {
        return None;
    }
    let name = r32(system, zakuro_core::region::us(LIT_SHADOWMAP));
    let tex0 = r32(system, material + 4);
    let name0 = if tex0 != 0 { r32(system, tex0 + 0x1C) } else { 0 };
    if tex0 != 0 && starts_with(system, name0, name) {
        return Some(0);
    }
    let tex1 = r32(system, material + 8);
    let name1 = if tex1 != 0 { r32(system, tex1 + 0x1C) } else { 0 };
    (tex1 != 0 && starts_with(system, name1, name)).then_some(1)
}

/// nw::h3d::fnd::PatriciaDic::FindIndex (0x0011D530).
fn patricia(system: &mut System, dic: u32, key: &[u8]) -> Option<u32> {
    if r16(system, dic) == 0 {
        return None;
    }
    let nodes = r32(system, dic + 4);
    let node = |i: u32| nodes + i * 12;
    let mut cur = node(r16(system, nodes + 4) as u32);
    if r32(system, cur) < r32(system, nodes) {
        let mut p = cur;
        for _ in 0..64 {
            let bit = r32(system, p);
            let byte = (bit >> 3) as usize;
            let right = byte < key.len() && (key[byte] >> (bit & 7)) & 1 != 0;
            let next = if right { r16(system, p + 6) } else { r16(system, p + 4) } as u32;
            cur = node(next);
            if r32(system, cur) >= bit {
                break;
            }
            p = cur;
        }
    }
    let name = r32(system, cur + 8);
    (name != 0 && cstr(system, name) == key).then(|| (cur - nodes - 12) / 12)
}

/// FieldShadow::GetShadowMapContent (0x001992FC): the shadow binary's
/// "ShadowMap" content, two (blob, words) pairs for units 0 and 1.
fn shadow_map_content(system: &mut System) -> Option<u32> {
    let holder = r32(system, zakuro_core::region::us(LIT_SHADOW_BINARY));
    let bin = r32(system, holder + 4);
    if bin == 0 {
        return None;
    }
    let root = r32(system, bin + 0x80 + 0x30);
    let at = r32(system, zakuro_core::region::us(LIT_CONTENT_NAME));
    let name = cstr(system, at);
    let index = patricia(system, root + 0x28, &name)?;
    let table = r32(system, root + 0x24);
    Some(r32(system, table + index * 4))
}

/// nw::h3d::res::TextureSamplerData::MakeUnit{0,1,2}Command's parameter
/// word for the unit (the sampler's type, wraps, filters).
fn sampler_param(b: &[u8; 16], etc1: bool) -> u32 {
    (matches!(b[4], 3..=5) as u32) << 2
        | (b[3] as u32) << 1
        | (if etc1 { 2 } else { 0 }) << 4
        | (b[2] as u32) << 8
        | (b[1] as u32) << 12
        | ((b[0] == 2 || b[0] == 4) as u32) << 20
        | ((b[4] == 2 || b[4] == 5) as u32) << 24
        | (b[0] as u32) << 28
}

const UNIT_PARAM: [u32; 3] = [0x083, 0x093, 0x09B];

/// FATES_GFX_SHADOW_RECEIVERS=0: the receivers' callback left out of the
/// native setup (an A/B check).
fn receivers_ported() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_GFX_SHADOW_RECEIVERS").map_or(true, |v| v != "0"))
}

/// FieldActorCallback::OnBegin (0x0020B664), what it adds to a receiver's
/// commands (ModelState::BeginCommand runs it last).
pub fn field_actor_words(w: &mut Vec<u32>, system: &mut System, o: &Objects) {
    if !receivers_ported() {
        return;
    }
    let mat = o.material;
    let on = r32(system, zakuro_core::region::us(LIT_SHADOW_ON));
    if r8(system, on) != 1 {
        // shadows off: "Detail..." textures with mipmaps get their sampler
        // again with the minimum level at the last one
        let detail = r32(system, zakuro_core::region::us(LIT_DETAIL));
        for unit in 0..3u32 {
            let tex = r32(system, mat + 4 + 4 * unit);
            if tex == 0 {
                continue;
            }
            let levels = r8(system, tex + 0x19);
            let tname = r32(system, tex + 0x1C);
            if levels == 0 || !starts_with(system, tname, detail) {
                continue;
            }
            let mut b = [0u8; 16];
            let samplers = r32(system, mat + 0x18);
            prim::rd(system, samplers + unit * 0x10, &mut b);
            b[5] = levels - 1;
            let etc1 = r8(system, tex + 0x18) == 0xC;
            // gfx-terrain: MakeUnit{0,1,2}Command's words, the level of
            // detail (with the levels: min = max = the last) included
            w.extend(crate::setup::sampler_unit_words(system, &b, unit as usize, etc1, levels - 1));
        }
        return;
    }
    let Some(unit) = receiver_unit(system, mat) else { return };
    if let Some(content) = shadow_map_content(system) {
        let (addr, words) = (r32(system, content + unit * 8), r32(system, content + unit * 8 + 4));
        if addr != 0 && words != 0 && words < 0x400 {
            let mut raw = vec![0u8; words as usize * 4];
            prim::rd(system, addr, &mut raw);
            w.extend(raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
        }
    }
    // stage 5: previous times one minus the texture's colour, alpha kept
    // (Stage(5), SetupPrevious, then the callback's bytes)
    let mut p = [0u8; 0x18];
    p[4..7].copy_from_slice(&[0xF, if unit == 1 { 4 } else { 3 }, 0xE]);
    p[0xD..0x10].copy_from_slice(&[0xF, 0xE, 0xE]);
    p[0] = 1;
    p[2] = 1;
    p[0x16..0x18].copy_from_slice(&0xF8u16.to_le_bytes());
    crate::setup::stage_words(w, &p);
    // the unit's sampler: the material's sampler type, clamp to border,
    // linear, and a black opaque border
    let mut b = [0u8; 16];
    let samplers = r32(system, mat + 0x18);
    b[0] = r8(system, samplers + unit * 0x10);
    b[1..6].copy_from_slice(&[1, 1, 1, 5, 0]);
    // its level-of-detail word: no bias, and the levels from a register
    // the call leaves as it was, 0 where the oracle looked
    // gfx-terrain: the unit's own parameter layout (MakeUnit1Command has
    // no type bits; the oracle showed bit 28 set on unit 1)
    let param = if unit == 0 { sampler_param(&b, false) } else { crate::setup::sampler_unit_words(system, &b, unit as usize, false, 0)[0] };
    w.extend([param, 0x000F_0000 | UNIT_PARAM[unit as usize], 0, 0x000F_0001 + UNIT_PARAM[unit as usize]]);
    let header = r32(system, if unit == 0 { LIT_BORDER0 } else { LIT_BORDER1 });
    w.extend([0xFF00_0000, header]);
}

/// FieldActorCallback's TextureMapState change for a receiver's shadow
/// coordinate set: camera projection (mapping 3) from camera 1, scale 1,
/// no rotation or translation, recomputed every frame. The game writes it
/// into the MaterialState after the draw, so it takes effect from the next
/// frame on; drawn natively, DrawMesh never runs, so it is applied here.
/// Returns the coordinate set, its bytes +0x94..+0x97 (flags, SRT mode,
/// mapping, camera) and its scale, rotation and translation.
pub fn receiver_mapping(system: &mut System, o: &Objects) -> Option<(u32, [u8; 4], [f32; 5])> {
    if !receivers_ported() {
        return None;
    }
    let model_flags = r16(system, o.model + 8) as u32;
    if model_flags & 1 == 0 || model_flags & 0x20 != 0 {
        return None;
    }
    let callback = r32(system, o.model + 4);
    let vtable = if callback != 0 { r32(system, callback) } else { 0 };
        if callback == 0 || zakuro_core::region::to_us(r32(system, vtable)) != FIELD_ACTOR_ON_BEGIN {
        return None;
    }
    let unit = receiver_unit(system, o.material)?;
    let one = f32::from_bits(r32(system, zakuro_core::region::us(0x0020_BB40)));
    let zero = f32::from_bits(r32(system, zakuro_core::region::us(0x0020_BB34)));
    Some((unit, [1, 0, 3, 1], [one, one, zero, zero, zero]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn f24_matches_the_pica() {
        assert_eq!(super::f24(120.0), 0x45_E000);
        assert_eq!(super::f24(200.0), 0x46_9000);
        assert_eq!(super::f24(128.0), 0x46_0000);
    }

    #[test]
    fn gauss_keeps_an_empty_map_empty() {
        let mut win = vec![0u8; 16 * 16 * 4];
        for p in win.chunks_exact_mut(4) {
            p[3] = 255;
        }
        super::gauss(&mut win, 16, 16, 1.0, [-1.5, 0.5, 1.5, -0.5]);
        assert!(win.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }
}

/// PrimPostEffectCallback::OnBegin (gfx-post's area).
const POST_EFFECT_ON_BEGIN: u32 = 0x0021_E220;

/// FATES_GFX_TEST_NOPOST=1, a test aid: the post-effect states taken out of
/// the display's list before either path sees it, so the rest of a frame
/// that has one (the avatar creator's top screen: bloom over the 3D) can be
/// compared native against emulated in one run. Not for play.
pub fn test_drop_post(system: &mut System, prim_ctx: u32, disp: u32) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("FATES_GFX_TEST_NOPOST").is_ok_and(|v| v == "1")) {
        return;
    }
    let states = prim::state_list(system, prim_ctx, disp);
    let kept: Vec<u32> = states
        .iter()
        .copied()
        .filter(|&s| {
            let (_, callback) = prim::state_callback(system, prim_ctx, s);
            callback == 0 || {
                let vtable = r32(system, callback);
                zakuro_core::region::to_us(r32(system, vtable)) != POST_EFFECT_ON_BEGIN
            }
        })
        .collect();
    if kept.len() == states.len() {
        return;
    }
    let list = prim_ctx + 0x1878 + disp * 12;
    let begin = r32(system, list);
    for (k, &s) in kept.iter().enumerate() {
        prim::w32(system, begin + 4 * k as u32, s);
    }
    prim::w32(system, list + 4, begin + 4 * kept.len() as u32);
}
