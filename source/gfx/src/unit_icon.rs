//! gfx-gaps: map unit sprites, marks and HP gauges drawn natively.
//!
//! Battle maps draw their units through a prim callback inside the scene's
//! command mode (scene2d.rs): `UnitIcon::BatchCallback::OnBegin`
//! (0x004F4200). It does not use its prim state's geometry. For each
//! `BatchDrawInfo` (0x80 bytes, list at `*(lit 0x004F43AC) + 8`, count at
//! +4) it sets a depth test (always, or less-equal when the unit's flag
//! 0x20 is set) and calls:
//!
//! - `DrawUnitIcon` (0x004F3DDC): the world matrix (`Pica::UniformMatrix`
//!   0x0013BE5C, the shader's world register, info +0x48), the unit sheet
//!   texture (`Pica::TextureSetting`, then its texture matrix), the
//!   alpha test, the combiner (`UnitIcon::TransferTexture` 0x004F4674,
//!   `TransferTextureGray` 0x004F48F4, `TransferTextureSilhouette`
//!   0x004F4E08: `Pica::TexEnvConst` for the unit's colours, then the
//!   stages, built once into a `PicaCommand` and replayed with
//!   `PicaCommand::Transfer` 0x0011F8B4 after that), and one quad per pass
//!   through `BatchDrawInfo::DrawVertex` (0x00546E38:
//!   `PicaHelper::SetVertexArray` 0x0017DB88, `VertexBuffer::PushRect...`,
//!   `PicaHelper::DrawQuad` 0x0013019C);
//! - `DrawMark` (0x004F43C0): the icon sheet, stage 5, one quad;
//! - `DrawHPGauge` (0x004F3AC0): stage 5 to the vertex colour, the gauge's
//!   quads.
//!
//! Then it sets depth less-equal and the world matrix back to identity.
//!
//! overrides/gfx_unit_icon.c tells this module when OnBegin starts and
//! ends. In between, the observed calls (gfx_post_observe, the same entry
//! gfx-post's recorder uses) are recorded here: the state the helpers set,
//! and at each DrawQuad the quad vertices the game just wrote, as a native
//! 2D draw. The draws go into the scene island's draws at the callback
//! state's place, so they depth-test against the scene as the emulated
//! ones do. A `PicaCommand::Transfer` is read from the command words the
//! PicaCommand holds (+4 words, +0x10 their size): TEV register writes,
//! decoded as setup.rs decodes H3D's command blobs.
//!
//! `UnitIcon::DrawCallback::OnBegin` (0x004F37C8), a top-level prim
//! callback (unit icons in menus and on the battle forecast), sets the same
//! combiner for its state's own geometry: `draw_callback` (prim.rs).

use std::ffi::c_void;

use zakuro_core::System;

use crate::prim::{self, r32, r8, Combiner, Draw, Sampling, Stage, TexRef};

/// The table overrides/gfx_unit_icon.c calls through,
/// `fates_gfx_hooks_unit_icon` there.
#[repr(C)]
pub struct Hooks {
    /// UnitIcon::BatchCallback::OnBegin(this), before it runs
    pub begin: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    /// the same, after it returned
    pub end: Option<unsafe extern "C" fn(*mut c_void, u32)>,
}

pub const BATCH_ON_BEGIN: u32 = 0x004F_4200;
pub const DRAW_ON_BEGIN: u32 = 0x004F_37C8;

/// gfx-units: dev: the batch being recorded is traced
/// (FATES_GFX_UNITICON_TRACE=N: the first N batches;
/// FATES_GFX_UNITICON_TRACE_FRAMES=a-b: those in gfx frames a to b)
pub static TRACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn tracing() -> bool {
    TRACING.load(std::sync::atomic::Ordering::Relaxed)
}

/// gfx-units: dev, whether gfx frame `frame` is in FATES_GFX_UNITICON_TRACE_FRAMES
pub fn trace_window(frame: u64) -> bool {
    crate::perf::var("FATES_GFX_UNITICON_TRACE_FRAMES").is_ok() && trace_batch(frame)
}

