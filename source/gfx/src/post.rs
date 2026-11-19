//! gfx-post: post effects drawn natively.
//!
//! Post effects are not prim states. A `PrimPostEffectCallback` in a
//! display's state list runs its effect object (in the avatar creator
//! `BloomContext::Draw`, 0x001AB0BC), and that code drives the GPU directly
//! through the engine's `PostEffectContext` (InitBeforeDraw, TextureAssign,
//! DrawQuads), the `Pica::` register helpers (TexEnvSrc/Operand/Combine/
//! Const/Scale/BufferInput, TextureSamplerType, TextureSetting, Viewport,
//! ScissorTest, Blend, BlendFunc, AlphaTest, DepthTest, ColorMask,
//! UniformMatrix), nn::gr's `FrameBuffer::MakeCommand` (the render target),
//! and nngx copies run from a `CommandAlter` callback
//! (`BloomCaptureCallback::OnReplace`: nngxFilterBlockImage,
//! nngxAddBlockImageCopyCommand, nngxAddMemoryFillCommand).
//!
//! overrides/gfx_post.c hands every one of those calls to this module while
//! a post-effect island runs, with its arguments, in call order, which is
//! the order the GPU runs them in. The [`Recorder`] keeps the state the
//! helpers set (as the PICA would hold it) and turns each DrawQuads into a
//! native draw and each copy into surface [`Action`]s (surfaces.rs). The
//! result is a [`Block`], which goes into the display's native frame at the
//! callback's place, and the renderer (render/post.rs) runs it there: the
//! frame so far is the display target, which the block reads as the
//! game's colour buffer and draws over.
//!
//! The game's own code still runs in full (the emulated GPU gets the same
//! commands), so nothing the engine expects changes.
//!
//! The interface for other islands (gfx-shadow's ShadowScene, which uses the
//! same PostEffectContext/Pica:: calls through ShadowGauss::Execute): start
//! a recording with [`State::begin`], let the game's code run, take the
//! block with [`State::end`]. The C side is one generic entry,
//! `gfx_post_observe(ctx, function address)`, called at the entry of each
//! observed function; [`Recorder::call`] decodes the arguments by address.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use zakuro_core::System;

use crate::prim::{r16, r32, r8, Combiner, Sampling, Stage, TexRef};
use crate::surfaces::{Action, Desc, Display, Surfaces};

/// The table overrides/gfx_post.c calls through, `fates_gfx_hooks_post`.
#[repr(C)]
pub struct Hooks {
    pub recording: i32,
    pub begin: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    pub end: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    pub call: Option<unsafe extern "C" fn(*mut c_void, u32, *const u32, u32)>,
}

static HOOKS: AtomicPtr<Hooks> = AtomicPtr::new(std::ptr::null_mut());

/// PrimPostEffectCallback::OnBegin: the class of the post-effect islands.
pub const POST_ON_BEGIN: u32 = 0x0021_E220;
/// BloomContext::Draw, the effect run the native layer is verified on.
pub const BLOOM_DRAW: u32 = 0x001A_B0BC;

/// FATES_GFX_POST: 0 keeps post-effect islands emulated; otherwise they are
/// drawn natively whenever the native 3D path (FATES_GFX_NATIVE3D=1) draws
/// the rest of their frame.
pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_POST").map_or(true, |v| v != "0") // gfx-perf: cached env
}

/// the address the C table is at, for setting `recording`
pub fn install(hooks: &mut Hooks, begin: unsafe extern "C" fn(*mut c_void, u32), end: unsafe extern "C" fn(*mut c_void, u32), call: unsafe extern "C" fn(*mut c_void, u32, *const u32, u32)) {
    hooks.begin = Some(begin);
    hooks.end = Some(end);
    hooks.call = Some(call);
    HOOKS.store(hooks as *mut Hooks, Ordering::Release);
}

// gfx-gaps: pub(crate), unit_icon.rs observes the same calls
pub(crate) fn set_recording(on: bool) {
    let p = HOOKS.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: the table is a static the C side reads; one word
        unsafe { std::ptr::write_volatile(&mut (*p).recording, on as i32) };
    }
}

