//! gfx-final: unit-icon textures rendered natively.
//!
//! `UnitIconInstance::OnBegin` (0x001F23D0) is a prim callback in the lower
//! display's list. For every unit icon still to be made (UnitIconObject
//! flags 8 and 0x80) it calls `UnitIconObject::CaptureTexture(object,
//! type)` (0x001D6650), which:
//!
//! - sets the icon's ITexture up (object + 0xA8 + type * 0x48: RGBA4, the
//!   size from the type's tables) and points an nn::gr FrameBuffer at it
//!   (`nn::gr::CTR::FrameBuffer::MakeCommand`, 0x00113354);
//! - sets the scissor and viewport to the texture, the view-projection
//!   (`ShaderObj::SetViewProj`, 0x001376AC), texture unit 0 off, stage 0 to
//!   the vertex colour, blending and the alpha test off;
//! - draws one transparent quad over the whole texture (the clear), through
//!   the object's VertexBuffer (+0x160) and `VertexBuffer::RenderQuad`
//!   (0x0050A0D4: `Pica::Draw` of the quads from a first vertex to the
//!   buffer's end);
//! - sets stage 0 to vertex colour x texture 0 and calls `CaptureSheet`,
//!   which draws the unit sheet's sprites (`PicaHelper::SetTexture`
//!   0x0017DA98, `ShaderObj::SetTexMtx` 0x00138848, `Pica::AlphaTest` with
//!   the sprite layer's alpha as an EQUAL reference, `DrawQuad` pushing
//!   rectangles, RenderQuad drawing them).
//!
//! OnBegin's start sets the prim layer's fixed state
//! (`prim::Context::SetupFixedState`: the combiner, built once into the
//! PicaCommand at prim context + 0x29EC and replayed with a tail-branched
//! `PicaCommand::Transfer`, read here from its words), depth and alpha
//! tests off. At its end the display's framebuffer comes back.
//!
//! overrides/gfx_capture.c says when OnBegin and each CaptureTexture start
//! and end; in between the observed calls (gfx_post_observe) are recorded
//! here as native 2D draws. When a CaptureTexture ends, the renderer draws
//! them into an offscreen target the texture's size (lib.rs
//! `capture_end`), reads it back, and the texels are written into guest
//! memory in the PICA's tiled RGBA4 layout, where every reader (the native
//! 2D layer's unit-icon draws, the emulated GPU if it ever samples it, the
//! CPU) finds them. The emulated GPU is no longer needed for icons.
//!
//! `FATES_GFX_CAPTURE=0` leaves them to the emulated GPU;
//! `FATES_GFX_CAPTURE_CHECK=1` (with `FATES_GFX_EMULATED=keep`, so the
//! emulated GPU still renders them) compares each native icon with the
//! emulated one when it next comes down.

use zakuro_core::System;

use crate::prim::{self, r32, Blend, Combiner, Draw, Sampling, TexRef};

/// UnitIconInstance::OnBegin, UnitIconObject::CaptureTexture
pub const INSTANCE_ON_BEGIN: u32 = 0x001F_23D0;

/// The table overrides/gfx_capture.c calls through,
/// `fates_gfx_hooks_capture` there.
#[repr(C)]
pub struct Hooks {
    pub instance: Option<unsafe extern "C" fn(*mut std::ffi::c_void, u32, u32)>,
    pub capture: Option<unsafe extern "C" fn(*mut std::ffi::c_void, u32, u32, u32)>,
}

/// FATES_GFX_CAPTURE=0 leaves the unit icons to the emulated GPU.
pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_CAPTURE").map_or(true, |v| v != "0")
}

/// The render target CaptureTexture points the framebuffer at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// virtual address of the texels
    pub addr: u32,
    pub width: u32,
    pub height: u32,
    /// PICA colour-buffer format (4: RGBA4)
    pub format: u32,
}

/// One CaptureTexture, recorded.
pub struct Capture {
    pub object: u32,
    pub target: Option<Target>,
    pub draws: Vec<Draw>,
}

