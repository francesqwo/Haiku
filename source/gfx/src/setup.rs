//! The state one H3D mesh draw needs, built natively from the engine's
//! objects: the vertex program and its uniform file, the fragment stages'
//! configuration (combiners, blending, depth, texture units, fragment
//! lighting) and the lighting tables.
//!
//! The configuration is held as the PICA's fixed-function parameter words,
//! the encoding the game's resources ship it in and the one the native
//! fragment shader (zakuro-gpu's raster.frag) takes. It is never read from
//! a command list the game built: the static parts are decoded once from
//! the BCH resource blobs that hold them (as SPICA's deserializer decodes
//! them), and everything that changes from draw to draw is computed here
//! from engine state, porting what Model::DrawMesh (0x0041DA20) and the
//! nw::h3d / nn::gr functions it calls would have encoded.

use zakuro_core::System;

use crate::prim::{r16, r32, r8};

pub const REGS: usize = 0x300;
pub const FLOATS: usize = 96;
pub const PROGRAM: usize = 4096;
pub const DESCRIPTORS: usize = 128;
pub const LUTS: usize = 24;

/// PICA registers with side effects (uploads through a cursor).
mod reg {
    pub const LIGHTING_LUT_INDEX: usize = 0x1C5;
    pub const LIGHTING_LUT_DATA0: usize = 0x1C8;
    pub const LIGHTING_LUT_DATA7: usize = 0x1CF;
    pub const VSH_BOOLUNIFORM: usize = 0x2B0;
    pub const VSH_INTUNIFORM_I0: usize = 0x2B1;
    pub const VSH_INTUNIFORM_I3: usize = 0x2B4;
    pub const VSH_ENTRYPOINT: usize = 0x2BA;
    pub const VSH_FLOATUNIFORM_INDEX: usize = 0x2C0;
    pub const VSH_FLOATUNIFORM_DATA0: usize = 0x2C1;
    pub const VSH_FLOATUNIFORM_DATA7: usize = 0x2C8;
    pub const VSH_CODETRANSFER_CONFIG: usize = 0x2CB;
    pub const VSH_CODETRANSFER_DATA0: usize = 0x2CC;
    pub const VSH_CODETRANSFER_DATA7: usize = 0x2D3;
    pub const VSH_OPDESCS_CONFIG: usize = 0x2D5;
    pub const VSH_OPDESCS_DATA0: usize = 0x2D6;
    pub const VSH_OPDESCS_DATA7: usize = 0x2DD;
    /// command-buffer jumps and the end marker, which say nothing about
    /// the draw
    pub const CMDBUF_FIRST: usize = 0x238;
    pub const CMDBUF_LAST: usize = 0x23D;
}

/// One draw's state. `known` marks what this module has defined; the rest
/// is whatever the fragment shader's defaults are.
#[derive(Clone)]
pub struct Setup {
    pub regs: Vec<u32>,
    pub known: Vec<bool>,
    pub floats: Vec<[f32; 4]>,
    pub floats_known: Vec<bool>,
    pub ints: [u32; 4],
    pub bools: u16,
    pub entry: u32,
    /// perf3: shared until written (Arc::make_mut), so a sub-mesh's copy
    /// of its mesh's setup does not copy the 16 KB program
    pub program: std::sync::Arc<Vec<u32>>,
    pub descriptors: Vec<u32>,
    pub program_known: bool,
    /// the lighting tables, raw 0x1C8 words, 256 per table
    /// perf3: shared until written, as `program` (24 KB)
    pub luts: std::sync::Arc<Vec<u32>>,
    pub luts_known: Vec<bool>,
    /// the fog table, raw 0xE8 words
    pub fog: Vec<u32>,
    pub fog_known: bool,
    fog_at: usize,
    float_f32: bool,
    float_index: usize,
    float_pending: Vec<u32>,
    program_at: usize,
    descriptor_at: usize,
    /// gfxmodels3: the float uniforms holding the texture coordinates'
    /// matrices (bit k for uniform k), see mutable
    pub texture_rows: u128,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            regs: vec![0; REGS],
            known: vec![false; REGS],
            floats: vec![[0.0; 4]; FLOATS],
            floats_known: vec![false; FLOATS],
            ints: [0; 4],
            bools: 0,
            entry: 0,
            program: zeros(PROGRAM), // perf3
            descriptors: vec![0; DESCRIPTORS],
            program_known: false,
            luts: zeros(LUTS * 256), // perf3
            luts_known: vec![false; LUTS],
            fog: vec![0; 128],
            fog_known: false,
            fog_at: 0,
            float_f32: false,
            float_index: 0,
            float_pending: Vec::new(),
            program_at: 0,
            descriptor_at: 0,
            texture_rows: 0, // gfxmodels3
        }
    }
}

/// perf3: a shared list of `n` zeros (a fresh setup's program and tables,
/// copied only when something is written into them).
fn zeros(n: usize) -> std::sync::Arc<Vec<u32>> {
    static PROGRAM_ZEROS: std::sync::OnceLock<std::sync::Arc<Vec<u32>>> = std::sync::OnceLock::new();
    static LUT_ZEROS: std::sync::OnceLock<std::sync::Arc<Vec<u32>>> = std::sync::OnceLock::new();
    let cell = if n == PROGRAM { &PROGRAM_ZEROS } else { &LUT_ZEROS };
    let z = cell.get_or_init(|| std::sync::Arc::new(vec![0; n]));
    if z.len() == n { z.clone() } else { std::sync::Arc::new(vec![0; n]) }
}

/// a float24 as the PICA's uniform uploads pack it.
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

impl Setup {
    /// one parameter word, with the byte mask the encoding carries.
    pub fn write(&mut self, r: usize, value: u32, mask: u32) {
        if r >= REGS || (reg::CMDBUF_FIRST..=reg::CMDBUF_LAST).contains(&r) {
            return;
        }
        let bytes: u32 = (0..4).filter(|b| mask >> b & 1 != 0).map(|b| 0xFFu32 << (8 * b)).sum();
        let value = (self.regs[r] & !bytes) | (value & bytes);
        self.regs[r] = value;
        self.known[r] = true;
        match r {
            reg::VSH_FLOATUNIFORM_INDEX => {
                self.float_index = (value & 0xFF) as usize;
                self.float_f32 = value >> 31 != 0;
                self.float_pending.clear();
            }
            reg::VSH_FLOATUNIFORM_DATA0..=reg::VSH_FLOATUNIFORM_DATA7 => {
                self.float_pending.push(value);
                let want = if self.float_f32 { 4 } else { 3 };
                if self.float_pending.len() == want {
                    let p = std::mem::take(&mut self.float_pending);
                    let v = if self.float_f32 {
                        [f32::from_bits(p[3]), f32::from_bits(p[2]), f32::from_bits(p[1]), f32::from_bits(p[0])]
                    } else {
                        let (a, b, c) = (p[0], p[1], p[2]);
                        let w = a >> 8;
                        let z = ((a & 0xFF) << 16) | (b >> 16);
                        let y = ((b & 0xFFFF) << 8) | (c >> 24);
                        let x = c & 0xFF_FFFF;
                        [f24(x), f24(y), f24(z), f24(w)]
                    };
                    self.set_float(self.float_index, v);
                    self.float_index += 1;
                }
            }
            reg::VSH_BOOLUNIFORM => self.bools = value as u16,
            reg::VSH_INTUNIFORM_I0..=reg::VSH_INTUNIFORM_I3 => self.ints[r - reg::VSH_INTUNIFORM_I0] = value,
            reg::VSH_ENTRYPOINT => self.entry = value & 0xFFFF,
            reg::VSH_CODETRANSFER_CONFIG => self.program_at = (value & 0xFFF) as usize,
            reg::VSH_CODETRANSFER_DATA0..=reg::VSH_CODETRANSFER_DATA7 => {
                if self.program_at < PROGRAM {
                    std::sync::Arc::make_mut(&mut self.program)[self.program_at] = value; // perf3
                    self.program_known = true;
                }
                self.program_at += 1;
            }
            reg::VSH_OPDESCS_CONFIG => self.descriptor_at = (value & 0x7F) as usize,
            reg::VSH_OPDESCS_DATA0..=reg::VSH_OPDESCS_DATA7 => {
                if self.descriptor_at < DESCRIPTORS {
                    self.descriptors[self.descriptor_at] = value;
                }
                self.descriptor_at += 1;
            }
            0xE6 => self.fog_at = value as usize,
            0xE8..=0xEF => {
                self.fog[self.fog_at % 128] = value;
                self.fog_known = true;
                self.fog_at += 1;
            }
            reg::LIGHTING_LUT_DATA0..=reg::LIGHTING_LUT_DATA7 => {
                let index = self.regs[reg::LIGHTING_LUT_INDEX];
                let (table, entry) = (((index >> 8) & 0x1F) as usize, (index & 0xFF) as usize);
                if table < LUTS {
                    std::sync::Arc::make_mut(&mut self.luts)[table * 256 + entry] = value; // perf3
                    self.luts_known[table] = true;
                }
                self.regs[reg::LIGHTING_LUT_INDEX] = (index & !0xFF) | ((entry as u32 + 1) & 0xFF);
            }
            _ => {}
        }
    }

