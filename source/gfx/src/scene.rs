//! A native H3D mesh draw: what the renderer needs to draw one sub-mesh
//! with the game's own vertex program (run by zakuro-gpu's shade.vert) and
//! the PICA fragment stages (zakuro-gpu's raster.frag), all from a
//! `setup::Setup` the engine state was turned into and the mesh's vertex
//! data as its BCH holds it.

use std::sync::Arc;

use zakuro_gpu::native3d;

use crate::h3d::{Mesh, SubMesh};
use crate::prim::{Sampling, TexRef};
use crate::setup::Setup;

/// The pipeline state a shaded draw needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// the blend function register, when blending
    pub blend: Option<u32>,
    /// red, green, blue, alpha writes
    pub mask: u32,
    pub depth_test: bool,
    pub depth_func: u8,
    pub depth_write: bool,
    /// PICA culling (0 none, 1 and 2 the two windings)
    pub cull: u8,
    pub lighting: bool,
    pub procedural: bool,
    /// 0 triangles, 1 strip, 2 fan
    pub topology: u8,
    /// gfx-final: raster.frag works the depth out itself (the depth map's
    /// offset puts it past what a viewport holds)
    pub writes_depth: bool,
}

#[derive(Clone)] // gfx-final
pub struct Shaded {
    /// the program and its operand descriptors, 4096 + 128 words
    pub program: Arc<Vec<u32>>,
    /// shade.vert's Shading block
    pub shading: Vec<u32>,
    /// raster.frag's Draw block
    pub draw: Vec<u32>,
    /// raster.frag's Tables, value and step pairs (gfx-perf: shared by
    /// every draw with the same tables)
    pub tables: Arc<Vec<f32>>,
    /// the vertex bytes as the mesh's buffer holds them
    pub vertices: Arc<Vec<u8>>,
    /// perf3: shared, as the vertices are: the frame's draw, its pictures in
    /// between and the free camera's haze overlay of it point at one list
    pub indices: Arc<Vec<u32>>,
    pub textures: [Option<(TexRef, Sampling)>; 3],
    /// gfx-terrain: each unit's texture sizes and level of detail
    pub mips: [Mip; 3],
    pub key: Key,
    /// PICA framebuffer rectangles (x, y, w, h)
    pub viewport: [i32; 4],
    pub scissor: [i32; 4],
    /// the Vulkan depth range the depth map gives
    pub depth_range: (f32, f32),
    pub blend_color: [f32; 4],
    /// gfx-hfr: the float uniforms that place the vertices (those the
    /// textures' coordinates can read too second), which pictures in
    /// between move (hfr.rs)
    pub placing: (u128, u128),
    /// gfx-hfr: its vertices are made anew each frame (nw::eft particles):
    /// matched with the frame before's by program and state, not by its
    /// vertex count
    pub loose: bool,
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

/// shade.vert's input layout of a mesh: per input register the start, the
/// stride and offset | type << 8 | count << 16, and the defaults.
fn inputs(mesh: &Mesh) -> ([[u32; 4]; 16], [[f32; 4]; 16]) {
    let mut attributes = [[0u32; 4]; 16];
    let mut defaults = [[0f32; 4]; 16];
    for (name, value) in &mesh.fixed {
        let r = name.reg() as usize;
        if r < 16 {
            defaults[r] = *value;
        }
    }
    for a in &mesh.attrs {
        let r = a.name.reg() as usize;
        if r < 16 {
            defaults[r] = [0.0, 0.0, 0.0, 1.0];
            attributes[r] = [0, mesh.stride, a.offset | (a.format as u32) << 8 | (a.elements as u32) << 16, 0];
        }
    }
    (attributes, defaults)
}

/// The texture a unit's parameter words bind: its address, size, format
/// and sampling.
pub fn unit_texture(r: &[u32], unit: usize, virt: &dyn Fn(u32) -> u32) -> Option<(TexRef, Sampling)> {
    let (base, addr_reg, format_reg) = match unit {
        0 => (0x081, 0x085, 0x08E),
        1 => (0x091, 0x095, 0x096),
        _ => (0x099, 0x09D, 0x09E),
    };
    let dim = r[base + 1];
    let (height, width) = (dim & 0x7FF, (dim >> 16) & 0x7FF);
    let param = r[base + 2];
    let addr = r[addr_reg] << 3;
    if addr == 0 || width == 0 || height == 0 {
        return None;
    }
    let wrap = ((param >> 12) & 7) as u8;
    // skybg: a cube map (unit 0, type 1): its six faces stacked (skybg.rs)
    let mut format = r[format_reg] & 0xF;
    if unit == 0 {
        if let Some(faces) = crate::skybg::unit0_cube_faces(r) {
            crate::skybg::note_cube(faces.map(virt));
            format |= crate::skybg::CUBE;
        }
    }
    Some((
        TexRef { addr: virt(addr), width, height, format },
        Sampling { wrap: wrap & 3, wrap_t: ((param >> 8) & 3) as u8, sharp: false, linear_mag: param & 2 != 0, linear_min: param & 4 != 0 },
    ))
}

/// gfx-terrain: how a unit goes between its texture's sizes, as the
/// emulated GPU reads it (zakuro-gpu raster.rs `level_count`, `Lod::read`)
/// from GPUREG_TEXUNITi_LOD and the filters in GPUREG_TEXUNITi_PARAM: as
/// many sizes as the largest level of detail allows, each half the last,
/// down to 8 texels a side; the level kept between `min` and `max`, with
/// the bias in 256ths. One size is only ever magnified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mip {
    pub levels: u8,
    pub min: u8,
    pub max: u8,
    pub bias: i16,
    /// bilinear rather than point sampling when minifying
    pub linear: bool,
    /// blends the two sizes about the level
    pub between: bool,
}