/// whether a post-effect callback object's effect is one the native layer
/// draws: BloomContext::Draw without depth of field (which reads the depth
/// buffer as a texture, not done natively yet)
pub fn supported(system: &mut System, callback: u32) -> bool {
    let effect = r32(system, callback + 4);
    if effect == 0 {
        return false;
    }
    let vtable = r32(system, effect);
    let run = zakuro_core::region::to_us(r32(system, vtable + 8)); // eur
    if run != BLOOM_DRAW {
        return false;
    }
    // BloomContext's settings (literal 0x001AB580): +0x14 depth of field
    let settings = r32(system, zakuro_core::region::us(0x001A_B580));
    r8(system, settings + 0x14) == 0
}

fn phys(system: &mut System, virt: u32) -> u32 {
    zakuro_core::services::gsp::virtual_to_physical(system, virt)
}

/// Where a post draw goes.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// the display's own colour buffer: the renderer's upright target
    Display,
    /// a surface; the framebuffer starts `row` rows into it and is `rows`
    /// rows tall (window y runs up from its last row)
    Surface { desc: Desc, row: u32, rows: u32 },
}

/// A texture unit of a post draw.
#[derive(Clone, Copy, Debug)]
pub enum Unit {
    /// disabled (samples as zero)
    Off,
    /// a surface: the texture is `width` x `height` from row `row` on
    Surface { desc: Desc, row: u32, width: u32, height: u32, sampling: Sampling },
    /// a texture in guest memory, decoded as the 2D layer decodes them
    /// (row 0 at t = 0); `key` is its content hash once read
    Guest { tex: TexRef, key: Option<u64>, sampling: Sampling },
}

/// One PostEffectContext::DrawQuads, as the PICA would draw it.
#[derive(Clone, Debug)]
pub struct PostDraw {
    pub target: Target,
    /// tex3.shbin's 40-byte vertices: position xyz, three texture
    /// coordinates, RGBA8 colour
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    /// uViewProj (c0..c3), rows
    pub matrix: [f32; 16],
    /// window rectangles of the framebuffer (x, y, w, h), y up
    pub viewport: [i32; 4],
    pub scissor: Option<[i32; 4]>,
    pub combiner: Combiner,
    /// the PICA blend function register (equations, factors)
    pub blend: u32,
    pub blend_color: [u8; 4],
    /// red, green, blue, alpha writes (bits 0..3)
    pub mask: u32,
    pub alpha_test: bool,
    pub alpha_func: u8,
    pub alpha_ref: u8,
    pub units: [Unit; 3],
    /// texture unit 2 samples coordinate 1 (TextureSamplerType)
    pub unit2_uv1: bool,
}

#[derive(Clone, Debug)]
pub enum Op {
    Act(Action),
    Draw(PostDraw),
    /// CommandAlter::AddCallback: an object the game registers to run
    /// again when this command range is copied (the right eye). The
    /// native renderer has no right eye; the call is kept for the record.
    Callback(u32),
}

/// A recorded post effect.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub disp: u8,
    pub ops: Vec<Op>,
    /// why the native picture of it is not complete, if it is not
    pub unsupported: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TexSet {
    addr: u32,
    width: u32,
    height: u32,
    format: u32,
    param: u32,
}

#[derive(Clone, Copy, Debug)]
struct Framebuffer {
    addr: u32,
    width: u32,
    rows: u32,
    format: u32,
}

/// The PICA state the helpers set, in their terms.
#[derive(Clone, Copy, Debug)]
struct Regs {
    combiner: Combiner,
    /// texture config (register 0x080)
    config: u32,
    tex: [TexSet; 3],
    viewport: [i32; 4],
    scissor: Option<[i32; 4]>,
    blend: u32,
    blend_color: [u8; 4],
    alpha: u32,
    depth: u32,
    mask: u32,
    matrix: [f32; 16],
    fb: Option<Framebuffer>,
}

impl Regs {
    fn new() -> Regs {
        Regs {
            // prim::Context::ResetCombiner's combiner, which the prim layer
            // leaves; the stages a post effect reads it sets itself
            combiner: Combiner::prim_reset(),
            config: 0,
            tex: [TexSet::default(); 3],
            viewport: [0, 0, 240, 400],
            scissor: None,
            blend: 0x0101_0000,
            blend_color: [0xFF; 4],
            alpha: 0x10,
            depth: 0,
            mask: 0xF,
            matrix: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.],
            fb: None,
        }
    }
}

