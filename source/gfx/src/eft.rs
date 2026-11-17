//! gfx-eft: nw::eft particles and stripes drawn natively.
//!
//! How Fates draws its effects (USA SE v2 addresses):
//!
//! - Every effect in a 3D scene is an `EffectNode`/`EffectEmitter` whose
//!   OnDrawXlu calls `EffectEmitter::Draw` (0x001C76DC) inside SceneSystem's
//!   scene pass. That switches the RenderState to the effect mode
//!   (RenderState::Change(2): projection, camera matrix and the eft camera
//!   position, see `setup::render_state_uniforms`) and calls
//!   `nw::eft::System::MakeDrawEmitterSetCommand` (0x0031E7BC) with the
//!   RenderState's command pointer (+0x10).
//! - That walks the set's emitters and calls each emitter's calc object
//!   (EmitterInstance +0xD8: EmitterSimpleCalc / EmitterComplexCalc /
//!   EmitterCompactCalc) virtual +0x10, `CalcParticle(cmd, emitter, pause,
//!   offset, frame step)`, and +0x14 for child particles. **In eft for CTR
//!   the simulation and the draw encoders are one pass**: CalcParticle
//!   starts the emitter's draw (`DrawCommander::MakeDrawCmdBegin`: the
//!   emitter's shader blob `Shader +4/+8`, its material blob
//!   `MaterialCache` at EmitterInstance +0x240, a 16-word uniform block of
//!   emitter colour and scale, the combiner constant colours), then per
//!   particle `_write{Simple,Complex,Compact,Child}Cmd` steps the particle
//!   (position += velocity, rotation, scale and alpha animation, texture
//!   pattern animation, colour interpolation) and writes its values as a
//!   10- or 7-vec4 float uniform block at c(1 + 10 k), and the draw-type
//!   function (`_drawCmdSimple_Billboard`, `_Dir`, `_Primitive`...) adds
//!   the type's words and every 8 particles a kick, the common resource's
//!   pre-built draw of 8 quads (`_drawCmd_Common_AddKickCmd`; the last
//!   batch's vertex count patched to 6 n in `MakeDrawCmdEnd`). Stripes
//!   (`MakeDrawHistoryStripeCommand`, `MakeDrawConnectionStripeCommand`)
//!   write two stripe vertices per uniform block the same way.
//! - So the per-frame particle state the GPU sees is those uniform blocks,
//!   computed by the game's own (natively recompiled) code. The vertex
//!   program (the PTCL resource's shader, picked per emitter by
//!   `Resource::GetShader`) turns slot index and corner into a billboard,
//!   directional quad or stripe vertex.
//!
//! Natively: overrides/gfx_eft.c reports each MakeDrawEmitterSetCommand
//! call's RenderState and the words it wrote. Those words are the eft
//! encoders' fixed output: shader and material blobs copied from the
//! resource, uniform blocks, kicks. `decode` applies them to a `Setup` (the
//! renderer's parameter-word state, as the H3D path decodes BCH blobs) on
//! top of what the RenderState holds, and at each kick takes the draw: the
//! game's eft vertex program, its uniform file, the common quad buffer's
//! layout, and the fragment configuration (combiners, blend, alpha and
//! depth test, texture units), drawn by the renderer's shaded pipeline
//! (shade.vert running the game's program, raster.frag the PICA fragment
//! stages) at the place of its scene in the frame. When the frame is drawn
//! natively in full the words are taken back out of the command list, so
//! the emulated GPU never draws them.

use std::sync::Arc;

use zakuro_core::System;

use crate::prim::{self, r32, Draw};
use crate::setup::{self, Setup};

/// what the eft path did, for FATES_GFX_REPORT.
#[derive(Default, Debug, Clone)]
pub struct Stats {
    /// MakeDrawEmitterSetCommand calls seen, and those drawn natively
    pub sets: u64,
    pub drawn_sets: u64,
    /// kicks (draws) decoded, and drawn natively
    pub kicks: u64,
    pub drawn_kicks: u64,
    /// words decoded
    pub words: u64,
    /// command-buffer jumps met in eft words (not expected)
    pub jumps: u64,
    /// kicks whose vertex input could not be read
    pub failed: u64,
    /// sets whose words held a malformed command (decoding stopped there)
    pub malformed: u64,
}

/// One kick: the state at the draw trigger.
pub struct Kick {
    pub setup: Setup,
    pub fixed: [[f32; 4]; 16],
    pub indexed: bool,
}