/// gfx-terrain: a unit's Mip from its parameter words.
pub fn unit_mip(r: &[u32], unit: usize, width: u32, height: u32) -> Mip {
    let base = [0x081, 0x091, 0x099][unit.min(2)];
    let (config, lod) = (r[base + 2], r[base + 3]);
    let largest = (lod >> 16) & 0xF;
    let levels = 1 + largest.min(width.trailing_zeros().min(height.trailing_zeros()).saturating_sub(3));
    if levels == 1 {
        return Mip { levels: 1, ..Mip::default() };
    }
    let max = largest.min(levels - 1);
    Mip {
        levels: levels as u8,
        min: ((lod >> 24) & 0xF).min(max) as u8,
        max: max as u8,
        bias: ((lod << 19) as i32 >> 19) as i16,
        linear: config & 0x4 != 0,
        between: config & (1 << 24) != 0,
    }
}

impl std::fmt::Debug for Shaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // gfx-terrain: with the bound textures (address, size, format)
        let tex = self.textures.map(|t| t.map(|(t, _)| (t.addr, t.width, t.height, t.format)));
        write!(f, "Shaded {{ {:?}, {} indices, tex {:X?} }}", self.key, self.indices.len(), tex)
    }
}

/// One sub-mesh draw from its setup.
pub fn shaded(setup: &Setup, mesh: &Mesh, sub: &SubMesh, vertices: Arc<Vec<u8>>, program: Arc<Vec<u32>>, virt: &dyn Fn(u32) -> u32) -> Option<Shaded> {
    let topology = match sub.primitive {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => return None,
    };
    let (attributes, defaults) = inputs(mesh);
    let indices = sub.indices.iter().map(|&i| i as u32).collect();
    Some(shaded_from(setup, topology, &attributes, &defaults, indices, vertices, program, virt))
}