/// gfx-units: whether the batch in gfx frame `frame` is traced
pub fn trace_batch(frame: u64) -> bool {
    static TRACED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if let Ok(v) = crate::perf::var("FATES_GFX_UNITICON_TRACE_FRAMES") {
        let (a, b) = v.split_once('-').unwrap_or((&v, &v));
        return a.parse::<u64>().is_ok_and(|a| frame >= a) && b.parse::<u64>().is_ok_and(|b| frame <= b);
    }
    std::env::var_os("FATES_GFX_UNITICON_TRACE").is_some()
        && TRACED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < crate::perf::var("FATES_GFX_UNITICON_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(3)
}

/// FATES_GFX_UNITICON=0 leaves the unit icons to the emulated GPU.
pub fn enabled() -> bool {
    std::env::var("FATES_GFX_UNITICON").map_or(true, |v| v != "0")
}

/// The TEV registers of stage `i` (source, operand, combine, constant,
/// scale follow).
const STAGE_REG: [usize; 6] = [0xC0, 0xC8, 0xD0, 0xD8, 0xF0, 0xF8];

/// a TEV register write into the combiner; false for any other register
pub fn tev_write(c: &mut Combiner, reg: usize, v: u32) -> bool {
    if reg == 0xE0 {
        c.update_rgb = ((v >> 8) & 0xF) as u8;
        c.update_a = ((v >> 12) & 0xF) as u8;
        return true;
    }
    if reg == 0xFD {
        c.buffer = v.to_le_bytes();
        return true;
    }
    let Some(i) = STAGE_REG.iter().position(|&b| (b..b + 5).contains(&reg)) else { return false };
    let s = &mut c.stages[i];
    let n = |shift: u32| ((v >> shift) & 0xF) as u8;
    match reg - STAGE_REG[i] {
        0 => {
            s.src_rgb = [n(0), n(4), n(8)];
            s.src_a = [n(16), n(20), n(24)];
        }
        1 => {
            s.op_rgb = [n(0), n(4), n(8)];
            s.op_a = [n(12) & 7, n(16) & 7, n(20) & 7];
        }
        2 => {
            s.comb_rgb = n(0);
            s.comb_a = n(16);
        }
        3 => s.konst = v.to_le_bytes(),
        _ => {
            s.scale_rgb = (v & 3) as u8;
            s.scale_a = ((v >> 16) & 3) as u8;
        }
    }
    true
}

/// PicaCommand::Transfer(command): the TEV register writes among the
/// command words it holds (+4 the words, +0x10 their size in bytes) into
/// the combiner. False when it holds none yet.
pub fn transfer(system: &mut System, command: u32, c: &mut Combiner) -> bool {
    let (words, size) = (r32(system, command + 4), r32(system, command + 0x10));
    if words == 0 || size == 0 || size > 0x1000 {
        return false;
    }
    let w: Vec<u32> = (0..size / 4).map(|i| r32(system, words + 4 * i)).collect();
    let mut i = 0usize;
    while i + 1 < w.len() {
        let header = w[i + 1];
        let id = (header & 0xFFFF) as usize;
        let mask = (header >> 16) & 0xF;
        let extra = ((header >> 20) & 0xFF) as usize;
        let consecutive = header >> 31 != 0;
        let mut put = |r: usize, v: u32| {
            if mask == 0xF {
                tev_write(c, r, v);
            }
        };
        put(id, w[i]);
        let mut r = id;
        for k in 0..extra {
            let Some(&v) = w.get(i + 2 + k) else { break };
            if consecutive {
                r += 1;
            }
            put(r, v);
        }
        i += 2 + extra + (extra & 1);
    }
    true
}

/// Records one BatchCallback::OnBegin.
pub struct Recorder {
    /// the prim context and its tables
    prim: u32,
    tables: prim::Tables,
    /// the state the helpers set so far
    cur: Draw,
    /// the shader's world and texture-matrix uniform registers
    world_reg: u32,
    tex_reg: u32,
    vertex_array: u32,
    /// the VertexBuffer the batch pushes its quads into
    /// (`*(*(lit 0x004F43A8) + 100)`), and the first vertex of a quad set
    /// up by SetVertexArray whose DrawQuad was not seen: DrawVertex
    /// tail-branches into DrawQuad, which the generated code may run in
    /// place, so such a draw is completed at the next observed call
    vb: u32,
    pending: Option<u32>,
    pub draws: Vec<Draw>,
    pub unsupported: Vec<String>,
}

impl Recorder {
    /// OnBegin starts for the prim state `state` (whose UpdateState has
    /// run) of prim context `prim`, display `disp`.
    pub fn new(system: &mut System, tables: prim::Tables, prim_ctx: u32, disp: u32, state: u32) -> Recorder {
        let cur = prim::state_setup(system, &tables, prim_ctx, disp, state);
        // prim::Context::GetCurrentShader (0x0053A47C) for the state, and
        // its uniform registers at +0x12900 (world) and +0x1290C (texture
        // matrix), as BatchCallback reads them
        let shape = r32(system, state) & 7;
        let table = r32(system, 0x0053_A4B0);
        let index = r32(system, table + shape * 4);
        let shader = r32(system, prim_ctx + index * 4 + 0x29E4);
        let (world_reg, tex_reg) = if shader != 0 { (r32(system, shader + 0x12900) & 0xFF, r32(system, shader + 0x1290C) & 0xFF) } else { (u32::MAX, u32::MAX) };
        let batch = r32(system, 0x004F_43A8);
        let vb = r32(system, batch + 100);
        if tracing() {
            // gfx-units: dev, the field camera (FieldWorld::GetCamera
            // 0x001782F8: *(*(lit 0x00178308) + 0x34)) and its matrices
            let lit = r32(system, 0x0017_8308);
            let world = r32(system, lit);
            let camera = r32(system, world + 0x34);
            if camera != 0 {
                for (name, at, n) in [("view", 0x98u32, 12u32), ("proj", 0xC8, 16), ("viewproj", 0x108, 16), ("invview", 0x2A8, 12), ("invproj", 0x2D8, 16), ("unproj", 0x318, 16)] {
                    let m: Vec<f32> = (0..n).map(|i| f32::from_bits(r32(system, camera + at + 4 * i))).collect();
                    eprintln!("gfx-uniticon   camera {camera:08X} {name} {m:?}");
                }
            }
            let w: Vec<String> = (0..6).map(|k| format!("{:08X}", r32(system, state + 4 * k))).collect();
            eprintln!("gfx-uniticon   state {} shape {shape} shader {shader:08X} world reg {world_reg:X} tex reg {tex_reg:X} prim45 {:02X} prim44 {:02X}", w.join(" "), r8(system, prim_ctx + 0x45), r8(system, prim_ctx + 0x44));
        }
        Recorder { prim: prim_ctx, tables, cur, world_reg, tex_reg, vertex_array: 0, vb, pending: None, draws: Vec::new(), unsupported: Vec::new() }
    }

    /// the VertexBuffer's current buffer: (physical address, vertices)
    /// (VertexBuffer::GetAddress 0x0013081C, GetSize 0x0013082C)
    fn vb_now(&self, system: &mut System) -> (u32, u32) {
        let inner = if self.vb != 0 { r32(system, self.vb + 0xC) } else { 0 };
        if inner == 0 {
            return (0, 0);
        }
        (r32(system, inner + 4), r32(system, inner + 0x14))
    }

    /// a quad set whose DrawQuad was not seen: drawn with the state as it
    /// is, from its first vertex to the buffer's end
    fn flush(&mut self, system: &mut System) {
        let Some(first) = self.pending.take() else { return };
        let (_, size) = self.vb_now(system);
        if size > first {
            self.quads(system, size - first);
        }
    }

    /// the end of the callback: a draw still open is completed
    pub fn finish(&mut self, system: &mut System) {
        self.flush(system);
    }

    /// `n` vertices of quads from the vertex array, drawn with the state now
    fn quads(&mut self, system: &mut System, n: u32) {
        if n == 0 || n > 0x4000 || self.vertex_array == 0 {
            return;
        }
        let mut vertices = vec![0u8; n as usize * 24];
        let at = prim::virt(system, self.vertex_array);
        prim::rd(system, at, &mut vertices);
        let indices = prim::quad_indices(system, &self.tables, self.prim, n * 3 / 2);
        if indices.iter().any(|&i| i >= n) {
            self.unsupported("quad indices past the vertices".to_owned());
            return;
        }
        if tracing() {
            let f = |k: usize| f32::from_le_bytes(vertices[k * 4..k * 4 + 4].try_into().unwrap());
            eprintln!("gfx-uniticon   quads at {:08X} (virt {at:08X}) n {n} v0 {} {} {} v1 {} {} {}", self.vertex_array, f(0), f(1), f(2), f(6), f(7), f(8));
        }
        let mut d = self.cur.clone();
        // dev: FATES_GFX_UNITICON_NODEPTH=1 draws them without the depth test
        if crate::perf::var_os("FATES_GFX_UNITICON_NODEPTH").is_some() {
            d.depth_test = false;
        }
        d.vertices = vertices;
        d.indices = indices;
        self.draws.push(d);
    }

    fn unsupported(&mut self, why: String) {
        if crate::perf::var_os("FATES_GFX_DRAW_TRACE").is_some() {
            eprintln!("gfx-draw-trace unsupported frame {} {why}", zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed)); // gfx-lines: dev
        }
        if !self.unsupported.contains(&why) {
            self.unsupported.push(why);
        }
    }

    /// gfx-units: whether an observed call is OnBegin's last step, the
    /// world matrix set back to identity (no unit sprite's is), so the
    /// batch is known to be over even when OnBegin's end hook does not run
    /// (the original left the override at an svc or budget exit)
    pub fn ends_batch(&self, system: &mut System, function: u32, a: &[u32]) -> bool {
        if function != 0x0013_BE5C || a.len() < 2 || a[0] & 0xFF != self.world_reg {
            return false;
        }
        let m: [f32; 12] = std::array::from_fn(|i| f32::from_bits(r32(system, a[1] + 4 * i as u32)));
        m == [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.]
    }

    /// one observed call, as gfx_post_observe hands it over
    pub fn call(&mut self, system: &mut System, function: u32, a: &[u32]) {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0);
        if crate::perf::var_os("FATES_GFX_UNITICON_TRACE_CALLS").is_some() && tracing() {
            eprintln!("gfx-uniticon   call {function:08X} {:08X?}", &a[..a.len().min(4)]); // gfx-units: dev
        }
        if function != 0x0013_019C {
            self.flush(system);
        }
        // the UnitIcon functions that replay a PicaCommand of stages
        // (their own Transfer can be a tail branch the generated code runs
        // in place): the command, as `*(lit) + offset`
        let replayed = match function {
            0x004F_4674 => Some((0x004F_487C, 0x58)), // TransferTexture
            0x004F_48F4 => Some((0x004F_4BCC, 0x5C)), // TransferTextureGray
            0x004F_4E08 => Some((0x004F_4F38, 0x60)), // TransferTextureSilhouette
            0x004F_43C0 => Some((0x004F_45BC, 0x54)), // BatchCallback::DrawMark
            0x004F_3AC0 => Some((0x004F_3DC4, 0x50)), // BatchCallback::DrawHPGauge
            _ => None,
        };
        if let Some((lit, off)) = replayed {
            let object = r32(system, lit);
            let command = r32(system, object + off);
            // empty: the function builds it now, through observed calls
            let _ = transfer(system, command, &mut self.cur.combiner);
            return;
        }
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
                if !transfer(system, arg(0), c) {
                    self.unsupported(format!("PicaCommand::Transfer of an empty command {:08X}", arg(0)));
                }
            }
            // Pica::TextureSamplerType: unit 0 only (the others stay off
            // in the prim layer's draws)
            0x0013_00F0 => {}
            // Pica::TextureSetting(unit, address, width, height, type,
            // format, wrap s, wrap t, min, mag, ...)
            0x0013_7718 => {
                if tracing() {
                    // mapsprites: dev, the sheet and its filters
                    eprintln!("gfx-uniticon   texture {:08X?}", &a[..a.len().min(12)]);
                }
                if arg(0) == 0 {
                    let tex = TexRef { addr: prim::virt(system, arg(1) & !7), width: arg(2), height: arg(3), format: arg(5) };
                    let linear_mag = arg(9) & 1 != 0;
                    // gfx-2d: nearest-sampled sprites are pixel art
                    // mapsprites: and so are the linear ones. The batch's
                    // sheets (unit sprites, marks, HP gauges) are always
                    // pixel art; DrawUnitIcon asks for linear (min 3, mag
                    // 1) whenever the map camera is zoomed out and the
                    // sprites are drawn smaller than 1:1, which at 8x
                    // was a plain bilinear blow-up of 32x32 art
                    let sharp = crate::perf::var("FATES_GFX_SHARP_SPRITES").map_or(true, |v| v != "0");
                    let sampling = Sampling { wrap: (arg(6) & 3) as u8, wrap_t: (arg(7) & 3) as u8, sharp, linear_mag, linear_min: matches!(arg(8), 1 | 3 | 5) };
                    self.cur.textures[0] = Some((tex, sampling));
                }
            }
            // Pica::UniformMatrix(register, const MTX34*)
            0x0013_BE5C => {
                let m: [f32; 12] = std::array::from_fn(|i| f32::from_bits(r32(system, arg(1) + 4 * i as u32)));
                let reg = arg(0) & 0xFF;
                if reg == self.world_reg {
                    self.cur.world = m;
                } else if reg == self.tex_reg {
                    self.cur.tex_matrix = m;
                } else {
                    self.unsupported(format!("UniformMatrix register {reg:X}"));
                }
            }
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
            // PicaHelper::SetVertexArray(physical address): quads follow
            0x0017_DB88 => {
                self.vertex_array = arg(0);
                let (base, _) = self.vb_now(system);
                if base != 0 && arg(0) >= base && (arg(0) - base) % 24 == 0 {
                    self.pending = Some((arg(0) - base) / 24);
                }
            }
            // PicaHelper::DrawQuad(vertices): the quads just pushed
            0x0013_019C => {
                self.pending = None;
                self.quads(system, arg(0));
            }
            // Pica::Draw and the like: a draw this module doesn't know
            0x0013_01A8 | 0x0020_4030 => self.unsupported(format!("draw call {function:08X}")),
            _ => {}
        }
    }
}