/// Records one post effect.
pub struct Recorder {
    regs: Regs,
    block: Block,
    display: Display,
    /// the PostEffectContext::DrawQuads whose Pica::Draw is expected
    expect_draw: u32,
    /// FATES_GFX_POST_DUMP=dir: each draw's state, for dev/gfx-post/oracle.py
    pub dumps: Vec<String>,
}

/// GL formats nngxFilterBlockImage takes, as PICA colour formats.
fn gl_format(gl: u32) -> Option<u32> {
    match gl {
        0x8058 | 0x1908 | 0x6752 => Some(0), // RGBA8
        0x8051 | 0x1907 | 0x6754 => Some(1), // RGB8
        0x8057 => Some(2),                   // RGB5_A1
        0x8D62 => Some(3),                   // RGB565
        0x8056 => Some(4),                   // RGBA4
        _ => None,
    }
}

fn sampling(param: u32) -> Sampling {
    Sampling { wrap: ((param >> 12) & 3) as u8, wrap_t: ((param >> 8) & 3) as u8, sharp: false, linear_mag: param & 2 != 0, linear_min: param & 4 != 0 }
}

impl Recorder {
    fn new(disp: u8, display: Display) -> Recorder {
        Recorder { regs: Regs::new(), block: Block { disp, ..Block::default() }, display, expect_draw: 0, dumps: Vec::new() }
    }