    pub fn set(&mut self, r: usize, value: u32) {
        self.write(r, value, 0xF);
    }

    pub fn set_float(&mut self, index: usize, v: [f32; 4]) {
        if index < FLOATS {
            self.floats[index] = v;
            self.floats_known[index] = true;
        }
    }

    /// a 3x4 matrix (row-major) into three uniforms from `index` on.
    pub fn set_mtx34(&mut self, index: usize, m: &[f32; 12]) {
        for row in 0..3 {
            self.set_float(index + row, [m[row * 4], m[row * 4 + 1], m[row * 4 + 2], m[row * 4 + 3]]);
        }
    }

    pub fn set_mtx44(&mut self, index: usize, m: &[f32; 16]) {
        for row in 0..4 {
            self.set_float(index + row, [m[row * 4], m[row * 4 + 1], m[row * 4 + 2], m[row * 4 + 3]]);
        }
    }

    /// a resource blob of parameter words (a BCH command blob: parameter,
    /// header with id, byte mask, extra count and the consecutive bit, the
    /// extra parameters, padding to 8 bytes), applied in order.
    pub fn blob(&mut self, system: &mut System, addr: u32, words: u32) {
        if addr == 0 || words == 0 {
            return;
        }
        let words = words.min(0x4000);
        let mut raw = vec![0u8; words as usize * 4];
        crate::prim::rd(system, addr, &mut raw);
        let w: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        self.words(&w);
    }

    /// the blob a (pointer, word count) pair at `at` names.
    pub fn blob_at(&mut self, system: &mut System, at: u32) {
        let (addr, words) = (r32(system, at), r32(system, at + 4));
        self.blob(system, addr, words);
    }

    pub fn words(&mut self, w: &[u32]) {
        let mut i = 0usize;
        while i + 1 < w.len() {
            let header = w[i + 1];
            let id = (header & 0xFFFF) as usize;
            let mask = (header >> 16) & 0xF;
            let extra = ((header >> 20) & 0xFF) as usize;
            let consecutive = header >> 31 != 0;
            self.write(id, w[i], mask);
            let mut r = id;
            for k in 0..extra {
                let Some(&v) = w.get(i + 2 + k) else { break };
                if consecutive {
                    r += 1;
                }
                self.write(r, v, mask);
            }
            i += 2 + extra + (extra & 1);
        }
    }

    /// the state as a file for dev/gfx/oracle.py, laid out as zakuro-gpu's
    /// FATES_GFX_ORACLE_DIR dump, then what is known.
    pub fn dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut put = |w: u32| out.extend_from_slice(&w.to_le_bytes());
        put(REGS as u32);
        self.regs.iter().for_each(|&w| put(w));
        self.floats.iter().flatten().for_each(|f| put(f.to_bits()));
        self.ints.iter().for_each(|&i| put(i));
        put(self.bools as u32);
        put(self.entry);
        self.program.iter().for_each(|&w| put(w));
        self.descriptors.iter().for_each(|&w| put(w));
        // the tables as zakuro-gpu holds them, a value and a step
        for &v in self.luts.iter() {
            let step = ((v >> 12) & 0x7FF) as f32 / 2047.0;
            let step = if v & (1 << 23) != 0 { -step } else { step };
            put(((v & 0xFFF) as f32 / 4095.0).to_bits());
            put(step.to_bits());
        }
        for &v in &self.fog {
            let step = ((v & 0x1FFF) as i32) << 19 >> 19;
            put((((v >> 13) & 0x7FF) as f32 / 2048.0).to_bits());
            put((step as f32 / 2048.0).to_bits());
        }
        out.extend(self.known.iter().map(|&k| k as u8));
        out.extend(self.floats_known.iter().map(|&k| k as u8));
        out.extend(self.luts_known.iter().map(|&k| k as u8));
        out.push(self.program_known as u8);
        out.push(self.fog_known as u8);
        out
    }
}

fn m34(system: &mut System, addr: u32) -> [f32; 12] {
    std::array::from_fn(|i| f32::from_bits(r32(system, addr + 4 * i as u32)))
}

/// gfx-models: skeleton matrices a sub-mesh draw read that place nothing
/// (a zero rotation part, numbers not finite or past any scene's size):
/// the vertices bound to them collapse to the scene's origin, as beams and
/// missing limbs. Counted for the report line.
pub static BAD_BONES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_bone(m: &[f32; 12]) {
    let zero = [0usize, 1, 2, 4, 5, 6, 8, 9, 10].iter().all(|&i| m[i] == 0.0);
    if zero || m.iter().any(|v| !v.is_finite() || v.abs() > 1.0e6) {
        BAD_BONES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

fn m44(system: &mut System, addr: u32) -> [f32; 16] {
    std::array::from_fn(|i| f32::from_bits(r32(system, addr + 4 * i as u32)))
}

/// row-major 3x4 product a * b (b's fourth row being 0 0 0 1).
pub fn mul34(a: &[f32; 12], b: &[f32; 12]) -> [f32; 12] {
    let mut o = [0f32; 12];
    for r in 0..3 {
        for c in 0..4 {
            o[r * 4 + c] = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c] + if c == 3 { a[r * 4 + 3] } else { 0.0 };
        }
    }
    o
}

fn f(system: &mut System, addr: u32) -> f32 {
    f32::from_bits(r32(system, addr))
}

/// a word from the game's literal pools: the parameter headers its
/// encoders use, read rather than copied so they are the game's own.
fn lit(system: &mut System, addr: u32) -> u32 {
    r32(system, zakuro_core::region::us(addr)) // eur: a USA literal, read where the build has it
}

const IDENTITY34: [f32; 12] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];

/// Where a DrawMesh call's objects are.
#[derive(Clone, Copy, Debug)]
pub struct Objects {
    pub model: u32,
    pub state: u32,
    pub mesh: u32,
    pub world: u32,
    /// the mesh's model resource (H3DModel)
    pub res_model: u32,
    /// the material resource (H3DMaterial, 0x2C bytes) and its parameters
    pub material: u32,
    pub params: u32,
    /// the model's MaterialState (0x158 bytes) and bind state (0x58 bytes)
    pub mstate: u32,
    pub bind: u32,
    /// the camera (RenderState +8) and the SceneState (RenderState +0xC)
    pub camera: u32,
    pub scene: u32,
    /// the translucency layer DrawMesh was given
    pub layer: u8,
}

impl Objects {
    pub fn read(system: &mut System, model: u32, state: u32, layer: u32, mesh: u32, world: u32) -> Objects {
        let index = r16(system, mesh) as u32;
        let res_model = r32(system, mesh + 0x2C);
        let material = r32(system, res_model + 0x34) + index * 0x2C;
        let params = r32(system, material);
        Objects {
            model,
            state,
            mesh,
            world,
            res_model,
            material,
            params,
            mstate: r32(system, model + 0x54) + index * 0x158,
            bind: r32(system, model + 0x64) + index * 0x58,
            camera: r32(system, state + 8),
            scene: r32(system, state + 0xC),
            layer: layer as u8,
        }
    }
}