pub struct Decoded {
    pub kicks: Vec<Kick>,
    pub jumps: u32,
    /// the word a command naming no PICA register stopped decoding at
    pub malformed: Option<usize>,
}

fn f24(v: u32) -> f32 {
    let v = v & 0xFF_FFFF;
    let sign = (v >> 23) & 1;
    let exp = (v >> 16) & 0x7F;
    if exp == 0 {
        return if sign != 0 { -0.0 } else { 0.0 };
    }
    let mant = v & 0xFFFF;
    f32::from_bits((sign << 31) | ((exp + 64) << 23) | (mant << 7))
}

struct Decoder {
    s: Setup,
    fixed: [[f32; 4]; 16],
    fixed_index: usize,
    staging: Vec<u32>,
    kicks: Vec<Kick>,
    jumps: u32,
}

impl Decoder {
    fn write(&mut self, r: usize, v: u32, mask: u32) {
        match r {
            0x232 => {
                self.fixed_index = (v & 0xF) as usize;
                self.staging.clear();
                self.s.write(r, v, mask);
            }
            0x233..=0x235 => {
                self.staging.push(v);
                if self.staging.len() == 3 {
                    let (a, b, c) = (self.staging[0], self.staging[1], self.staging[2]);
                    self.staging.clear();
                    let value = [f24(c & 0xFF_FFFF), f24(((b & 0xFFFF) << 8) | (c >> 24)), f24(((a & 0xFF) << 16) | (b >> 16)), f24(a >> 8)];
                    if self.fixed_index < 15 {
                        self.fixed[self.fixed_index] = value;
                        self.fixed_index += 1;
                    }
                }
            }
            0x22E | 0x22F => self.kicks.push(Kick { setup: self.s.clone(), fixed: self.fixed, indexed: r == 0x22F }),
            0x23C | 0x23D => self.jumps += 1,
            _ => self.s.write(r, v, mask),
        }
    }
}

/// The draws a run of eft command words makes, on top of `base`.
pub fn decode(words: &[u32], base: &Setup) -> Decoded {
    let mut d = Decoder { s: base.clone(), fixed: [[0.0, 0.0, 0.0, 1.0]; 16], fixed_index: 0, staging: Vec::new(), kicks: Vec::new(), jumps: 0 };
    let mut i = 0usize;
    let mut malformed = None;
    while i + 1 < words.len() {
        let header = words[i + 1];
        let id = (header & 0xFFFF) as usize;
        // a command naming no register (the file is 0x300 words): some
        // emitters' runtime-built material blobs hold one (FFFFFFFF,
        // 32AB8E23 in the prologue's map fire). Its length would carry
        // the parse into the middle of later commands; the emulated GPU
        // stops the list there, and so does this
        if id >= setup::REGS {
            malformed = Some(i);
            break;
        }
        let mask = (header >> 16) & 0xF;
        let extra = ((header >> 20) & 0xFF) as usize;
        let consecutive = header >> 31 != 0;
        d.write(id, words[i], mask);
        let mut r = id;
        for k in 0..extra {
            let Some(&v) = words.get(i + 2 + k) else { break };
            if consecutive {
                r += 1;
            }
            d.write(r, v, mask);
        }
        i += 2 + extra + (extra & 1);
    }
    Decoded { kicks: d.kicks, jumps: d.jumps, malformed }
}

/// A kick's vertex input as shade.vert takes it: per input register the
/// start in `vertices`, the stride and offset | type << 8 | count << 16,
/// the defaults, and the indices (rebased to the first vertex copied).
pub struct Input {
    pub attributes: [[u32; 4]; 16],
    pub defaults: [[f32; 4]; 16],
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    /// the array base and first loader's offset, as zakuro-gpu's oracle
    /// names a draw's buffer
    pub buffer: u32,
}