    fn unsupported(&mut self, why: String) {
        if crate::perf::var_os("FATES_GFX_DRAW_TRACE").is_some() {
            eprintln!("gfx-draw-trace unsupported frame {} {why}", zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed)); // gfx-lines: dev
        }
        if !self.block.unsupported.contains(&why) {
            self.block.unsupported.push(why);
        }
    }

    /// one observed call (`function` is its address)
    pub fn call(&mut self, system: &mut System, surfaces: &mut Surfaces, function: u32, a: &[u32]) {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0);
        let r = &mut self.regs;
        match function {
            // Pica::TexEnvSrc(stage, rgb0..2, alpha0..2)
            0x0012_6640 => {
                if let Some(s) = r.combiner.stages.get_mut(arg(0) as usize) {
                    s.src_rgb = [arg(1), arg(2), arg(3)].map(|v| v as u8 & 0xF);
                    s.src_a = [arg(4), arg(5), arg(6)].map(|v| v as u8 & 0xF);
                }
            }
            // Pica::TexEnvOperand(stage, rgb0..2, alpha0..2)
            0x0012_FFA8 => {
                if let Some(s) = r.combiner.stages.get_mut(arg(0) as usize) {
                    s.op_rgb = [arg(1), arg(2), arg(3)].map(|v| v as u8 & 0xF);
                    s.op_a = [arg(4), arg(5), arg(6)].map(|v| v as u8 & 0x7);
                }
            }
            // Pica::TexEnvCombine(stage, rgb, alpha)
            0x0012_FF68 => {
                if let Some(s) = r.combiner.stages.get_mut(arg(0) as usize) {
                    s.comb_rgb = arg(1) as u8 & 0xF;
                    s.comb_a = arg(2) as u8 & 0xF;
                }
            }
            // Pica::TexEnvConst(stage, r, g, b, a)
            0x003D_2214 => {
                if let Some(s) = r.combiner.stages.get_mut(arg(0) as usize) {
                    s.konst = [arg(1), arg(2), arg(3), arg(4)].map(|v| v as u8);
                }
            }
            // Pica::TexEnvScale(stage, rgb, alpha)
            0x0012_6060 => {
                if let Some(s) = r.combiner.stages.get_mut(arg(0) as usize) {
                    s.scale_rgb = arg(1) as u8 & 3;
                    s.scale_a = arg(2) as u8 & 3;
                }
            }
            // Pica::TexEnvBufferInput(rgb0, a0, rgb1, a1, rgb2, a2, rgb3, a3)
            0x0013_0098 => {
                let b = |i: usize| (arg(i) != 0) as u8;
                r.combiner.update_rgb = b(0) | b(2) << 1 | b(4) << 2 | b(6) << 3;
                r.combiner.update_a = b(1) | b(3) << 1 | b(5) << 2 | b(7) << 3;
            }
            // Pica::TextureSamplerType(t0 type, t1, t2, t3, t2 coordinate, t3 coordinate)
            0x0013_00F0 => {
                r.config = (arg(0) != 5) as u32 | ((arg(1) != 0) as u32) << 1 | ((arg(2) != 0) as u32) << 2 | arg(5) << 8 | ((arg(3) != 0) as u32) << 10 | ((arg(4) != 0) as u32) << 13 | 0x1000;
            }
            // Pica::TextureSetting(unit, address, width, height, type, format,
            // wrap s, wrap t, min, mag, ...), the parameter word as it builds it
            0x0013_7718 => {
                let unit = arg(0) as usize;
                if unit < 3 {
                    let min = arg(8);
                    let mip_linear = matches!(min, 3..=5) as u32;
                    let param = mip_linear << 2 | (arg(9) & 1) << 1 | ((arg(5) == 0xC) as u32 * 2) << 4 | (arg(7) & 7) << 8 | (arg(6) & 7) << 12 | (arg(13) & 1) << 20 | (arg(4) & 7) << 28;
                    r.tex[unit] = TexSet { addr: arg(1) & !7, width: arg(2), height: arg(3), format: arg(5), param };
                }
            }
            // Pica::Viewport(x, y, w, h)
            0x0012_63E0 => r.viewport = [arg(0) as i32, arg(1) as i32, arg(2) as i32, arg(3) as i32],
            // Pica::ScissorTest(enable, x, y, w, h)
            0x0012_5F60 => r.scissor = (arg(0) != 0).then(|| [arg(1) as i32, arg(2) as i32, arg(3) as i32, arg(4) as i32]),
            // Pica::Blend(enable): off is one, zero (blending stays on)
            0x0013_027C => {
                if arg(0) == 0 {
                    r.blend = 0x0101_0000;
                }
            }
            // Pica::BlendFunc(eq rgb, eq alpha, src rgb, dst rgb, src alpha, dst alpha)
            0x0013_03FC => r.blend = arg(0) | arg(1) << 8 | arg(2) << 16 | arg(3) << 20 | arg(4) << 24 | arg(5) << 28,
            // Pica::AlphaTest(enable, func, ref)
            0x0012_6548 => r.alpha = (arg(0) & 1) | (arg(1) & 7) << 4 | (arg(2) & 0xFF) << 8,
            // Pica::DepthTest(enable, func, write)
            0x0012_657C => r.depth = (arg(0) & 1) | (arg(1) & 7) << 4 | ((arg(2) != 0) as u32) << 12,
            // Pica::ColorMask(r, g, b, a)
            0x003D_26D0 => r.mask = (arg(0) != 0) as u32 | ((arg(1) != 0) as u32) << 1 | ((arg(2) != 0) as u32) << 2 | ((arg(3) != 0) as u32) << 3,
            // Pica::Culling: post quads are never culled
            0x0013_02D8 => {}
            // Pica::UniformMatrix(register, const MTX44*)
            0x0013_76BC => {
                for (i, m) in r.matrix.iter_mut().enumerate() {
                    *m = f32::from_bits(r32(system, arg(1) + 4 * i as u32));
                }
            }
            // PostEffectContext::InitBeforeDraw: its Pica:: calls come in
            // on their own
            0x0020_3BF8 => {}
            // nn::gr::CTR::FrameBuffer::MakeCommand(this, ...): the target
            0x0011_3354 => {
                let fb = arg(0);
                let color = r32(system, fb);
                r.fb = Some(Framebuffer { addr: phys(system, color), format: r8(system, fb + 4) as u32, width: r32(system, fb + 8), rows: r32(system, fb + 0xC) });
            }
            // FrameBuffer::MakeCommand(cmd, bool): the display's framebuffer
            // object (literal 0x00106550) through nn::gr's
            0x0010_653C => {
                let fb = r32(system, zakuro_core::region::us(0x0010_6550));
                self.call(system, surfaces, 0x0011_3354, &[fb, arg(0), 3, arg(1)]);
            }
            // PostEffectContext::DrawQuads(this, first quad, quads)
            0x0020_4030 => {
                self.expect_draw += 1;
                self.draw_quads(system, surfaces, arg(0), arg(1), arg(2));
            }
            // Pica::Draw(count): DrawQuads' own, or one this layer doesn't know
            0x0013_01A8 => {
                if self.expect_draw > 0 {
                    self.expect_draw -= 1;
                } else {
                    self.unsupported(format!("Pica::Draw outside DrawQuads ({} vertices)", arg(0)));
                }
            }
            // nngxFilterBlockImage(src, dst, width, height, GL format)
            0x005C_85EC => match gl_format(arg(4)) {
                Some(format) => {
                    let (src, dst) = (phys(system, arg(0)), phys(system, arg(1)));
                    let mut out = Vec::new();
                    surfaces.filter(src, dst, arg(2), arg(3), format, &mut out);
                    self.block.ops.extend(out.into_iter().map(Op::Act));
                    crate::cache::native_copy(zakuro_core::native_gfx::Copy::Transfer, src, dst); // gfx-perf
                }
                None => self.unsupported(format!("nngxFilterBlockImage format {:X}", arg(4))),
            },
            // nngxAddBlockImageCopyCommand(src, src unit, src gap, dst, dst unit, dst gap, size)
            0x005C_854C => {
                let (src, dst) = (phys(system, arg(0)), phys(system, arg(3)));
                let mut out = Vec::new();
                if !surfaces.copy(src, arg(1), arg(2), dst, arg(4), arg(5), arg(6), &mut out) {
                    self.unsupported(format!("block image copy {src:08X} -> {dst:08X} ({:X} bytes)", arg(6)));
                } else {
                    crate::cache::native_copy(zakuro_core::native_gfx::Copy::TextureCopy, src, dst); // gfx-perf
                }
                self.block.ops.extend(out.into_iter().map(Op::Act));
            }
            // nngxAddMemoryFillCommand(start0, size0, value0, width0, start1, size1, value1, width1)
            0x0011_3C00 => {
                for k in [0usize, 4] {
                    let (start, size, value, width) = (arg(k), arg(k + 1), arg(k + 2), arg(k + 3));
                    if start == 0 || size == 0 {
                        continue;
                    }
                    let value = match width {
                        16 => value & 0xFFFF | (value & 0xFFFF) << 16,
                        _ => value,
                    };
                    let s = phys(system, start);
                    surfaces.fill(s, s + size, value);
                    crate::cache::native_copy(zakuro_core::native_gfx::Copy::Fill, s, s + size); // gfx-perf
                }
            }
            // CommandAlter::AddCallback(callback)
            0x001A_C484 => self.block.ops.push(Op::Callback(arg(0))),
            _ => {}
        }
    }

    /// PostEffectContext::DrawQuads: the context's vertex array (+8) and
    /// quad index list (+0xC, six u16 per quad) drawn with the state now.
    fn draw_quads(&mut self, system: &mut System, surfaces: &mut Surfaces, ctx: u32, first: u32, quads: u32) {
        let r = self.regs;
        if r.depth & 1 != 0 {
            self.unsupported("a depth-tested post draw".to_owned());
        }
        let (vb, ib) = (r32(system, ctx + 8), r32(system, ctx + 0xC));
        let indices: Vec<u32> = (0..quads * 6).map(|i| r16(system, ib + first * 12 + 2 * i) as u32).collect();
        let count = indices.iter().max().map_or(0, |m| m + 1);
        let mut vertices = vec![0u8; count as usize * 40];
        system.memory.read_bytes(vb, &mut vertices);
        let used: Vec<usize> = {
            let mut u: Vec<usize> = indices.iter().map(|&i| i as usize).collect();
            u.sort_unstable();
            u.dedup();
            u
        };
        let f = |v: usize, k: usize| f32::from_le_bytes(vertices[v * 40 + k * 4..v * 40 + k * 4 + 4].try_into().unwrap());
        // the window rectangle the draw covers: the viewport, the scissor,
        // and the quads' corners
        let [vx, vy, vw, vh] = r.viewport.map(|v| v as f32);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &v in &used {
            let p = [f(v, 0), f(v, 1), f(v, 2), 1.0];
            let row = |i: usize| (0..4).map(|k| r.matrix[i * 4 + k] * p[k]).sum::<f32>();
            let w = row(3);
            if w.abs() < 1e-6 {
                continue;
            }
            let (nx, ny) = (row(0) / w, row(1) / w);
            let (wx, wy) = (vx + (nx + 1.0) / 2.0 * vw, vy + (ny + 1.0) / 2.0 * vh);
            x0 = x0.min(wx);
            x1 = x1.max(wx);
            y0 = y0.min(wy);
            y1 = y1.max(wy);
        }
        x0 = x0.max(vx);
        y0 = y0.max(vy);
        x1 = x1.min(vx + vw);
        y1 = y1.min(vy + vh);
        if let Some([sx, sy, sw, sh]) = r.scissor {
            x0 = x0.max(sx as f32);
            y0 = y0.max(sy as f32);
            x1 = x1.min((sx + sw) as f32);
            y1 = y1.min((sy + sh) as f32);
        }
        let mut out = Vec::new();
        // the textures, brought up to date for the rows the quads sample
        let mut units = [Unit::Off; 3];
        let unit2_uv1 = r.config & (1 << 13) != 0;
        for (u, unit) in units.iter_mut().enumerate() {
            if r.config & (1 << u) == 0 {
                continue;
            }
            let t = r.tex[u];
            if t.addr == 0 || t.width == 0 || t.height == 0 {
                continue;
            }
            let uv = 3 + 2 * if u == 2 && unit2_uv1 { 1 } else { u };
            let (mut t0, mut t1) = (f32::MAX, f32::MIN);
            for &v in &used {
                t0 = t0.min(f(v, uv + 1));
                t1 = t1.max(f(v, uv + 1));
            }
            let h = t.height as f32;
            let r0 = ((h * (1.0 - t1.min(1.0))).floor() - 1.0).max(0.0) as u32;
            let r1 = ((h * (1.0 - t0.max(0.0))).ceil() + 1.0).min(h) as u32;
            let samp = sampling(t.param);
            let bytes = t.width * t.height * crate::surfaces::bpp(t.format.min(4));
            if t.format <= 4 && surfaces.owned(t.addr, t.addr + bytes) {
                let (desc, row) = surfaces.read(t.addr, t.width, t.format, t.height, r0, r1.max(r0 + 1), &mut out);
                *unit = Unit::Surface { desc, row, width: t.width, height: t.height, sampling: samp };
            } else {
                let virt = crate::prim::virt(system, t.addr);
                *unit = Unit::Guest { tex: TexRef { addr: virt, width: t.width, height: t.height, format: t.format }, key: None, sampling: samp };
            }
        }
        // the target, brought up to date where the quads draw
        let target = match r.fb {
            None => {
                self.unsupported("a post draw before any FrameBuffer::MakeCommand".to_owned());
                return;
            }
            Some(fb) if surfaces.display_at(fb.addr).is_some() => Target::Display,
            Some(fb) => {
                let rows = fb.rows;
                let (m0, m1) = if x1 > x0 && y1 > y0 { ((rows as f32 - y1).floor().max(0.0) as u32, (rows as f32 - y0).ceil().min(rows as f32) as u32) } else { (0, 0) };
                match surfaces.write(fb.addr, fb.width, fb.format, rows, m0, m1.max(m0), &mut out) {
                    Some((desc, row)) => Target::Surface { desc, row, rows },
                    None => Target::Display,
                }
            }
        };
        self.block.ops.extend(out.into_iter().map(Op::Act));
        let draw = PostDraw {
            target,
            vertices,
            indices,
            matrix: r.matrix,
            viewport: r.viewport,
            scissor: r.scissor,
            combiner: r.combiner,
            blend: r.blend,
            blend_color: r.blend_color,
            mask: r.mask,
            alpha_test: r.alpha & 1 != 0,
            alpha_func: ((r.alpha >> 4) & 7) as u8,
            alpha_ref: ((r.alpha >> 8) & 0xFF) as u8,
            units,
            unit2_uv1,
        };
        if crate::perf::var_os("FATES_GFX_POST_DUMP").is_some() { // gfx-perf: cached env
            self.dumps.push(dump(&r, &draw, ctx, first, quads));
        }
        self.block.ops.push(Op::Draw(draw));
    }
}