/// UnitIcon::DrawCallback::OnBegin (0x004F37C8) for a prim state: the
/// buffer prim::Callback::GetBufferAddr names (+4 flags, +0x14 and +0x30
/// colours), then TransferTextureGray (flag 8) or TransferTexture: the two
/// colours as stage constants, the stages from their PicaCommand.
pub fn draw_callback(system: &mut System, prim_ctx: u32, w2: u32, c: &mut Combiner) {
    let table = r32(system, prim_ctx + 0x2494);
    let entry = r32(system, table + ((w2 & 0x3FF_FFFF) >> 20) * 8);
    let buffer = if entry != 0 { r32(system, entry) } else { 0 };
    if buffer == 0 {
        return;
    }
    let color = |system: &mut System, at: u32| [r8(system, at), r8(system, at + 1), r8(system, at + 2), 0];
    let (c1, c2) = (color(system, buffer + 0x14), color(system, buffer + 0x30));
    let gray = r32(system, buffer + 4) & 8 != 0;
    // TransferTextureGray: constants of stages 0 and 5, PicaCommand at
    // *(lit 0x004F4BCC) + 0x5C; TransferTexture: stages 2 and 5,
    // *(lit 0x004F487C) + 0x58
    let (first, lit, off) = if gray { (0usize, 0x004F_4BCC, 0x5C) } else { (2usize, 0x004F_487C, 0x58) };
    c.stages[first].konst = c1;
    c.stages[5].konst = c2;
    let object = r32(system, lit);
    let command = r32(system, object + off);
    if !transfer(system, command, c) {
        // not built yet: the stages as the functions build them
        fallback_stages(c, gray);
    }
}