/// the PICA's attribute loaders and formats, as zakuro-gpu's raster.rs
/// lays out a vertex: per loader its offset, stride and fields (attribute,
/// offset in the vertex, type, count).
fn layout(r: &[u32]) -> Vec<(u32, u32, u32, Vec<(usize, u32, u32, u32)>)> {
    let format = r[0x201] as u64 | ((r[0x202] as u64) << 32);
    let mut out = Vec::new();
    for l in 0..12 {
        let base = 0x203 + l * 3;
        let (w1, w2) = (r[base + 1], r[base + 2]);
        let components = w1 as u64 | (((w2 & 0xFFFF) as u64) << 32);
        let stride = (w2 >> 16) & 0xFF;
        let count = (w2 >> 28) & 0xF;
        if count == 0 {
            continue;
        }
        let mut offset = 0u32;
        let mut fields = Vec::new();
        for slot in 0..count.min(12) {
            let id = ((components >> (slot * 4)) & 0xF) as usize;
            if id >= 12 {
                offset = offset.next_multiple_of(4) + (id as u32 - 11) * 4;
                continue;
            }
            let nibble = ((format >> (id * 4)) & 0xF) as u32;
            let (ty, n) = (nibble & 3, (nibble >> 2) + 1);
            let size = match ty {
                0 | 1 => 1,
                2 => 2,
                _ => 4,
            };
            offset = offset.next_multiple_of(size);
            fields.push((id, offset, ty, n));
            offset += size * n;
        }
        out.push((r[base] & 0x0FFF_FFFF, stride, offset, fields));
    }
    out
}

/// reads a kick's vertices and indices from guest memory.
pub fn input(system: &mut System, k: &Kick) -> Option<Input> {
    let r = &k.setup.regs;
    let base = (r[0x200] & 0x1FFF_FFFF).wrapping_mul(8);
    let count = r[0x228] as usize;
    if count == 0 || count > 0x10000 {
        return None;
    }
    let raw: Vec<u32> = if k.indexed {
        let idx = r[0x227];
        let at = prim::virt(system, base.wrapping_add(idx & 0x0FFF_FFFF));
        if idx >> 31 != 0 {
            let mut b = vec![0u8; count * 2];
            prim::rd(system, at, &mut b);
            b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect()
        } else {
            let mut b = vec![0u8; count];
            prim::rd(system, at, &mut b);
            b.iter().map(|&v| v as u32).collect()
        }
    } else {
        let first = r[0x22A];
        (first..first + count as u32).collect()
    };
    let (low, high) = (*raw.iter().min()?, *raw.iter().max()?);
    let loaders = layout(r);
    let buffer = base.wrapping_add(r[0x203]);
    // which attribute each input register reads (the shader's input map)
    let n = (((r[0x2B9] & 0xF) + 1) as usize).min(12);
    let map = r[0x2BB] as u64 | ((r[0x2BC] as u64) << 32);
    let fixed_mask = (r[0x202] >> 16) & 0xFFF;
    let mut attributes = [[0u32; 4]; 16];
    let mut defaults = [[0f32; 4]; 16];
    let mut vertices = Vec::new();
    let mut starts = Vec::new();
    for (offset, stride, size, _) in &loaders {
        let from = base.wrapping_add(*offset).wrapping_add(low * stride);
        let len = ((high - low) * stride + size) as usize;
        if len > 0x40_0000 {
            return None;
        }
        let start = vertices.len() as u32;
        let mut bytes = vec![0u8; len];
        let at = prim::virt(system, from);
        system.sync_gpu(from, len as u32);
        prim::rd(system, at, &mut bytes);
        vertices.extend_from_slice(&bytes);
        while vertices.len() % 4 != 0 {
            vertices.push(0);
        }
        starts.push(start);
    }
    vertices.extend_from_slice(&[0; 8]);
    for id in 0..n {
        let register = ((map >> (id * 4)) & 0xF) as usize;
        // the last loader giving the attribute wins, as in raster.rs
        let mut source = None;
        for (l, (_, stride, _, fields)) in loaders.iter().enumerate() {
            for &(fid, off, ty, cnt) in fields {
                if fid == id {
                    source = Some((starts[l], *stride, off, ty, cnt));
                }
            }
        }
        match source {
            Some((start, stride, off, ty, cnt)) => {
                attributes[register] = [start, stride, off | ty << 8 | cnt << 16, 0];
                defaults[register] = [0.0, 0.0, 0.0, 1.0];
            }
            None if fixed_mask & (1 << id) != 0 => defaults[register] = k.fixed[id],
            None => defaults[register] = [0.0, 0.0, 0.0, 1.0],
        }
    }
    let indices = raw.iter().map(|&i| i - low).collect();
    Some(Input { attributes, defaults, vertices, indices, buffer })
}