/// a draw's state as the PICA registers dev/gfx-post/oracle.py compares
/// would hold it, one `name value` per line
fn dump(r: &Regs, d: &PostDraw, ctx: u32, first: u32, quads: u32) -> String {
    let mut s = format!("ctx {ctx:08X}\nfirst {first}\nindices {}\n", quads * 6);
    if let Some(fb) = r.fb {
        s += &format!("fb_color {:08X}\nfb_width {}\nfb_height {}\n", fb.addr, fb.width, fb.rows);
    }
    s += &format!("viewport {} {} {} {}\n", r.viewport[0], r.viewport[1], r.viewport[2], r.viewport[3]);
    match r.scissor {
        Some(sc) => s += &format!("scissor 3 {} {} {} {}\n", sc[0], sc[1], sc[0] + sc[2] - 1, sc[1] + sc[3] - 1),
        None => s += "scissor 0\n",
    }
    s += &format!("tex_config {:08X}\n", r.config & 0x2707);
    for u in 0..3 {
        if r.config & (1 << u) != 0 {
            let t = r.tex[u];
            s += &format!("unit{u} {:08X} {} {} {:08X} {}\n", t.addr, t.width, t.height, t.param & 0x0FFF_FFFF, t.format);
        }
    }
    let nib = |v: [u8; 3]| v[0] as u32 | (v[1] as u32) << 4 | (v[2] as u32) << 8;
    for (i, st) in r.combiner.stages.iter().enumerate() {
        let st: &Stage = st;
        s += &format!(
            "tev{i} {:08X} {:08X} {:08X} {:08X} {:08X}\n",
            nib(st.src_rgb) | nib(st.src_a) << 16,
            nib(st.op_rgb) | nib(st.op_a) << 12,
            st.comb_rgb as u32 | (st.comb_a as u32) << 16,
            u32::from_le_bytes(st.konst),
            st.scale_rgb as u32 | (st.scale_a as u32) << 16
        );
    }
    s += &format!("buffer_input {:08X}\n", (r.combiner.update_rgb as u32) << 8 | (r.combiner.update_a as u32) << 12);
    s += &format!("blend {:08X}\nalpha {:08X}\nmask {:X}\n", r.blend, r.alpha, r.mask);
    for row in 0..4 {
        s += &format!("c{row} {:.5} {:.5} {:.5} {:.5}\n", d.matrix[row * 4], d.matrix[row * 4 + 1], d.matrix[row * 4 + 2], d.matrix[row * 4 + 3]);
    }
    s
}