/// The literal pools of the functions whose registers are not in the
/// objects: the bind tables RenderState::Change (0x001A3140) uploads the
/// projection and the camera matrix through.
const RENDERSTATE_PROJ_BIND: u32 = 0x001A_3340;
const RENDERSTATE_CAMERA_BIND: u32 = 0x001A_334C;

/// The state of a DrawMesh call, up to the sub-meshes: what is common to
/// every sub-mesh of the mesh.
pub fn mesh_setup(system: &mut System, o: &Objects) -> Setup {
    let mut s = Setup::default();
    // the material's shader binding: the program, its constants and its
    // output map (H3DMaterialShader: id, then three blobs; DrawMesh picks
    // the first when the shader changes)
    let shader = r32(system, o.params + 8);
    if shader != 0 {
        let (addr, words) = (r32(system, shader + 8), r32(system, shader + 0xC));
        s.blob(system, addr, words);
        if crate::perf::var_os("FATES_GFX_SETUP_DEBUG").is_some() { // gfx-perf: cached env
            for at in [8u32, 0x10, 0x18] {
                let mut t = Setup::default();
                t.blob_at(system, shader + at);
                let set: Vec<String> = (0..FLOATS).filter(|&i| t.floats_known[i]).map(|i| format!("c{i}={:?}", t.floats[i])).collect();
                let regs: Vec<String> = (0..REGS).filter(|&i| t.known[i] && !(0x2C0..=0x2DD).contains(&i)).map(|i| format!("{i:03X}={:08X}", t.regs[i])).collect();
                eprintln!("gfx-setup shader {shader:08X} +{at:X}: {} | {}", set.join(" "), regs.join(" "));
            }
        }
    }
    render_state_uniforms(&mut s, system, o.camera);
    // DrawMesh: the view (camera +0x98) and an identity world
    let view = m34(system, o.camera + 0x98);
    s.set_mtx34(r8(system, o.bind + 10) as usize, &view);
    s.set_mtx34(r8(system, o.bind + 2) as usize, &IDENTITY34);
    lights_uniform(&mut s, system, o);
    material(&mut s, system, o);
    model_state(&mut s, system, o, o.layer);
    // the mesh's own blob (MeshData::MakeCommand: vertex layout, its
    // scales and offset)
    s.blob_at(system, o.mesh + 8);
    s
}

/// What RenderState::Change (0x001A3140) uploads when a 3D pass starts
/// (gfx-eft: shared with the effect draws, eft.rs): the projection (camera
/// +0x1A8, 4x4) and the camera matrix (+0x148, 3x4) into the registers its
/// bind tables name, and the camera position for the effect shaders
/// (nw::eft::System::MakeCameraPositionCommand 0x00317238).
pub fn render_state_uniforms(s: &mut Setup, system: &mut System, camera: u32) {
    let proj_bind = r32(system, zakuro_core::region::us(RENDERSTATE_PROJ_BIND));
    let cam_bind = r32(system, zakuro_core::region::us(RENDERSTATE_CAMERA_BIND));
    let proj = m44(system, camera + 0x1A8);
    s.set_mtx44(r8(system, proj_bind + 2) as usize, &proj);
    let cam = m34(system, camera + 0x148);
    s.set_mtx34(r8(system, cam_bind + 2) as usize, &cam);
    let eft_index = lit(system, 0x0031_7288);
    let pos = [f(system, camera + 0x68), f(system, camera + 0x6C), f(system, camera + 0x70)];
    s.set_float((eft_index & 0xFF) as usize, [pos[0], pos[1], pos[2], 0.0]);
}

/// The state of one sub-mesh draw: the mesh's, the bone palette
/// (MeshData::MakeCommand 0x0051DD30) or the world matrix, and the
/// sub-mesh's blob (index buffer, booleans).
pub fn submesh_setup(system: &mut System, o: &Objects, base: &Setup, sub: u32) -> Setup {
    let mut s = base.clone();
    if r32(system, o.mesh + 0x14) == 0 {
        // gfx-terrain: a mesh drawn through sub-mesh culling (MeshData::
        // MakeCommand 0x0051E0F8; only unskinned models take that path):
        // DrawMesh's world matrix, and the culling entry's booleans (the
        // header in its literal 0x0051E25C, the upper half 0x7FFF)
        let w = m34(system, o.world);
        s.set_mtx34(r8(system, o.bind + 0x22) as usize, &w);
        let entry = crate::h3d::culling_entry(system, o.mesh, o.res_model);
        let bools = r16(system, entry + 0x18) as u32;
        s.words(&[bools | 0x7FFF_0000, lit(system, 0x0051_E25C)]);
        return s;
    }
    let sa = r32(system, o.mesh + 0x10) + sub * 0x34;
    let palette_reg = r8(system, o.bind + 0x22) as usize;
    let skeleton = r32(system, o.model + 0x24 + 0x14);
    let flags = r8(system, sa);
    let count = r16(system, sa + 2) as u32;
    let skinned = r8(system, o.res_model) & 2 != 0;
    if !skinned {
        // a model without a skeleton: DrawMesh uploads the world matrix
        let w = m34(system, o.world);
        s.set_mtx34(palette_reg, &w);
    } else if flags & 1 != 0 {
        // smooth: each bone times its inverse bind matrix
        let bones = r32(system, o.res_model + 0x70);
        for k in 0..count.min(20) {
            let b = r16(system, sa + 4 + 2 * k) as u32;
            let m = m34(system, skeleton + b * 0x30);
            note_bone(&m); // gfx-models
            let inv = m34(system, bones + b * 100 + 0x2C);
            s.set_mtx34(palette_reg + 3 * k as usize, &mul34(&m, &inv));
        }
    } else if flags & 2 != 0 {
        for k in 0..count.min(20) {
            let b = r16(system, sa + 4 + 2 * k) as u32;
            let m = m34(system, skeleton + b * 0x30);
            note_bone(&m); // gfx-models
            s.set_mtx34(palette_reg + 3 * k as usize, &m);
        }
    } else {
        let b = r16(system, sa + 4) as u32;
        let m = m34(system, skeleton + b * 0x30);
        note_bone(&m); // gfx-models
        s.set_mtx34(palette_reg, &m);
    }
    s.blob_at(system, sa + 0x2C);
    s
}

/// LightSet::MakeUniformCommand (0x00546398): the hemisphere light's
/// ground and sky colours and its direction, into the registers the bind
/// state names (+0x2A, +0x32, +0x3A).
fn lights_uniform(s: &mut Setup, system: &mut System, o: &Objects) {
    let index = r16(system, o.mstate + 0x20) as u32;
    let stride = lit(system, 0x0041_E220) * 4;
    let set = o.scene + index * stride + 0x108;
    let light = r32(system, set + 8);
    if light == 0 {
        return;
    }
    for (k, at) in [(0x2Au32, 0x30u32), (0x32, 0x40), (0x3A, 0x50)] {
        let reg = r8(system, o.bind + k) as usize;
        let v = [f(system, light + at), f(system, light + at + 4), f(system, light + at + 8), f(system, light + at + 12)];
        s.set_float(reg, v);
    }
}

/// The light set a material lights with (SceneState +0x1C, 0x278 bytes
/// each, its nn::gr FragmentLight at +4), copied: SetLightColor writes the
/// material's colours into it, which the copy takes instead.
fn fragment_light(system: &mut System, o: &Objects) -> Vec<u8> {
    let index = r16(system, o.mstate + 0x20) as u32;
    let base = r32(system, o.scene + 0x1C) + index * 0x278;
    let mut copy = vec![0u8; 0x278];
    crate::prim::rd(system, base, &mut copy);
    copy
}