/// PICA primitive mode (0x25E bits 8-9) as the shaded pipeline's topology.
fn topology(r: &[u32]) -> u8 {
    match (r[0x25E] >> 8) & 3 {
        1 => 1,
        2 => 2,
        _ => 0,
    }
}

/// What an eft draw inherits from the draws before it: the eft material
/// blobs write only part of the fog register (0x0E0), so on the GPU a
/// particle keeps the fog mode, colour and table the last fog write left.
/// Those are a mesh material's fog (setup.rs `fog`, MakeFogCommand, which
/// the native mesh draws record here) and `Pica::FogConfig` (gfx_eft.c),
/// which the fixed state calls: the 2D layer before a scene, and the
/// map's unit sprites, gauges, icons and cursor inside the scene pass
/// (prim::BeginCommand -> prim::Context::SetupFixedState). The GPU's state
/// is one for all scenes, and so is this.
#[derive(Default)]
pub struct Inherit {
    regs: Option<[u32; 2]>,
    fog: Vec<u32>,
    /// this pass's eft word ranges (from, words' hash, bytes), checked
    /// again when the pass is done
    pending: Vec<(u32, u64, u32)>,
}

impl Inherit {
    /// a mesh draw with setup `s`.
    pub fn mesh(&mut self, s: &Setup) {
        self.regs = Some([s.regs[0x0E0], s.regs[0x0E1]]);
        if s.fog_known {
            self.fog.clone_from(&s.fog);
        }
    }

    /// Pica::FogConfig's two parameter words with their headers' byte
    /// masks (the table stays).
    fn fog_config(&mut self, e0: u32, mask0: u32, e1: u32, mask1: u32) {
        let bytes = |m: u32| -> u32 { (0..4).filter(|b| m >> b & 1 != 0).map(|b| 0xFFu32 << (8 * b)).sum() };
        let [o0, o1] = self.regs.unwrap_or([0, 0]);
        let (b0, b1) = (bytes(mask0), bytes(mask1));
        self.regs = Some([(o0 & !b0) | (e0 & b0), (o1 & !b1) | (e1 & b1)]);
    }

    fn apply(&self, s: &mut Setup) {
        let Some([mode, color]) = self.regs else { return };
        s.set(0x0E0, mode);
        s.set(0x0E1, color);
        if !self.fog.is_empty() {
            s.fog.clone_from(&self.fog);
            s.fog_known = true;
        }
    }
}

/// the effect state RenderState::Change(2) leaves for the eft words of a
/// RenderState drawing into display `disp`.
pub fn base(system: &mut System, state: u32, disp: usize) -> Setup {
    let mut s = Setup::default();
    let camera = r32(system, state + 8);
    setup::render_state_uniforms(&mut s, system, camera);
    setup::target(&mut s, disp);
    // the scissor the 2D layer's target leaves on: the whole display, as
    // the GPU has it when the scene draws (equivalent to none)
    s.set(0x065, 3);
    s.set(0x066, 0);
    s.set(0x067, if disp == 0 { 0x018F_00EF } else { 0x013F_00EF });
    s
}

/// a native Draw carrying a shaded eft draw.
fn draw_of(shaded: crate::scene::Shaded) -> Draw {
    Draw {
        vertices: Vec::new(),
        indices: Vec::new(),
        world: [0.0; 12],
        view_proj: [0.0; 16],
        tex_matrix: [0.0; 12],
        color: [1.0; 4],
        textures: shaded.textures,
        combiner: prim::Combiner::prim_reset(),
        blend: prim::Blend::prim(7),
        depth_test: false,
        depth_func: 0,
        depth_write: false,
        alpha_test: false,
        alpha_func: 0,
        alpha_ref: 0,
        cull: 0,
        scissor: [0, 0, 0, 0],
        viewport: [0, 0, 0, 0],
        shaded: Some(Arc::new(shaded)),
        post: None,
        depth_offset: 0.0, // gfx-gaps
    }
}