/// Records one UnitIconInstance::OnBegin.
pub struct Recorder {
    cur: Draw,
    fb: Option<Target>,
    pub capture: Option<Capture>,
    pub unsupported: Vec<String>,
}

fn identity34() -> [f32; 12] {
    [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.]
}

impl Recorder {
    /// OnBegin starts. `world`: the world matrix UpdateState left for the
    /// callback's prim state (the shader's world register, which OnBegin
    /// does not set). `prim_ctx`: the prim context, whose fixed-state
    /// PicaCommand (+0x29EC) SetupFixedState replays.
    pub fn new(system: &mut System, prim_ctx: u32, world: [f32; 12]) -> Recorder {
        let mut combiner = Combiner::prim_reset();
        if prim_ctx != 0 {
            // empty the first time: SetupFixedState then builds it through
            // observed calls
            let _ = crate::unit_icon::transfer(system, prim_ctx + 0x29EC, &mut combiner);
        }
        let cur = Draw {
            vertices: Vec::new(),
            indices: Vec::new(),
            world,
            view_proj: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.],
            tex_matrix: identity34(),
            color: [1.0; 4],
            textures: [None; 3],
            combiner,
            // SetupFixedState: Blend(1) with one, zero; Blend(0) later
            blend: Blend { enable: false, eq: [0, 0], func: [1, 0, 1, 0] },
            depth_test: false,
            depth_func: 0,
            depth_write: false,
            alpha_test: false,
            alpha_func: 1,
            alpha_ref: 0,
            cull: 0,
            scissor: [0, 0, 1024, 1024],
            viewport: [0, 0, 1024, 1024],
            shaded: None,
            post: None,
            depth_offset: 0.0,
        };
        Recorder { cur, fb: None, capture: None, unsupported: Vec::new() }
    }

    fn unsupported(&mut self, why: String) {
        if !self.unsupported.contains(&why) {
            self.unsupported.push(why);
        }
    }

    pub fn begin_capture(&mut self, object: u32) {
        self.capture = Some(Capture { object, target: None, draws: Vec::new() });
    }

    /// the CaptureTexture done: its draws and target
    pub fn end_capture(&mut self) -> Option<Capture> {
        self.capture.take()
    }

    /// Pica::Draw(count) from VertexBuffer::RenderQuad: the quads from
    /// `count` / 6 * 4 vertices before the object's VertexBuffer's end
    fn draw(&mut self, system: &mut System, count: u32) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        let vb = cap.object + 0x160;
        let inner = r32(system, vb + 0xC);
        if inner == 0 || count == 0 || count % 6 != 0 {
            self.unsupported(format!("Pica::Draw of {count} indices"));
            return;
        }
        let (data, size) = (r32(system, inner + 0xC), r32(system, inner + 0x14));
        let n = count / 6 * 4;
        if n > size || n > 0x4000 {
            self.unsupported(format!("{n} vertices past the buffer's {size}"));
            return;
        }
        let first = size - n;
        let mut vertices = vec![0u8; n as usize * 24];
        prim::rd(system, data + first * 24, &mut vertices);
        // PushRect's corners: (x, y), (x + w, y), (x, y + h), (x + w, y + h);
        // the two triangles of each rectangle (culling is off, so their
        // winding does not matter)
        let indices = (0..n / 4).flat_map(|q| [0, 1, 2, 2, 1, 3].map(|i| q * 4 + i)).collect();
        let mut d = self.cur.clone();
        d.vertices = vertices;
        d.indices = indices;
        cap.target = cap.target.or(self.fb);
        cap.draws.push(d);
    }

    /// one observed call, as gfx_post_observe hands it over
    pub fn call(&mut self, system: &mut System, function: u32, a: &[u32]) {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0);
        let c = &mut self.cur.combiner;
        match function {
            // Pica::TexEnvSrc(stage, rgb0..2, alpha0..2)
            0x0012_6640 => {
                if let Some(s) = c.stages.get_mut(arg(0) as usize) {
                    s.src_rgb = [arg(1), arg(2), arg(3)].map(|v| v as u8 & 0xF);
                    s.src_a = [arg(4), arg(5), arg(6)].map(|v| v as u8 & 0xF);
                }
            }
            // Pica::TexEnvOperand(stage, rgb0..2, alpha0..2)
            0x0012_FFA8 => {
                if let Some(s) = c.stages.get_mut(arg(0) as usize) {
                    s.op_rgb = [arg(1), arg(2), arg(3)].map(|v| v as u8 & 0xF);
                    s.op_a = [arg(4), arg(5), arg(6)].map(|v| v as u8 & 0x7);
                }
            }
            // Pica::TexEnvCombine(stage, rgb, alpha)
            0x0012_FF68 => {
                if let Some(s) = c.stages.get_mut(arg(0) as usize) {
                    s.comb_rgb = arg(1) as u8 & 0xF;
                    s.comb_a = arg(2) as u8 & 0xF;
                }
            }
            // Pica::TexEnvConst(stage, r, g, b, a)
            0x003D_2214 => {
                if let Some(s) = c.stages.get_mut(arg(0) as usize) {
                    s.konst = [arg(1), arg(2), arg(3), arg(4)].map(|v| v as u8);
                }
            }
            // Pica::TexEnvScale(stage, rgb, alpha)
            0x0012_6060 => {
                if let Some(s) = c.stages.get_mut(arg(0) as usize) {
                    s.scale_rgb = arg(1) as u8 & 3;
                    s.scale_a = arg(2) as u8 & 3;
                }
            }
            // Pica::TexEnvBufferInput(rgb0, a0, rgb1, a1, rgb2, a2, rgb3, a3)
            0x0013_0098 => {
                let b = |i: usize| (arg(i) != 0) as u8;
                c.update_rgb = b(0) | b(2) << 1 | b(4) << 2 | b(6) << 3;
                c.update_a = b(1) | b(3) << 1 | b(5) << 2 | b(7) << 3;
            }
            // PicaCommand::Transfer(command): stages built before
            0x0011_F8B4 => {
                let _ = crate::unit_icon::transfer(system, arg(0), c);
            }
            // PicaHelper::SetTexture(ITexture*): null turns unit 0 off; the
            // texture itself comes with its Pica::TextureSetting
            0x0017_DA98 => {
                if arg(0) == 0 {
                    self.cur.textures[0] = None;
                }
            }
            // Pica::TextureSetting(unit, address, width, height, type,
            // format, wrap s, wrap t, min, mag, ...)
            0x0013_7718 => {
                if arg(0) == 0 {
                    let tex = TexRef { addr: prim::virt(system, arg(1) & !7), width: arg(2), height: arg(3), format: arg(5) };
                    let sampling = Sampling { wrap: (arg(6) & 3) as u8, wrap_t: (arg(7) & 3) as u8, sharp: false, linear_mag: arg(9) & 1 != 0, linear_min: matches!(arg(8), 1 | 3 | 5) };
                    self.cur.textures[0] = Some((tex, sampling));
                }
            }
            // ShaderObj::SetViewProj(shader, const MTX44&)
            0x0013_76AC => {
                self.cur.view_proj = std::array::from_fn(|i| f32::from_bits(r32(system, arg(1) + 4 * i as u32)));
            }
            // ShaderObj::SetTexMtx(shader, const MTX34&)
            0x0013_8848 => {
                self.cur.tex_matrix = std::array::from_fn(|i| f32::from_bits(r32(system, arg(1) + 4 * i as u32)));
            }
            // Pica::UniformMatrix(register, MTX34): SetTexMtx's own (its
            // tail branch, when it is observed), already taken
            0x0013_BE5C => {}
            // Pica::AlphaTest(enable, func, ref)
            0x0012_6548 => {
                self.cur.alpha_test = arg(0) & 1 != 0;
                self.cur.alpha_func = (arg(1) & 7) as u8;
                self.cur.alpha_ref = arg(2) as u8;
            }
            // Pica::DepthTest(enable, func, write)
            0x0012_657C => {
                self.cur.depth_test = arg(0) & 1 != 0;
                self.cur.depth_func = (arg(1) & 7) as u8;
                self.cur.depth_write = arg(2) != 0;
            }
            // Pica::Blend(enable): off is one, zero
            0x0013_027C => {
                if arg(0) == 0 {
                    self.cur.blend = Blend { enable: false, eq: [0, 0], func: [1, 0, 1, 0] };
                } else {
                    self.cur.blend.enable = true;
                }
            }
            // Pica::BlendFunc(eq rgb, eq alpha, src rgb, dst rgb, src alpha, dst alpha)
            0x0013_03FC => {
                let enable = self.cur.blend.enable;
                self.cur.blend = Blend { enable, eq: [arg(0) as u8, arg(1) as u8], func: [arg(2) as u8, arg(3) as u8, arg(4) as u8, arg(5) as u8] };
            }
            // Pica::Viewport(x, y, w, h), Pica::ScissorTest(enable, x, y, w, h)
            0x0012_63E0 => self.cur.viewport = [arg(0) as i32, arg(1) as i32, arg(2) as i32, arg(3) as i32],
            0x0012_5F60 => {
                self.cur.scissor = if arg(0) != 0 { [arg(1) as i32, arg(2) as i32, arg(3) as i32, arg(4) as i32] } else { [0, 0, 1024, 1024] };
            }
            // nn::gr::CTR::FrameBuffer::MakeCommand(this, ...): the colour
            // buffer (virtual), format, width, height
            0x0011_3354 => {
                let fb = arg(0);
                self.fb = Some(Target { addr: r32(system, fb), format: prim::r8(system, fb + 4) as u32, width: r32(system, fb + 8), height: r32(system, fb + 0xC) });
            }
            // FrameBuffer::MakeCommand(cmd, bool): the display's again
            0x0010_653C => self.fb = None,
            // Pica::Draw(count)
            0x0013_01A8 => self.draw(system, arg(0)),
            // draws this module does not know
            0x0013_019C | 0x0020_4030 => self.unsupported(format!("draw call {function:08X}")),
            _ => {}
        }
    }
}