/// MaterialData::SetLightColor (0x0051AE78) on the copy: each enabled
/// light's colours times the material's, the global ambient the emission
/// plus the material's ambient times the scene's.
fn set_light_color(system: &mut System, o: &Objects, l: &mut [u8]) {
    let flags6 = r8(system, o.params + 6);
    let mut m = [0u8; 0x14];
    crate::prim::rd(system, o.mstate + 0x3C, &mut m);
    // m[0..3] emission, m[4..] ambient, m[8..] diffuse, m[0xC..] specular
    // 0, m[0x10..] specular 1 (MaterialState +0x3C on)
    let mul = |c: u8, k: u8| ((c as u32 * k as u32) >> 8) as u8;
    for i in 0..8usize {
        if l[4 + 0x164 + i] == 0 {
            continue;
        }
        // the FragmentLight's source i is at +4 + i * 0x2C of it, so +8 +
        // i * 0x2C of the set; the light's colours at +0x1B4 + i * 0x10
        let src = 8 + i * 0x2C;
        let light = 0x1B4 + i * 0x10;
        for c in 0..3 {
            l[src + 7 + c] = mul(l[light + c], m[4 + c]);
            l[src + 4 + c] = mul(l[light + 4 + c], m[8 + c]);
            l[src + 0xA + c] = mul(l[light + 8 + c], m[0xC + c]);
            l[src + 0xD + c] = if flags6 & 0x10 == 0 { mul(l[light + 0xC + c], m[0x10 + c]) } else { l[light + 0xC + c] };
        }
        l[src + 2] = (flags6 & 0x20) >> 5;
        l[src + 3] = (flags6 & 0x40) >> 6;
    }
    for c in 0..3 {
        let v = m[c] as u32 + ((m[4 + c] as u32 * l[0x234 + c] as u32) >> 8);
        l[4 + c] = v.min(255) as u8;
    }
}

/// The material's state, as MaterialData::MakeCommandWithoutCache
/// (0x0051B9F0) and MakeMutableCommand (0x0051B2DC) encode it.
fn material(s: &mut Setup, system: &mut System, o: &Objects) {
    let mflags = r16(system, o.mstate) as u32;
    // gfx-gaps: DrawMesh: the light set's spot and distance-attenuation
    // tables first, for a lit material
    if mflags & 1 != 0 && crate::perf::var("FATES_GFX_LIGHT_LUTS").map_or(true, |v| v != "0") {
        light_luts(s, system, o);
    }
    // DrawMesh: the fog, or fog off, before the material
    if mflags & 0x10 != 0 {
        fog(s, system, o);
    } else {
        s.words(&[0, lit(system, 0x0041_E224)]);
    }
    let mut w: Vec<u32> = Vec::new();
    if mflags & 1 == 0 {
        // nn::gr::CTR::FragmentLight::MakeDisableCommand (0x0023C490)
        let (h1, h2, h3) = (lit(system, 0x0023_C4DC), lit(system, 0x0023_C4E0), lit(system, 0x0023_C4E4));
        w.extend([0, h1, 0, 0]);
        w.extend([0, h2, 1, h3, 0, h3 & !((h2 as i32 >> 5) as u32)]);
    } else {
        let mut l = fragment_light(system, o);
        lut_config(&mut w, system, o, &l[4..]);
        set_light_color(system, o, &mut l);
        let p6 = r8(system, o.params + 6);
        for (k, bit) in [(0x191usize, 1u8), (0x192, 2), (0x193, 4), (0x194, 8), (0x195, 0x10)] {
            l[k] = (p6 & bit != 0) as u8;
        }
        l[0x188] = r8(system, o.params + 0xA4) & 0xF;
        l[0x189] = r8(system, o.params + 0xA5);
        l[0x18B] = r8(system, o.params + 0xA6);
        l[0x18C] = r8(system, o.params + 0xA7);
        light_env(&mut w, system, &l[4..], true);
        for i in 0..8 {
            if l[4 + 0x164 + i] != 0 {
                light_source(&mut w, &l[8 + i * 0x2C..8 + (i + 1) * 0x2C]);
            }
        }
    }
    if mflags & 0x20 == 0 {
        w.extend([0, lit(system, 0x0051_BBE0)]);
    } else {
        w.extend(polygon_offset_words(system, o)); // gfx-gaps: the material's polygon offset
    }
    s.words(&w);
    // the static blobs: the fragment blob, the material's texture blob and
    // the three the MaterialState points at
    s.blob_at(system, o.params + 0xC8);
    s.blob_at(system, o.material + 0x10);
    for at in [0x140u32, 0x148, 0x150] {
        s.blob_at(system, o.mstate + at);
    }
    // gfx-shadow: LightSet::MakeAttitudeCommand (DrawMesh, between the
    // material's commands and its mutable ones): the left eye's words
    if r16(system, o.params + 4) & 1 != 0 {
        let index = r16(system, o.mstate + 0x20) as u32;
        let light_set = o.scene + index * lit(system, 0x0041_E220) * 4 + 0x108;
        let words = crate::command_alter::attitude(system, light_set, crate::command_alter::Eye::Left);
        s.words(&words);
    }
    mutable(s, system, o, mflags);
}

/// MaterialData::MakeLutConfigCommand (0x0051B874): the material's three
/// lookup-table configuration words with the light set's selections, and
/// its tables.
fn lut_config(w: &mut Vec<u32>, system: &mut System, o: &Objects, fl: &[u8]) {
    let mut c: [u32; 6] = std::array::from_fn(|i| r32(system, o.params + 0xA8 + 4 * i as u32));
    c[2] = (c[2] & 0xFFFF_F8FF) | (fl[0x19C] as u32) << 8;
    c[4] = (c[4] & 0xFFFF_F8FF) | (fl[0x19E] as u32) << 8;
    let mut tables = Vec::new();
    // the samplers (distribution 0, 1, fresnel, reflection r, g, b), each
    // a flag byte and its table's blob; a missing one leaves its input's
    // abs bit clear, as the game's (uninitialised) flags happen to
    let mut abs = [1u8; 6];
    let select = lit(system, 0x0051_B9EC);
    for i in 0..6u32 {
        let sampler = r32(system, o.params + 0x8C + 4 * i);
        if sampler == 0 {
            continue;
        }
        abs[i as usize] = r8(system, sampler) & 1;
        tables.extend([r32(system, select + i * 8), r32(system, select + i * 8 + 4)]);
        let (addr, words) = (r32(system, sampler + 4), r32(system, sampler + 8));
        let mut raw = vec![0u8; (words.min(0x1000) * 4) as usize];
        crate::prim::rd(system, addr, &mut raw);
        tables.extend(raw.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())));
    }
    c[0] = ((abs[0] == 0) as u32) << 1
        | ((abs[1] == 0) as u32) << 5
        | ((fl[0x19D] == 0) as u32) << 9
        | ((abs[2] == 0) as u32) << 13
        | ((abs[5] == 0) as u32) << 17
        | ((abs[4] == 0) as u32) << 21
        | ((abs[3] == 0) as u32) << 25;
    w.extend(c);
    w.extend(tables);
}