/// The native post layer's state in the renderer's front end (lib.rs).
#[derive(Default)]
pub struct State {
    pub surfaces: Surfaces,
    pub recorder: Option<Recorder>,
    /// draws dumped so far (FATES_GFX_POST_DUMP)
    pub dumped: u32,
    pub blocks: u64,
}

impl State {
    /// a post effect starts on display `disp`: its colour buffer is the
    /// display's native target from now on
    pub fn begin(&mut self, system: &mut System, disp: u8) {
        let fb = r32(system, zakuro_core::region::us(0x0010_6564));
        let color = r32(system, fb);
        let display = Display {
            key: crate::surfaces::Key { addr: phys(system, color), width: r32(system, fb + 8), format: r8(system, fb + 4) as u32 },
            rows: r32(system, fb + 0xC),
        };
        self.surfaces.set_display(disp, display);
        self.recorder = Some(Recorder::new(disp, display));
        set_recording(true);
    }

    pub fn call(&mut self, system: &mut System, function: u32, args: &[u32]) {
        if let Some(rec) = self.recorder.as_mut() {
            rec.call(system, &mut self.surfaces, function, args);
        }
    }

    /// the recording so far, as a block; recording stops
    pub fn end(&mut self) -> Option<Block> {
        set_recording(false);
        let rec = self.recorder.take()?;
        self.blocks += 1;
        // gfx-terrain: dev, the PICA combiner the effect leaves behind
        if std::env::var_os("FATES_GFX_POST_FINAL").is_some() && self.blocks < 3 {
            eprintln!("gfx-post final combiner {:?} blend {:08X} alpha {:08X} depth {:08X}", rec.regs.combiner, rec.regs.blend, rec.regs.alpha, rec.regs.depth);
        }
        if let Some(dir) = std::env::var_os("FATES_GFX_POST_DUMP") {
            let _ = std::fs::create_dir_all(&dir);
            let limit = std::env::var("FATES_GFX_POST_DUMP_N").ok().and_then(|v| v.parse().ok()).unwrap_or(8u32);
            for d in rec.dumps {
                if self.dumped < limit {
                    let _ = std::fs::write(std::path::Path::new(&dir).join(format!("native-{}.txt", self.dumped)), d);
                }
                self.dumped += 1;
            }
        }
        let _ = rec.display;
        Some(rec.block)
    }
}