/// the stages TransferTexture / TransferTextureGray build (their
/// disassembly), for a PicaCommand that holds none yet
fn fallback_stages(c: &mut Combiner, gray: bool) {
    let st = |src_rgb: [u8; 3], src_a: [u8; 3], op_rgb: [u8; 3], op_a: [u8; 3], comb_rgb: u8, comb_a: u8, scale_rgb: u8, scale_a: u8, konst: [u8; 4]| Stage { src_rgb, src_a, op_rgb, op_a, comb_rgb, comb_a, scale_rgb, scale_a, konst };
    let s = &mut c.stages;
    if !gray {
        s[2] = st([3, 0xE, 0], [3, 0xE, 0], [0; 3], [0; 3], 1, 3, 0, 1, s[2].konst);
        s[3] = st([0xF, 0xF, 0], [0xF, 0, 0], [0, 2, 0], [0; 3], 1, 0, 0, 0, s[3].konst);
        s[4] = st([0xF, 3, 0xF], [3, 0, 0], [0, 0, 2], [0; 3], 4, 0, 0, 2, s[4].konst);
        s[5] = st([0, 0xF, 0xE], [0xF, 0, 0], [0; 3], [0; 3], 8, 1, 0, 0, s[5].konst);
    } else {
        s[0] = st([3, 0xE, 0], [3, 0xE, 0], [0; 3], [0; 3], 1, 3, 0, 1, s[0].konst);
        s[1] = st([0xF, 0xF, 0], [0xF, 0, 0], [0, 2, 0], [0; 3], 1, 0, 0, 0, s[1].konst);
        s[2] = st([0xF, 3, 0xF], [3, 0, 0], [0, 0, 2], [0; 3], 4, 1, 0, 0, s[2].konst);
        s[3] = st([0xF, 0xE, 0], [0xF, 0, 0], [0; 3], [0; 3], 1, 0, 0, 0, [0x4C, 0x95, 0x1D, 0xFF]);
        s[4] = st([0xF, 0xF, 0], [0xF, 0, 0], [4, 8, 0], [0; 3], 2, 0, 0, 0, s[4].konst);
        s[5] = st([0, 0xF, 0xE], [0xF, 0, 0], [0; 3], [0; 3], 8, 1, 0, 0, s[5].konst);
    }
}