/// nn::gr::CTR::FragmentLight::MakeLightEnvCommand (0x0050F070), `p` the
/// FragmentLight.
fn light_env(w: &mut Vec<u32>, system: &mut System, p: &[u8], first: bool) {
    let b = |i: usize| p[i] as u32;
    let mut config1: u32 = ((b(399) == 0) as u32) << 16
        | ((b(400) == 0) as u32) << 17
        | ((b(0x185) == 0) as u32) << 19
        | (if b(0x191) == 0 { 7 } else { 0 }) << 20
        | 0xFF04_FFFF;
    let (mut count, mut permutation) = (0u32, 0u32);
    for i in 0..8u32 {
        if p[0x164 + i as usize] == 0 {
            continue;
        }
        if p[0x17C + i as usize] != 0 {
            config1 &= !(1 << i);
        }
        if p[0x16C + i as usize] != 0 {
            config1 &= !(1 << (i + 8));
        }
        if p[0x174 + i as usize] != 0 {
            config1 &= !(1 << (i + 0x18));
        }
        permutation |= i << ((count & 0x3F) << 2);
        count += 1;
    }
    let lits: [u32; 6] = std::array::from_fn(|k| lit(system, 0x0050_F2C8 + 4 * k as u32));
    if first {
        w.extend([0, lits[0], 0, 0]);
    }
    let config0 = ((b(0x189) != 0 || b(0x18A) != 0 || b(0x18B) != 0) as u32)
        | b(0x185) << 2
        | b(0x184) << 4
        | ((b(0x189) != 0) as u32) << 16
        | ((b(0x18A) != 0) as u32) << 17
        | ((b(0x18C) != 0) as u32) << 18
        | ((b(0x18B) != 0) as u32) << 19
        | b(0x188) << 22
        | b(0x186) << 24
        | ((b(0x18E) != 0) as u32) << 27
        | b(0x187) << 28
        | ((b(0x187) != 0 && b(0x18D) == 0) as u32) << 30
        | 0x8000_0400;
    w.extend([
        (count != 0) as u32,
        lits[1],
        b(1) << 10 | b(0) << 20 | b(2),
        lits[2],
        count.saturating_sub(1),
        lits[3],
        config0,
        lits[4],
        config1,
        lits[4] + 1,
        (count == 0) as u32,
        lits[5],
        permutation,
        lits[5] + 0x13,
    ]);
}

/// nn::gr::CTR::FragmentLight::Source::MakeAllCommand (0x0050F328), `p`
/// the source.
fn light_source(w: &mut Vec<u32>, p: &[u8]) {
    let b = |i: usize| p[i] as u32;
    let word = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap());
    w.extend([
        b(0xB) << 10 | b(0xA) << 20 | b(0xC),
        (b(0) * 0x10 + 0x140) | 0x80BF_0000,
        b(0xF) | b(0xE) << 10 | b(0xD) << 20,
        b(6) | b(5) << 10 | b(4) << 20,
        b(9) | b(8) << 10 | b(7) << 20,
        word(0x10),
        word(0x14),
        word(0x24),
        word(0x28),
        0,
        b(0x18) | ((b(1) != 0) as u32) << 1 | ((b(2) != 0) as u32) << 2 | ((b(3) != 0) as u32) << 3,
        word(0x1C),
        word(0x20),
        0,
    ]);
}

/// MaterialData::MakeMutableCommand (0x0051B2DC): the combiners' constant
/// colours, the buffer colour, the texture matrices, the material colours
/// for the vertex shader.
fn mutable(s: &mut Setup, system: &mut System, o: &Objects, mflags: u32) {
    let mut w: Vec<u32> = Vec::new();
    let head = lit(system, 0x0051_B73C);
    let select = r32(system, o.mstate + 8);
    let not_b = (select & 0xFFFFF) >> 16 != 0xB;
    let ss = o.scene + 0x1C;
    if !not_b {
        let v = r32(system, ss + 0x35 * 4);
        w.extend([v, head, v, head | 0xB]);
    }
    let constant = |system: &mut System, nibble: u32| r32(system, o.mstate + 0x24 + ((select >> (nibble * 4)) & 0xF) * 4);
    if mflags & 0x400 != 0 {
        for (k, at) in [0x0051_B740u32, 0x0051_B744, 0x0051_B748, 0x0051_B74C].into_iter().enumerate() {
            let h = lit(system, at);
            let v = constant(system, k as u32);
            w.extend([v, h]);
        }
        if not_b {
            let (a, b) = (constant(system, 4), constant(system, 5));
            w.extend([a, head, b, head | 0xB]);
        }
    }
    if mflags & 0x800 != 0 {
        w.extend([r32(system, o.mstate + 0x54), lit(system, 0x0051_B750)]);
    }
    s.words(&w);
    // the texture matrices (TextureMapState at +0x58, as its Update left
    // them) into the registers the shader's bind table names
    let shader = r32(system, o.params + 8);
    let shbind = r32(system, shader + 0x24);
    let tex_reg = r8(system, shbind + 0xCE) as usize;
    let (rows, projection) = texture_map_state(system, o);
    for (row, v) in rows.iter().enumerate() {
        s.set_float(tex_reg + row, *v);
        if tex_reg + row < 128 {
            s.texture_rows |= 1u128 << (tex_reg + row); // gfxmodels3
        }
    }
    if let Some(v) = projection {
        // gfx-shadow: a projection mapping's translation, as Update leaves it
        let reg = r8(system, shbind + 0xE6) as usize;
        s.set_float(reg, v);
    } else if r8(system, o.mstate + 0xE8) != 0 {
        let reg = r8(system, shbind + 0xE6) as usize;
        let v = [f(system, o.mstate + 0xD8), f(system, o.mstate + 0xDC), f(system, o.mstate + 0xE0), f(system, o.mstate + 0xE4)];
        s.set_float(reg, v);
    }
    let mut w: Vec<u32> = Vec::new();
    if mflags & 0x8000 != 0 {
        let h = lit(system, 0x0051_B758);
        let v = [r32(system, o.mstate + 0x134), r32(system, o.mstate + 0x138), r32(system, o.mstate + 0x13C)];
        w.extend([v[0], h, v[1], h + 0x10, v[2], h + 0x18]);
    }
    s.words(&w);
    if mflags & 0x200 != 0 {
        let k = f32::from_bits(lit(system, 0x0051_B75C));
        let amb = r32(system, o.mstate + 0x40);
        let dif = r32(system, o.mstate + 0x44);
        let byte = |v: u32, i: u32| ((v >> (8 * i)) & 0xFF) as f32;
        let scale = f(system, o.params + 0x88);
        let a_reg = r8(system, shbind + 0xF6) as usize;
        let d_reg = r8(system, shbind + 0xFE) as usize;
        s.set_float(a_reg, [byte(amb, 0) * k, byte(amb, 1) * k, byte(amb, 2) * k, scale]);
        s.set_float(d_reg, [byte(dif, 0) * k, byte(dif, 1) * k, byte(dif, 2) * k, byte(dif, 3) * k]);
    }
}