/// the upright picture the renderer drew (`uw` = the texture's height
/// across, `uh` = its width down) as the texture's memory holds it: tiled
/// (8x8 tiles in Morton order, rows of tiles from memory row 0 on), each
/// pixel in the colour buffer's format as the PICA writes it (RGBA4: the
/// top four bits of each byte).
pub fn encode(upright: &[u8], t: &Target) -> Option<Vec<u8>> {
    let format = match t.format {
        0 => zakuro_gpu::format::ColorFormat::Rgba8,
        1 => zakuro_gpu::format::ColorFormat::Rgb8,
        2 => zakuro_gpu::format::ColorFormat::Rgb5A1,
        3 => zakuro_gpu::format::ColorFormat::Rgb565,
        4 => zakuro_gpu::format::ColorFormat::Rgba4,
        _ => return None,
    };
    let bpp = crate::surfaces::bpp(t.format);
    let (w, h) = (t.width as usize, t.height as usize);
    let (uw, uh) = (h, w);
    if upright.len() < uw * uh * 4 || w % 8 != 0 || h % 8 != 0 {
        return None;
    }
    let mut out = vec![0u8; w * h * bpp as usize];
    for r in 0..h {
        for c in 0..w {
            // as shadow.rs's memory_order: framebuffer x runs down the
            // upright picture from its bottom
            let s = ((w - 1 - c) * uw + r) * 4;
            let rgba = [upright[s], upright[s + 1], upright[s + 2], upright[s + 3]];
            let at = zakuro_gpu::format::morton_offset(c as u32, r as u32, t.width, bpp) as usize;
            format.encode(rgba, &mut out[at..at + bpp as usize]);
        }
    }
    Some(out)
}