/// gfx-eft: one draw from its setup and its vertex input as shade.vert
/// takes it (per input register: start, stride, offset | type << 8 |
/// count << 16; the defaults), for draws that are not H3D sub-meshes.
#[allow(clippy::too_many_arguments)]
pub fn shaded_from(setup: &Setup, topology: u8, attributes: &[[u32; 4]; 16], defaults: &[[f32; 4]; 16], indices: Vec<u32>, vertices: Arc<Vec<u8>>, program: Arc<Vec<u32>>, virt: &dyn Fn(u32) -> u32) -> Shaded {
    let r = &setup.regs;
    let semantics = native3d::semantics(r);
    let depth_scale = f24(r[0x04D]);
    let depth_offset = f24(r[0x04E]);
    let (vw, vh) = (f24(r[0x041]) * 2.0, f24(r[0x043]) * 2.0);
    let (vx, vy) = ((r[0x068] & 0x3FF) as i32, ((r[0x068] >> 16) & 0x3FF) as i32);
    let mut shading: Vec<u32> = Vec::with_capacity(564);
    shading.extend(setup.floats.iter().flatten().map(|f| f.to_bits()));
    for v in setup.ints {
        shading.extend([v & 0xFF, (v >> 8) & 0xFF, ((v >> 16) & 0xFF) as u8 as i8 as i32 as u32, 0]);
    }
    shading.extend([setup.bools as u32, setup.entry, 0, 0]);
    shading.extend(semantics);
    shading.extend([depth_scale.to_bits(), depth_offset.to_bits(), 0, 0]);
    shading.extend([vx as f32, vy as f32, vw, vh].map(f32::to_bits));
    shading.extend(attributes.as_flattened());
    shading.extend(defaults.as_flattened().iter().map(|f| f.to_bits()));

    let mut textures = [None; 3];
    let mut mips = [Mip { levels: 1, ..Mip::default() }; 3];
    let mut enabled = 0u32;
    let config = r[0x080];
    for unit in 0..3 {
        if config & (1 << unit) != 0 {
            textures[unit] = unit_texture(r, unit, virt);
            if let Some((t, _)) = textures[unit] {
                enabled |= 1 << unit;
                mips[unit] = unit_mip(r, unit, t.width, t.height); // gfx-terrain
                if t.format & crate::skybg::CUBE != 0 {
                    mips[unit] = Mip { levels: 1, ..Mip::default() }; // skybg: the stacked faces' largest size
                }
            }
        }
    }
    let draw = native3d::draw_block(r, enabled, depth_scale, depth_offset);
    // gfx-perf: built once per content (cache::tables)
    let tables = crate::cache::tables(&setup.luts, &setup.fog, || {
    let mut tables = vec![0f32; native3d::TABLE_ENTRIES * 2];
    for (i, &v) in setup.luts.iter().enumerate() {
        let step = ((v >> 12) & 0x7FF) as f32 / 2047.0;
        tables[i * 2] = (v & 0xFFF) as f32 / 4095.0;
        tables[i * 2 + 1] = if v & (1 << 23) != 0 { -step } else { step };
    }
    for (i, &v) in setup.fog.iter().enumerate() {
        let step = ((v & 0x1FFF) as i32) << 19 >> 19;
        tables[(native3d::FOG_TABLE + i) * 2] = ((v >> 13) & 0x7FF) as f32 / 2048.0;
        tables[(native3d::FOG_TABLE + i) * 2 + 1] = step as f32 / 2048.0;
    }
    tables
    });

    let mask = r[0x107];
    let color_write = if r[0x113] != 0 { (mask >> 8) & 0xF } else { 0 };
    let writable = r[0x115] != 0;
    let key = Key {
        blend: (r[0x100] & 0x100 != 0).then_some(r[0x101]),
        mask: color_write,
        depth_test: mask & 1 != 0,
        depth_func: ((mask >> 4) & 7) as u8,
        depth_write: writable && mask & (1 << 12) != 0,
        cull: (r[0x040] & 3) as u8,
        lighting: native3d::lights(r),
        procedural: config & 0x400 != 0,
        topology,
        writes_depth: false, // gfx-final: set below
    };
    let mut key = key;
    if crate::perf::var_os("FATES_GFX_SHADED_FORCE").is_some() { // gfx-perf: cached env
        key.mask = 0xF;
        key.depth_test = false;
        key.depth_write = false;
        key.blend = None;
    }
    if crate::perf::var_os("FATES_GFX_SHADED_DUMP").is_some() { // gfx-perf: cached env
        eprintln!("gfx-shaded {key:?} viewport {:?} depth {} {} entry {} sem {:?}", [vx, vy, vw as i32, vh as i32], depth_scale, depth_offset, setup.entry, &semantics[..8]);
    }
    // the scissor: mode in 0x065, corners in 0x066/0x067
    let scissor = if r[0x065] & 3 != 0 {
        let (x0, y0) = ((r[0x066] & 0x3FF) as i32, ((r[0x066] >> 16) & 0x3FF) as i32);
        let (x1, y1) = ((r[0x067] & 0x3FF) as i32, ((r[0x067] >> 16) & 0x3FF) as i32);
        [x0, y0, x1 - x0 + 1, y1 - y0 + 1]
    } else {
        [vx, vy, vw as i32, vh as i32]
    };
    let mapped = (depth_offset, depth_offset - depth_scale);
    // gfx-gaps: a polygon offset (setup.rs) moves the range a little
    // past 0 or 1, which a Vulkan viewport can't hold: clamped there it
    // keeps the offset, scaled by the depth (the same near 1, where the
    // scenes' depths are)
    // gfx-final: as the emulated GPU does it, raster.frag's depth-writing
    // build works such a depth map out exactly (the cave's light patches,
    // ch3b: decals a polygon offset of -0.00015 keeps over the ground; the
    // clamp scaled the offset by the depth, and where the decal dips under
    // the ground they were cut off with hard edges).
    // FATES_GFX_DEPTH_WRITE=0 keeps the clamp
    let exact = !crate::perf::var("FATES_GFX_DEPTH_WRITE").is_ok_and(|v| v == "0");
    let depth_range = if [mapped.0, mapped.1].iter().all(|d| (0.0..=1.0).contains(d)) {
        mapped
    } else if exact && crate::perf::var("FATES_GFX_POLYGON_OFFSET").map_or(true, |v| v != "0") {
        key.writes_depth = true;
        (0.0, 1.0)
    } else if [mapped.0, mapped.1].iter().all(|d| (-0.01..=1.01).contains(d)) && crate::perf::var("FATES_GFX_POLYGON_OFFSET").map_or(true, |v| v != "0") {
        (mapped.0.clamp(0.0, 1.0), mapped.1.clamp(0.0, 1.0))
    } else {
        (0.0, 1.0)
    };
    let mut placing = crate::hfr::placing(setup, &program); // gfx-hfr
    // gfxmodels3: the texture coordinates' matrices (setup.rs mutable) can
    // count as placing the vertices when the program reads its uniforms
    // through an address register (a skinned mesh's bone palette); they
    // place the textures. A 3D face's eyes blink by stepping the eye
    // texture's translation from one cell of its sheet to the next (0,
    // 7/8, 1): moved part of the way, every picture in between showed the
    // face's texture between two cells, eyes half gone or none at all,
    // "they seem to lose their face every few seconds". Counted with the
    // uniforms the textures' coordinates read, a step between sixty-
    // fourths is a step (hfr.rs steps) and the picture in between shows
    // the frame's cell; a scroll still moves. FATES_GFX_HFR_TEXROWS=0 for
    // the old way.
    if crate::perf::var("FATES_GFX_HFR_TEXROWS").map_or(true, |v| v != "0") {
        placing.1 |= placing.0 & setup.texture_rows;
    }
    Shaded {
        program,
        shading,
        draw,
        tables,
        vertices,
        indices: Arc::new(indices), // perf3
        textures,
        mips,
        key,
        viewport: [vx, vy, vw as i32, vh as i32],
        scissor,
        depth_range,
        blend_color: r[0x103].to_le_bytes().map(|c| c as f32 / 255.0),
        placing, // gfx-hfr
        loose: false, // gfx-hfr
    }
}

/// The program words a setup holds, with its descriptors, as shade.vert's
/// Program block takes them.
pub fn program(setup: &Setup) -> Vec<u32> {
    let mut p = setup.program.to_vec(); // perf3: Arc
    p.extend_from_slice(&setup.descriptors);
    p
}