/// nw::h3d::TextureMapState::Update (0x00320FF4) on a copy of the state
/// (MaterialState +0x58): each texture coordinate set's matrix from its
/// scale, rotation and translation in the material's SRT mode, into the
/// uniform rows its layout names. Returns the eight rows MakeMutableCommand
/// uploads, and, when a camera projection (mapping 3) was computed, the
/// translations it moves to +0x80 (gfx-shadow).
fn texture_map_state(system: &mut System, o: &Objects) -> ([[f32; 4]; 8], Option<[f32; 4]>) {
    let tms = o.mstate + 0x58;
    let mut rows: [[f32; 4]; 8] = std::array::from_fn(|r| {
        let at = tms + r as u32 * 16;
        [f(system, at), f(system, at + 4), f(system, at + 8), f(system, at + 12)]
    });
    let k = |system: &mut System, at: u32| f32::from_bits(lit(system, at));
    let (c24, c22, c23, zero, one, c25) =
        (k(system, 0x0032_1360), k(system, 0x0032_1364), k(system, 0x0032_1368), k(system, 0x0032_136C), k(system, 0x0032_1370), k(system, 0x0032_1374));
    let _ = c25;
    let mflags = r16(system, o.mstate) as u32;
    let ss = o.scene + 0x1C;
    let dirty_bits = lit(system, 0x0032_1378);
    let layouts = lit(system, 0x0032_1628);
    let bases = lit(system, 0x0032_162C);
    let counts = lit(system, 0x0032_1630);
    let which_layout = r8(system, o.mstate + 3) as u32;
    let layout = r16(system, layouts + which_layout * 2) as u32;
    // gfx-shadow: a receiver's shadow coordinate set as FieldActorCallback
    // leaves it, and the projection translations (TextureMapState +0x80)
    let receiver = crate::shadow::receiver_mapping(system, o);
    let mut translation = [f(system, tms + 0x80), f(system, tms + 0x84), f(system, tms + 0x88), f(system, tms + 0x8C)];
    let mut projected = false;
    for u in 0..3u32 {
        let e = tms + u * 0x18;
        let over = receiver.filter(|r| r.0 == u).map(|r| (r.1, r.2));
        let flags = over.map_or_else(|| r8(system, e + 0x94), |(b, _)| b[0]);
        let mapping = over.map_or_else(|| r8(system, e + 0x96), |(b, _)| b[2]);
        let mut dirty = flags & 1 != 0;
        if !dirty {
            dirty = match mapping {
                3 => {
                    let cams = r32(system, ss + 0x40);
                    let reference = over.map_or_else(|| r8(system, e + 0x97), |(b, _)| b[3]) as i8 as i32;
                    r8(system, cams.wrapping_add(((reference + 1) * 0x6C) as u32)) & 1 != 0
                }
                4 => r8(system, ss + 0x48) & 1 != 0,
                _ => false,
            } || mflags & r16(system, dirty_bits + u * 2) as u32 != 0;
        }
        if !dirty {
            continue;
        }
        let (sx, sy, rot, tx, ty) = match over {
            Some((_, v)) => (v[0], v[1], v[2], v[3], v[4]),
            None => (f(system, e + 0x98), f(system, e + 0x9C), f(system, e + 0xA0), f(system, e + 0xA4), f(system, e + 0xA8)),
        };
        // nn::math::SinCosFIdx of the rotation times the radian-to-index
        // factor: the sine and cosine of the rotation in radians
        let angle = rot * c22 * (std::f32::consts::TAU / (c22 * std::f32::consts::TAU).round().max(1.0));
        let (sn, cs) = angle.sin_cos();
        let mut m = [zero; 16];
        m[10] = one;
        m[15] = one;
        match over.map_or_else(|| r8(system, e + 0x95), |(b, _)| b[1]) {
            0 => {
                m[0] = sx * cs;
                m[1] = -(sx * sn);
                m[3] = (((sn * c23 - cs * c23) + c23) - tx) * sx;
                m[4] = sy * sn;
                m[5] = sy * cs;
                m[7] = (((sn * c24 - cs * c23) + c23) - ty) * sy;
            }
            1 => {
                m[0] = sx * cs;
                m[1] = sx * sn;
                m[3] = (-(cs * tx) - sn * ty) * sx;
                m[4] = -(sy * sn);
                m[5] = sy * cs;
                m[7] = (sn * tx - cs * ty) * sy;
            }
            2 => {
                m[0] = sx * cs;
                m[1] = -(sx * sn);
                m[3] = (sx * cs * (-tx - c23) - sx * sn * (ty - c23)) + c23;
                m[4] = sy * sn;
                m[5] = sy * cs;
                m[7] = sy * sn * (-tx - c23) + sy * cs * (ty - c23) + c23;
            }
            _ => {
                m[0] = one;
                m[5] = one;
            }
        }
        match mapping {
            1 => {
                for i in 0..12 {
                    m[i] = f(system, ss + 0x10 + 4 * i as u32);
                }
            }
            3 => {
                // gfx-shadow: camera projection: the translation moved out,
                // then times the camera entry's projection (+4) and view
                // (+0x34), entry (reference + 1) of SceneState +0x40
                let reference = over.map_or_else(|| r8(system, e + 0x97), |(b, _)| b[3]) as i8 as i32;
                let entry = r32(system, ss + 0x40).wrapping_add(((reference + 1) * 0x6C) as u32);
                let slot = if u == 0 { 0 } else { 2 };
                translation[slot] = m[3];
                translation[slot + 1] = m[7];
                m[3] = zero;
                m[7] = zero;
                let a: [f32; 12] = std::array::from_fn(|i| m[i]);
                let r = mul34(&mul34(&a, &m34(system, entry + 4)), &m34(system, entry + 0x34));
                m[..12].copy_from_slice(&r);
                projected = true;
            }
            4 => {
                crate::perf::warn_once(format!("gfx: H3D texture mapping 4 (projection) not ported")); // gfx-perf: once
            }
            _ => {}
        }
        let which = (layout >> (u * 4)) & 7;
        for kk in 0..3u32 {
            let count = r8(system, counts + kk) as usize;
            if which & (1 << kk) == 0 || count == 0 {
                continue;
            }
            let base = r8(system, bases + kk) as usize;
            for row in 0..count {
                if base + row < 8 {
                    rows[base + row] = [m[row * 4], m[row * 4 + 1], m[row * 4 + 2], m[row * 4 + 3]];
                }
            }
        }
    }
    // gfxmodels3: dev, FATES_GFX_TEXMAP_TRACE=1: materials whose eye
    // position was set (BattleUnit::SetMaterialEyePosition, flag 0x4000)
    if mflags & 0x4000 != 0 && crate::perf::var_os("FATES_GFX_TEXMAP_TRACE").is_some() {
        let v: Vec<f32> = [0x98u32, 0x9C, 0xA0, 0xA4, 0xA8].iter().map(|&k| f(system, tms + 0x30 + k)).collect();
        let b: Vec<u8> = [0x94u32, 0x95, 0x96].iter().map(|&k| r8(system, tms + 0x30 + k)).collect();
        eprintln!(
            "gfx-texmap game {} mstate {:08X} flags {mflags:04X} u2 flags {:02X} mode {} map {} s {:.3},{:.3} r {:.3} t {:.4},{:.4} rows {:?}",
            zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
            o.mstate,
            b[0], b[1], b[2],
            v[0], v[1], v[2], v[3], v[4],
            rows
        );
    }
    (rows, projected.then_some(translation))
}

/// MaterialData::MakeFogCommand (0x0051B208): the scene fog the material
/// names (SceneState +0x1C, +8: 0x218 bytes each; on, the colour, and the
/// table's words as the fog keeps them encoded).
fn fog(s: &mut Setup, system: &mut System, o: &Objects) {
    let index = r16(system, o.mstate + 0x22) as u32;
    let ss = o.scene + 0x1C;
    let at = r32(system, ss + 8) + index * 0x218;
    let on = r8(system, at) != 0;
    let flip = r8(system, at + 1) != 0;
    let color = r32(system, at + 2) & 0xFF_FFFF;
    let mut w = vec![
        if on { 5 } else { 0 } | (flip as u32) << 16,
        lit(system, 0x0051_B2D4),
        color,
        lit(system, 0x0051_B2D8),
    ];
    if on {
        let mut raw = vec![0u8; 0x210];
        crate::prim::rd(system, at + 8, &mut raw);
        w.extend(raw.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())));
    }
    s.words(&w);
}

/// nn::gr::CTR::Combiner::Stage::MakeCommand (0x00510070) for a stage as
/// its byte layout holds it.
pub(crate) fn stage_words(w: &mut Vec<u32>, p: &[u8; 0x18]) {
    let b = |i: usize| p[i] as u32;
    w.extend([
        b(4) | b(5) << 4 | b(6) << 8 | b(0xD) << 16 | b(0xE) << 20 | b(0xF) << 24,
        u16::from_le_bytes([p[0x16], p[0x17]]) as u32 | 0x804F_0000,
        b(1) | b(2) << 4 | b(3) << 8 | b(0xA) << 12 | b(0xB) << 16 | b(0xC) << 20,
        b(0) | b(9) << 16,
        (b(0x12) | b(0x13) << 8 | b(0x14) << 16) | b(0x15) << 24,
        b(7) | b(0x10) << 16,
    ]);
}

/// nw::h3d::BlendCommand::MakeCommand (0x0051A8DC): `p` its byte layout
/// (on, equations, factors) and the blend colour at +8.
fn blend_words(w: &mut Vec<u32>, system: &mut System, p: &[u8; 12]) {
    let (l968, l96c, l970) = (lit(system, 0x0051_A968), lit(system, 0x0051_A96C), lit(system, 0x0051_A970));
    let header = l968.wrapping_sub(0xE1_0000);
    if p[0] == 0 {
        w.extend([l968, header, l970, l96c]);
    } else {
        let b = |i: usize| p[i] as u32;
        w.extend([
            l968,
            header,
            (b(1) | b(2) << 8 | b(3) << 16) | b(5) << 20 | b(4) << 24 | b(6) << 28,
            l96c,
            6,
            l96c + 1,
            u32::from_le_bytes([p[8], p[9], p[10], p[11]]),
            l96c | (l96c as i32 >> 0x12) as u32,
        ]);
    }
}