fn raw_words(system: &mut System, from: u32, bytes: u32) -> Vec<u32> {
    let mut raw = vec![0u8; bytes as usize];
    prim::rd(system, from, &mut raw);
    raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn env_flag(name: &str) -> bool {
    crate::perf::var_os(name).is_some() // gfx-perf: cached env
}

impl crate::Gfx {
    /// MakeDrawEmitterSetCommand wrote [from, to) for RenderState `state`
    /// (0 when unknown). Returns nonzero when the words are drawn natively
    /// and should be taken out of the command list.
    pub(crate) fn eft_draw(&mut self, system: &mut System, state: u32, from: u32, to: u32) -> i32 {
        self.eft.sets += 1;
        let bytes = to.wrapping_sub(from);
        if state == 0 || bytes == 0 || bytes > 0x10_0000 || bytes % 4 != 0 {
            return 0;
        }
        let scene = r32(system, state + 0xC);
        let island = self.islands.as_ref().filter(|i| i.slots.iter().any(|(o, _)| *o == scene)).map(|i| (i.disp, i.compacted));
        let oracle = crate::perf::var("FATES_GFX_ORACLE").ok().filter(|_| crate::perf::var_os("FATES_GFX_ORACLE_DIR").is_some()); // gfx-perf: cached env
        let trace = env_flag("FATES_GFX_EFT_TRACE");
        if island.is_none() && oracle.is_none() && !trace {
            return 0;
        }
        let disp = match island {
            Some((d, _)) => d,
            None => (self.target != 0x400 && self.target != 0x410) as usize,
        };
        let mut raw = vec![0u8; bytes as usize];
        prim::rd(system, from, &mut raw);
        let words: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        self.eft.words += words.len() as u64;
        if env_flag("FATES_GFX_EFT_CHECK") {
            self.eft_inherit.pending.push((from, crate::fnv(&raw), bytes));
        }
        let mut base = base(system, state, disp);
        self.eft_inherit.apply(&mut base);
        let decoded = decode(&words, &base);
        self.eft.kicks += decoded.kicks.len() as u64;
        self.eft.jumps += decoded.jumps as u64;
        self.eft.malformed += decoded.malformed.is_some() as u64;
        if crate::perf::var_os("FATES_GFX_LISTS").is_some() { // gfx-perf: cached env
            let p = zakuro_core::services::gsp::virtual_to_physical(system, from);
            let w: Vec<u32> = raw_words(system, from, bytes);
            let sum = w.iter().fold(0u64, |h, &x| h.wrapping_mul(31).wrapping_add(x as u64));
            eprintln!("gfx-eft-range frame {} {p:08X} {bytes:X} sum {sum:016X} first {:08X?}", self.stats.frames, &w[..w.len().min(6)]);
        }
        if trace {
            if let Some(i) = self.islands.as_ref() {
                eprintln!("gfx-eft islands {:X?} inherit {:?}", i.slots, self.eft_inherit.regs);
            }
            let camera = r32(system, state + 8);
            let fl = |system: &mut System, a: u32| f32::from_bits(r32(system, a));
            eprintln!(
                "gfx-eft camera {camera:08X} mode {} m148.w {:.1} m98.w {:.1} pos {:.1}",
                prim::r8(system, state + 4),
                fl(system, camera + 0x148 + 12),
                fl(system, camera + 0x98 + 12),
                fl(system, camera + 0x68)
            );
            eprintln!(
                "gfx-eft frame {} disp {disp} state {state:08X} scene {scene:08X} words {} kicks {} jumps {} malformed {:?} island {:?}",
                self.stats.frames,
                words.len(),
                decoded.kicks.len(),
                decoded.jumps,
                decoded.malformed,
                island
            );
        }
        let mut made = Vec::new();
        for k in &decoded.kicks {
            let Some(inp) = input(system, k) else {
                self.eft.failed += 1;
                continue;
            };
            if let (Some(want), Some(dir)) = (oracle.as_deref(), crate::perf::var_os("FATES_GFX_ORACLE_DIR")) { // gfx-perf: cached env
                if want == "all" || u32::from_str_radix(want, 16).is_ok_and(|w| w == inp.buffer) {
                    let env = |name: &str, default: u32| crate::perf::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default); // gfx-perf: cached env
                    let (skip, per) = (env("FATES_GFX_ORACLE_SKIP", 0), env("FATES_GFX_ORACLE_PER", 2));
                    // fewer native dumps than GPU ones, so each finds its
                    // pair (the GPU counts the upper screen twice)
                    let per = env("FATES_GFX_ORACLE_NATIVE_PER", per);
                    let skip = env("FATES_GFX_ORACLE_NATIVE_SKIP", skip);
                    let count = self.oracle_seen.entry(inp.buffer).or_insert(0);
                    let n = *count;
                    *count += 1;
                    if n >= skip && n < skip + per {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("native-{:08X}-{}.bin", inp.buffer, n - skip)), k.setup.dump());
                    }
                }
            }
            if trace {
                let r = &k.setup.regs;
                eprintln!(
                    "gfx-eft   kick n {} indexed {} buffer {:08X} prim {} entry {} tex {:08X} {:08X} blend {:08X} {:08X} alpha {:08X} depth {:08X} verts {} attrs {:?}",
                    inp.indices.len(),
                    k.indexed,
                    inp.buffer,
                    (r[0x25E] >> 8) & 3,
                    k.setup.entry,
                    r[0x080],
                    r[0x085] << 3,
                    r[0x100],
                    r[0x101],
                    r[0x104],
                    r[0x107],
                    inp.vertices.len(),
                    &inp.attributes[..4]
                );
            }
            if island.is_none() {
                continue;
            }
            let program = crate::cache::program(&k.setup); // gfx-perf: shared by content
            let virt = |phys: u32| prim::virt(system, phys);
            let mut shaded = crate::scene::shaded_from(&k.setup, topology(&k.setup.regs), &inp.attributes, &inp.defaults, inp.indices, Arc::new(inp.vertices), program, &virt);
            // dev: FATES_GFX_EFT_DEBUG=nodepth|noblend
            match crate::perf::var("FATES_GFX_EFT_DEBUG").as_deref() { // gfx-perf: cached env
                Ok("nodepth") => shaded.key.depth_test = false,
                Ok("noblend") => shaded.key.blend = None,
                _ => {}
            }
            shaded.loose = true; // gfx-hfr: particles' vertices change each frame
            shaded.placing.1 |= shaded.placing.0 & crate::hfr::particle_texture_shared(&k.setup, &shaded.program); // gfxleft: flip-book cells step
            let mut keys = [None; 3];
            for unit in 0..3 {
                if let Some((tex, _)) = shaded.textures[unit] {
                    keys[unit] = self.texture_oriented(system, tex, false);
                }
            }
            made.push((draw_of(shaded), keys));
        }
        let Some((_, compacted)) = island else { return 0 };
        self.eft.drawn_sets += 1;
        self.eft.drawn_kicks += made.len() as u64;
        crate::gfx3::trace_made(&mut made, zakuro_gpu::wide::factor()); // gfx3: dev
        if let Some(islands) = self.islands.as_mut() {
            islands.draws.entry(scene).or_default().extend(made);
        }
        // FATES_GFX_EFT=keep: draw natively but leave the words to the
        // emulated GPU too
        (compacted && crate::perf::var("FATES_GFX_EFT").map_or(true, |v| v != "keep")) as i32 // gfx-perf: cached env
    }
}