fn words(system: &mut System, at: u32, n: u32) -> String {
    (0..n).map(|i| format!("{:08X}", r32(system, at + 4 * i))).collect::<Vec<_>>().join(" ")
}

/// FATES_GFX_TRACE_CALLBACKS: a post effect's object (a BloomContext), the
/// objects it names, and the display's framebuffer.
pub fn trace_effect(system: &mut System, effect: u32) {
    eprintln!("gfx-post effect {effect:08X}: {}", words(system, effect, 0x24));
    let fb = effect + 0x28;
    eprintln!("gfx-post   framebuffer +0x28: {}", words(system, fb, 0x12));
    let cb = r32(system, effect + 0x10);
    eprintln!("gfx-post   capture callback {cb:08X}: {}", words(system, cb, 4));
    for (name, at) in [("+0x14", r32(system, effect + 0x14)), ("+0x20", r32(system, effect + 0x20)), ("+0x08", r32(system, effect + 8)), ("cb+4", r32(system, cb + 4)), ("cb+8", r32(system, cb + 8)), ("cb+C", r32(system, cb + 0xC))] {
        let p = phys(system, at);
        eprintln!("gfx-post   {name} {at:08X} -> phys {p:08X}");
    }
    let settings = r32(system, zakuro_core::region::us(0x001A_B580));
    eprintln!("gfx-post   settings {settings:08X}: {}", words(system, settings, 8));
    eprintln!("gfx-post   filter flags {:08X}", r32(system, zakuro_core::region::us(0x0021_7524)));
    let display = r32(system, zakuro_core::region::us(0x0010_6564));
    eprintln!("gfx-post   display framebuffer {display:08X}: {}", words(system, display, 0x12));
    let vb = r32(system, effect + 8);
    for q in 0..16u32 {
        let mut line = format!("gfx-post   quad {q:2}:");
        for v in 0..4u32 {
            let at = vb + (q * 4 + v) * 40;
            let f: Vec<f32> = (0..9).map(|k| f32::from_bits(r32(system, at + 4 * k))).collect();
            line += &format!(" [{:.1} {:.1} {:.1} | {:.4} {:.4} | {:.4} {:.4} | {:.4} {:.4} | {:08X}]", f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], f[8], r32(system, at + 36));
        }
        eprintln!("{line}");
    }
    let ib = r32(system, effect + 0xC);
    eprintln!("gfx-post   indices: {}", (0..16u32).map(|i| format!("{}", crate::prim::r16(system, ib + 2 * i))).collect::<Vec<_>>().join(" "));
    let color = r32(system, display);
    eprintln!("gfx-post   display colour {color:08X} -> phys {:08X}", phys(system, color));
}

/// a block as a frame draw (it stands where its callback was)
pub fn as_draw(block: std::sync::Arc<Block>) -> crate::prim::Draw {
    crate::prim::Draw {
        vertices: Vec::new(),
        indices: Vec::new(),
        world: [0.0; 12],
        view_proj: [0.0; 16],
        tex_matrix: [0.0; 12],
        color: [1.0; 4],
        textures: [None; 3],
        combiner: Combiner::prim_reset(),
        blend: crate::prim::Blend::prim(7),
        depth_test: false,
        depth_func: 0,
        depth_write: false,
        alpha_test: false,
        alpha_func: 0,
        alpha_ref: 0,
        cull: 0,
        scissor: [0, 0, 0, 0],
        viewport: [0, 0, 0, 0],
        shaded: None,
        post: Some(block),
        depth_offset: 0.0, // gfx-gaps
    }
}