/// FrameBufferAccessCommand::SetReadAndWriteFrom (0x00321704) and
/// MakeCommand (0x0051AAB4): the colour and depth buffer access the
/// material's fragment blob holds at its frame-access offset.
fn framebuffer_words(w: &mut Vec<u32>, system: &mut System, params: u32) {
    let blob = r32(system, params + 0xD0);
    let at = r8(system, params + 7) as u32;
    let word = |system: &mut System, k: u32| r32(system, blob + (at + k) * 4);
    let mut p = [word(system, 0) as u8, word(system, 2) as u8, word(system, 4) as u8, 0, 0];
    let v6 = word(system, 6);
    p[3] = v6 as u8;
    p[4] = if v6 & 2 == 0 { 0 } else { 0x10 } | (word(system, 2) & 0xFF) as u8;
    // BeginCommand sets the colour read on after
    p[0] = 1;
    let (l08, l0c) = (lit(system, 0x0051_AB08), lit(system, 0x0051_AB0C));
    let h = l08 | (l08 as i32 >> 8) as u32;
    w.extend([
        p[0] as u32,
        l08,
        p[1] as u32,
        h,
        p[2] as u32,
        h + 1,
        p[3] as u32,
        (h + 1) | ((h + 1) as i32 >> 8) as u32,
        (p[4] as u32) << 8,
        l0c,
    ]);
}

/// ModelState::BeginCommand (0x005071F8): what a model overrides of its
/// materials, its fade colour through combiner stage 5, depth test,
/// culling and blending.
fn model_state(s: &mut Setup, system: &mut System, o: &Objects, layer: u8) {
    let flags = r16(system, o.model + 8) as u32;
    if flags & 1 == 0 {
        return;
    }
    let mut w: Vec<u32> = Vec::new();
    let mb = |system: &mut System, k: u32| r8(system, o.model + k);
    if flags & 8 != 0 {
        // a stage that keeps the previous one, then times the model colour
        let mut p = [0u8; 0x18];
        for (i, v) in [(4usize, 0xF), (5, 0xF), (6, 0xF), (0xD, 0xF), (0xE, 0xF), (0xF, 0xF)] {
            p[i] = v;
        }
        p[0x16..0x18].copy_from_slice(&0xF8u16.to_le_bytes());
        p[0] = 1;
        p[9] = 1;
        p[4] = 0xF;
        p[0xD] = 0xF;
        p[5] = 0xE;
        p[0xE] = 0xE;
        p[6] = 0xE;
        p[0xF] = 0xE;
        for k in 0..4 {
            p[0x12 + k] = mb(system, 0xC + k as u32);
        }
        if layer == 0 {
            p[0x15] = 0xFF;
        }
        let add = [mb(system, 0x10), mb(system, 0x11), mb(system, 0x12), mb(system, 0x13)];
        if add[0] as u32 + add[1] as u32 + add[2] as u32 != 0 {
            if layer == 3 {
                p[7] = 1;
                let k = f32::from_bits(lit(system, 0x0050_76CC));
                for c in 0..3 {
                    let v = mb(system, 0xC + c as u32) as f32 * k + add[c] as f32 * k;
                    p[0x12 + c] = v.clamp(0.0, 255.0) as u8;
                }
            } else {
                // the buffer colour the stage then adds (the combiner's
                // other buffer settings left as constructed)
                w.extend([0, lit(system, 0x0051_0068), u32::from_le_bytes(add), lit(system, 0x0051_006C)]);
                p[0] = 8;
                p[6] = 0xD;
            }
        }
        p[5] = 0xE;
        p[4] = 0xF;
        stage_words(&mut w, &p);
        if mb(system, 0xF) != 0xFF && layer == 0 {
            let mut b = [0u8; 12];
            b[..7].copy_from_slice(&[1, 0, 4, 0xC, 1, 0xD, 1]);
            b[8..12].copy_from_slice(&r32(system, o.model + 0xC).to_le_bytes());
            blend_words(&mut w, system, &b);
            framebuffer_words(&mut w, system, o.params);
        }
    }
    if flags & 0x10 != 0 {
        let mode = r16(system, o.model + 10) as u32;
        if mode & 0xF != 0 {
            let (on, func) = match mode & 0xF {
                1 => (1, 5),
                2 => (0, 5),
                3 => (1, 6),
                4 => (1, 1),
                _ => (0, 0),
            };
            w.extend([on | func << 4, lit(system, 0x0051_A9B4)]);
        }
        if mode & 0xF00 != 0 {
            let cull = match (mode >> 8) & 0xF {
                1 => 2,
                2 => 1,
                _ => 0,
            };
            w.extend([cull, lit(system, 0x0051_A988)]);
        }
        if mode & 0xF000 != 0 && matches!(mode >> 12, 1 | 2) {
            crate::perf::warn_once(format!("gfx: H3D model clip plane (ClipTool) not ported")); // gfx-perf: once
        }
        if mode & 0xF0 != 0 {
            let mut b = [0u8; 12];
            match (mode >> 4) & 0xF {
                2 => b[..7].copy_from_slice(&[1, 0, 0, 6, 1, 7, 0]),
                3 => b[..7].copy_from_slice(&[1, 0, 0, 6, 1, 1, 0]),
                4 => b[..7].copy_from_slice(&[1, 2, 0, 6, 1, 1, 0]),
                _ => {}
            }
            if b[0] != 0 {
                let at = lit(system, 0x0050_76D0);
                let c = r32(system, at);
                b[8..12].copy_from_slice(&c.to_le_bytes());
            }
            blend_words(&mut w, system, &b);
            framebuffer_words(&mut w, system, o.params);
        }
    }
    if flags & 0x800 != 0 {
        // gfx-gaps: Graphics::SetClearType(top, 2), no PICA state: lib.rs
        // skipped_draw_mesh does it for the DrawMesh calls the native path skips
    }
    let callback = r32(system, o.model + 4);
    if callback != 0 && (flags << 0x1A) as i32 >= 0 {
        let vtable = r32(system, callback);
        let (f0, f1) = (zakuro_core::region::to_us(r32(system, vtable)), zakuro_core::region::to_us(r32(system, vtable + 4))); // eur
        if f0 == crate::shadow::FIELD_ACTOR_ON_BEGIN {
            // gfx-shadow: the shadow receivers' callback
            crate::shadow::field_actor_words(&mut w, system, o);
        } else if f0 == WIND_ON_BEGIN {
            // gfx-terrain: grass and foliage (battle and map scenery)
            wind_words(&mut w, system, callback);
        } else {
            crate::perf::warn_once(format!("gfx: H3D model state callback, vtable {vtable:08X} ({f0:08X} {f1:08X}), not ported")); // gfx-perf: once
        }
    }
    s.words(&w);
}