impl crate::Gfx {
    /// FATES_GFX_EFT_CHECK: after a display's scene pass, whether each eft
    /// word range still holds what was decoded (dev check).
    pub(crate) fn eft_check(&mut self, system: &mut System, at: &str, keep: bool) {
        let pending = if keep { self.eft_inherit.pending.clone() } else { std::mem::take(&mut self.eft_inherit.pending) };
        if pending.is_empty() {
            return;
        }
        let mut changed = 0;
        for &(from, hash, bytes) in &pending {
            let mut raw = vec![0u8; bytes as usize];
            prim::rd(system, from, &mut raw);
            if crate::fnv(&raw) != hash {
                changed += 1;
            }
        }
        eprintln!("gfx-eft-check {at} frame {} sets {} changed {}", self.stats.frames, pending.len(), changed);
    }
}

impl crate::Gfx {
    /// Pica::FogConfig(on, flip, r, g, b) (gfx_eft.c): the words it writes,
    /// with the headers from its literal pool.
    pub(crate) fn eft_fog(&mut self, system: &mut System, on: u32, flip: u32, r: u32, g: u32, b: u32) {
        let e0 = if on & 0xFF != 0 { 5 } else { 0 } | (flip & 0xFF) << 16;
        let e1 = (r & 0xFF) | (g & 0xFF) << 8 | (b & 0xFF) << 16;
        let (h0, h1) = (r32(system, 0x0011_B8A8), r32(system, 0x0011_B8AC));
        if (h0 & 0xFFFF) == 0x0E0 && (h1 & 0xFFFF) == 0x0E1 {
            self.eft_inherit.fog_config(e0, (h0 >> 16) & 0xF, e1, (h1 >> 16) & 0xF);
        }
    }
}