/// gfx-terrain: nw::h3d::res::TextureSamplerData::MakeUnit{0,1,2}Command
/// (0x0051D118, 0x0051D27C, 0x0051D3D8): a unit's parameter word and its
/// level-of-detail word, with their headers, for the sampler's bytes `b`
/// (+0 type, +1 and +2 wraps, +3 magnification, +4 minification and
/// mipmap mode, +5 smallest level, +8 the bias as an f32), the texture's
/// ETC1-ness and its largest level. Unit 0 also carries the type (bits
/// 20 and 28). The bias goes through the function's own constants.
pub(crate) fn sampler_unit_words(system: &mut System, b: &[u8; 16], unit: usize, etc1: bool, max_level: u8) -> [u32; 4] {
    let unit = unit.min(2);
    let (h_param, h_lod, k) = match unit {
        0 => (lit(system, 0x0051_D260), lit(system, 0x0051_D260) + 1, 0x0051_D264u32),
        1 => (lit(system, 0x0051_D3B8), lit(system, 0x0051_D3BC), 0x0051_D3C0),
        _ => (lit(system, 0x0051_D514), lit(system, 0x0051_D518), 0x0051_D51C),
    };
    let mode = b[4];
    let mut param = (matches!(mode, 3..=5) as u32) << 2
        | (b[3] as u32) << 1
        | (if etc1 { 2 } else { 0 }) << 4
        | (b[2] as u32) << 8
        | (b[1] as u32) << 12
        | ((mode == 2 || mode == 5) as u32) << 24;
    if unit == 0 {
        param |= ((b[0] == 2 || b[0] == 4) as u32) << 20 | (b[0] as u32) << 28;
    }
    let lod = if mode == 3 || mode == 0 {
        0
    } else {
        let kf = |system: &mut System, i: u32| f32::from_bits(lit(system, k + 4 * i));
        let (zero, add, mul, limit, half, top) = (kf(system, 0), kf(system, 1), kf(system, 2), lit(system, k + 12), kf(system, 4), kf(system, 5));
        let v = f32::from_bits(u32::from_le_bytes([b[8], b[9], b[10], b[11]]));
        let bias = if v == zero || (v.to_bits() << 1) >> 24 == 0xFF {
            0
        } else {
            let x = (v + add) * mul;
            if zero > x {
                (zero + half) as u32
            } else if x.to_bits() as i32 >= 0x4600_0000 {
                (top - half) as u32
            } else if (x.to_bits() as i32) < limit as i32 {
                (x + half) as u32
            } else {
                (x - half) as u32
            }
        };
        bias | (max_level as u32) << 16 | (b[5] as u32) << 24
    };
    [param, h_param, lod, h_lod]
}

/// gfx-terrain: WindCallback::OnBegin (0x001C5BF0), the ModelState callback
/// of grass and foliage models.
const WIND_ON_BEGIN: u32 = 0x001C_5BF0;

/// gfx-terrain: what WindCallback::OnBegin writes: a float32 uniform
/// upload (register byte at +0x1A; x, y, z, w at +4, +8, +0xC, +0x10, sent
/// w first as the f32 upload takes them) under the header in its literal
/// 0x001C5C50, then the word at +0x14 under the header in 0x001C5C54 (the
/// combiner's stage 5 constant colour: the model's shade).
fn wind_words(w: &mut Vec<u32>, system: &mut System, cb: u32) {
    let (h0, h1) = (lit(system, 0x001C_5C50), lit(system, 0x001C_5C54));
    w.extend([
        r8(system, cb + 0x1A) as u32 | 0x8000_0000,
        h0,
        r32(system, cb + 0x10),
        r32(system, cb + 0xC),
        r32(system, cb + 8),
        r32(system, cb + 4),
        r32(system, cb + 0x14),
        h1,
    ]);
}

/// The target's part of the state: what the 2D layer's target leaves set
/// when a scene callback draws (prim::Context::SetTargetDisplay): the whole
/// portrait framebuffer as viewport and scissor, and the depth map from
/// -1..0 to 0..1.
pub fn target(s: &mut Setup, disp: usize) {
    // float24: 120 (half of 240), 200 and 160 (half of 400 and 320), -1
    s.set(0x041, 0x45_E000);
    s.set(0x043, if disp == 0 { 0x46_9000 } else { 0x46_4000 });
    s.set(0x068, 0);
    s.set(0x04D, 0xBF_0000);
    // gfx-final: the material's polygon offset (MakePolygonOffsetCommand,
    // in mesh_setup) comes after the target's depth map in the GPU's
    // commands and stays: set to 0 here it was lost (the ch3b cave's light
    // patches, decals 0.00015 in front of the ground, failed their depth
    // test against it in hard-edged patches). FATES_GFX_KEEP_OFFSET=0: lost
    if !s.known[0x04E] || crate::perf::var("FATES_GFX_KEEP_OFFSET").is_ok_and(|v| v == "0") {
        s.set(0x04E, 0);
    }
    s.set(0x065, 0);
}


/// gfx-gaps: nw::h3d::DepthBufferingCommand::MakePolygonOffsetCommand
/// (0x0051A9B8) as MaterialData::MakeCommandWithoutCache calls it: the
/// SceneState's (SceneSystem +0x1C, as DrawMesh passes it) depth-buffering
/// word (+0xBC; bit 31 a w-buffer, bits 24-30
/// the depth bits, the low 24 the plain offset as a float24) and depth
/// range (+0xC4, +0xC8), the material's offset (params +0xC4), into the
/// depth map's offset word (header at literal 0x0051AAAC).
fn polygon_offset_words(system: &mut System, o: &Objects) -> [u32; 2] {
    let at = o.scene + 0x1C + 0xBC;
    let word = r32(system, at);
    let f = f32::from_bits(r32(system, o.params + 0xC4));
    let k = |system: &mut System, a: u32| f32::from_bits(r32(system, zakuro_core::region::us(a))); // eur: USA literals
    let (k_mul, k_zero, k_div) = (k(system, 0x0051_AAA4), k(system, 0x0051_AAA8), k(system, 0x0051_AAB0));
    let bits = (word >> 24) & 0x7F;
    let range = ((1u64 << bits.min(31)) - 1) as f32;
    let value = if word & 0x8000_0000 != 0 {
        if f == k_zero {
            0
        } else if bits == 24 {
            to_f24(f * k_mul / k_div)
        } else {
            to_f24(f / range)
        }
    } else if f == k_zero {
        word & 0xFF_FFFF
    } else {
        let (near, far) = (k(system, at + 8), k(system, at + 0xC));
        to_f24(near - (near - far) * f * k_mul / range)
    };
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if crate::perf::var_os("FATES_GFX_POLYGON_OFFSET_TRACE").is_some() && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let header = lit(system, 0x0051_AAAC);
        eprintln!("gfx-polygon-offset h3d word {word:08X} offset {f} -> {} header {header:08X}", f24(value));
    }
    [value, lit(system, 0x0051_AAAC)]
}

/// gfx-gaps: nn::gr::CTR::Float32ToFloat24 (0x0012013C).
fn to_f24(v: f32) -> u32 {
    let b = v.to_bits();
    let sign = b >> 31;
    let exp = ((b >> 23) & 0xFF) as i32 - 64;
    if v == 0.0 || exp <= 0 {
        return sign << 23;
    }
    let exp = exp.min(0x7F) as u32;
    (sign << 23) | (exp << 16) | ((b >> 7) & 0xFFFF)
}

/// gfx-gaps: MaterialData::MakeLightLutCommand (0x0051B768): for each of
/// the eight lights of the material's FragmentLight (SceneState
/// `*(+0) + index * 0x278`, index MaterialState +0x20), its spot table
/// (+0x238 + 4 i) as lookup table 8 + i and its distance-attenuation table
/// (+0x258 + 4 i) as table 16 + i: the table select word (header at
/// `*(lit 0x0051B870) + 0x14` / `+ 0xC`), then a jump to the table's
/// command blob (+4 address, +8 words). Left out, a light with distance
/// attenuation (night maps' torches) lit nothing natively: the GPU had
/// the tables from the emulated draws, the native setup none.
fn light_luts(s: &mut Setup, system: &mut System, o: &Objects) {
    let index = r16(system, o.mstate + 0x20) as u32;
    let base = r32(system, o.scene + 0x1C) + index * 0x278;
    let headers = r32(system, zakuro_core::region::us(0x0051_B870));
    for i in 0..8u32 {
        for (at, select, header) in [(0x238, 0x800, 0x14), (0x258, 0x1000, 0xC)] {
            let table = r32(system, base + at + 4 * i);
            if table == 0 {
                continue;
            }
            let h = r32(system, headers + header);
            s.words(&[(i << 8) + select, h]);
            s.blob_at(system, table + 4);
        }
    }
}
