//! NintendoWare H3D (BCH) resources as the game holds them in memory,
//! translated once into native descriptions.
//!
//! The game relocates each BCH file in place (nw::h3d::res::ResourceBinary,
//! 0x003233F4), so its structures sit in guest memory with absolute
//! pointers, in the layout SPICA (gdkchan/SPICA, SPICA.Formats.CtrH3D)
//! documents: lists are (pointer, count) pairs. Some state exists only as
//! the PICA command blobs the BCH ships (vertex layouts, index buffers,
//! material state); those blobs are decoded here once per resource, the
//! way SPICA's deserializer decodes them, never per frame.

use std::collections::HashMap;

use zakuro_core::System;

use crate::prim::{r16, r32, r8, rd};

/// PICA registers the H3D command blobs set.
mod reg {
    pub const ATTRIBBUFFERS_LOC: u16 = 0x200;
    pub const ATTRIBBUFFERS_FORMAT_LOW: u16 = 0x201;
    pub const ATTRIBBUFFERS_FORMAT_HIGH: u16 = 0x202;
    pub const ATTRIBBUFFER0_OFFSET: u16 = 0x203;
    pub const ATTRIBBUFFER0_CONFIG1: u16 = 0x204;
    pub const ATTRIBBUFFER0_CONFIG2: u16 = 0x205;
    pub const INDEXBUFFER_CONFIG: u16 = 0x227;
    pub const NUMVERTICES: u16 = 0x228;
    pub const FIXEDATTRIB_INDEX: u16 = 0x232;
    pub const FIXEDATTRIB_DATA0: u16 = 0x233;
    pub const FIXEDATTRIB_DATA1: u16 = 0x234;
    pub const FIXEDATTRIB_DATA2: u16 = 0x235;
    pub const PRIMITIVE_CONFIG: u16 = 0x25E;
    pub const VSH_BOOLUNIFORM: u16 = 0x2B0;
    pub const VSH_NUM_ATTR: u16 = 0x2B9;
    pub const VSH_ATTRIBUTES_PERMUTATION_LOW: u16 = 0x2BB;
    pub const VSH_ATTRIBUTES_PERMUTATION_HIGH: u16 = 0x2BC;
    pub const VSH_FLOATUNIFORM_INDEX: u16 = 0x2C0;
    pub const VSH_FLOATUNIFORM_DATA0: u16 = 0x2C1;
    pub const VSH_FLOATUNIFORM_DATA7: u16 = 0x2C8;
}

/// Reads a PICA command blob into its register writes (register, value),
/// in order, as SPICA's PICACommandReader does. Byte-masked writes keep
/// only their value; the masks of these resource blobs are full.
pub fn commands(system: &mut System, addr: u32, words: u32) -> Vec<(u16, u32)> {
    let words = words.min(0x10000);
    let mut raw = vec![0u8; words as usize * 4];
    rd(system, addr, &mut raw);
    let w: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < w.len() {
        let first = w[i];
        let header = w[i + 1];
        let id = (header & 0xFFFF) as u16;
        let extra = ((header >> 20) & 0xFF) as usize;
        let consecutive = header >> 31 != 0;
        out.push((id, first));
        let mut reg = id;
        for k in 0..extra {
            let Some(&v) = w.get(i + 2 + k) else { break };
            if consecutive {
                reg += 1;
            }
            out.push((reg, v));
        }
        // a command is the parameter, the header and the extra
        // parameters, padded to 8 bytes
        i += 2 + extra + (extra & 1);
    }
    out
}

/// A float24 vector as the PICA's uniform uploads pack it.
fn f24(v: u32) -> f32 {
    let v = v & 0xFF_FFFF;
    let sign = (v >> 23) & 1;
    let exp = (v >> 16) & 0x7F;
    if exp == 0 {
        return if sign != 0 { -0.0 } else { 0.0 };
    }
    let mant = v & 0xFFFF;
    let bits = (sign << 31) | ((exp + 64) << 23) | (mant << 7);
    f32::from_bits(bits)
}

/// A vec4 the PICA packs as three words of float24s (uniform uploads and
/// fixed attributes): w in the top of the first word, x in the bottom of
/// the last.
fn unpack24(a: u32, b: u32, c: u32) -> [f32; 4] {
    let w = a >> 8;
    let z = ((a & 0xFF) << 16) | (b >> 16);
    let y = ((b & 0xFFFF) << 8) | (c >> 24);
    let x = c & 0xFF_FFFF;
    [f24(x), f24(y), f24(z), f24(w)]
}

/// The vertex-shader float uniforms a blob uploads (register -> value).
pub fn float_uniforms(cmds: &[(u16, u32)]) -> HashMap<u32, [f32; 4]> {
    let mut out = HashMap::new();
    let mut index = 0u32;
    let mut f32_mode = false;
    let mut pending: Vec<u32> = Vec::new();
    for &(r, v) in cmds {
        if r == reg::VSH_FLOATUNIFORM_INDEX {
            index = v & 0xFF;
            f32_mode = v >> 31 != 0;
            pending.clear();
        } else if (reg::VSH_FLOATUNIFORM_DATA0..=reg::VSH_FLOATUNIFORM_DATA7).contains(&r) {
            pending.push(v);
            if f32_mode && pending.len() == 4 {
                // w, z, y, x
                let f = |k: usize| f32::from_bits(pending[k]);
                out.insert(index, [f(3), f(2), f(1), f(0)]);
                index += 1;
                pending.clear();
            } else if !f32_mode && pending.len() == 3 {
                out.insert(index, unpack24(pending[0], pending[1], pending[2]));
                index += 1;
                pending.clear();
            }
        }
    }
    out
}

/// PICA vertex attribute names (SPICA PICAAttributeName).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrName {
    Position,
    Normal,
    Tangent,
    Color,
    TexCoord0,
    TexCoord1,
    TexCoord2,
    BoneIndex,
    BoneWeight,
    UserAttribute(u8),
}

impl AttrName {
    /// the vertex-shader input register the attribute feeds
    pub fn reg(self) -> u32 {
        match self {
            AttrName::Position => 0,
            AttrName::Normal => 1,
            AttrName::Tangent => 2,
            AttrName::Color => 3,
            AttrName::TexCoord0 => 4,
            AttrName::TexCoord1 => 5,
            AttrName::TexCoord2 => 6,
            AttrName::BoneIndex => 7,
            AttrName::BoneWeight => 8,
            AttrName::UserAttribute(n) => n as u32,
        }
    }

    fn from(v: u32) -> AttrName {
        match v {
            0 => AttrName::Position,
            1 => AttrName::Normal,
            2 => AttrName::Tangent,
            3 => AttrName::Color,
            4 => AttrName::TexCoord0,
            5 => AttrName::TexCoord1,
            6 => AttrName::TexCoord2,
            7 => AttrName::BoneIndex,
            8 => AttrName::BoneWeight,
            n => AttrName::UserAttribute(n as u8),
        }
    }
}

/// One attribute in the vertex buffer: format 0 byte, 1 ubyte, 2 short,
/// 3 float; `elements` 1..4; `offset` within the vertex; `scale` the factor
/// the shader multiplies it by.
#[derive(Clone, Copy, Debug)]
pub struct Attr {
    pub name: AttrName,
    pub format: u8,
    pub elements: u8,
    pub offset: u32,
    pub scale: f32,
}

#[derive(Clone, Debug)]
pub struct SubMesh {
    /// 0 none, 1 rigid, 2 smooth (H3DSubMeshSkinning)
    pub skinning: u8,
    pub bones: Vec<u16>,
    pub indices: Vec<u16>,
    /// PICA primitive mode (0 triangles, 1 strip, 2 fan, 3 geometry)
    pub primitive: u32,
    pub bool_uniforms: u16,
    /// gfx-terrain: drawn through the model's sub-mesh culling (H3DModel
    /// SubMeshCullings): no command blob of its own, its booleans set by
    /// MeshData::MakeCommand (0x0051E0F8)
    pub culled: bool,
}

/// A mesh translated once: its vertex layout, the vertex bytes and its
/// sub-meshes.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub material: u16,
    pub flags: u8,
    pub node: u16,
    pub key: u16,
    pub stride: u32,
    pub attrs: Vec<Attr>,
    /// fixed attributes (name, xyzw)
    pub fixed: Vec<(AttrName, [f32; 4])>,
    /// where the vertex buffer is (physical, as the blob names it) and its bytes
    pub buffer_phys: u32,
    pub vertices: Vec<u8>,
    pub position_offset: [f32; 4],
    pub submeshes: Vec<SubMesh>,
    /// the model (H3DModel) the mesh belongs to
    pub model: u32,
}

/// gfx-terrain: a mesh's index in its model's mesh list, as
/// MeshData::MakeCommand (0x0051E0F8) computes it: the byte distance from
/// the list (H3DModel +0x40) divided by the MeshData size with the
/// multiplier in its literal 0x0051E258 (smlal, then >> 5).
fn culling_index(system: &mut System, addr: u32, model: u32) -> u32 {
    let diff = addr.wrapping_sub(r32(system, model + 0x40)) as i32 as i64;
    let magic = r32(system, zakuro_core::region::us(0x0051_E258)) as i32 as i64;
    let hi = (((diff << 32) + magic * diff) >> 32) as i32;
    (((hi >> 5) - (hi >> 31)) as u32) & 0xFFFF
}

/// gfx-terrain: the sub-mesh culling entry of a mesh without sub-meshes
/// (H3DModel +0x68, 0x1C bytes each: +0 the node tree, +4 its size, +8
/// the nodes' bounds, +0x10 the index ranges (index-buffer word, count),
/// +0x14 their number, +0x18 the booleans), as one sub-mesh of all its
/// ranges. MeshData::MakeCommand (0x0051E0F8) draws the runs of ranges
/// whose nodes intersect the view volume (SubMeshCullingData::
/// TestSubMeshIntersection 0x0051CD28), each as one draw from its first
/// range's start to its last range's end; the ranges are consecutive in
/// one index buffer, so all of them make one draw, and what culling
/// leaves out is outside the view anyway. Triangles: the culled draw
/// writes no primitive mode, and the PICA's is left at 3 (triangles from
/// an index buffer).
fn culled_submesh(system: &mut System, addr: u32, model: u32, base: u32) -> Option<SubMesh> {
    if culling_index(system, addr, model) >= r32(system, model + 0x6C) {
        return None;
    }
    let entry = culling_entry(system, addr, model);
    let (ranges, n) = (r32(system, entry + 0x10), r32(system, entry + 0x14));
    if ranges == 0 || n == 0 || n > 0x1000 {
        return None;
    }
    let first = r32(system, ranges);
    let (last, last_count) = (r32(system, ranges + (n - 1) * 8), r32(system, ranges + (n - 1) * 8 + 4));
    let wide = first >> 31 != 0;
    let count = ((last.wrapping_sub(first) & 0x0FFF_FFFF) >> wide as u32) + last_count;
    let count = count.min(0x40000);
    let ib_virt = virt(system, base.wrapping_mul(8).wrapping_add(first & 0x0FFF_FFFF));
    let mut raw = vec![0u8; (count * if wide { 2 } else { 1 }) as usize];
    rd(system, ib_virt, &mut raw);
    let indices: Vec<u16> =
        if wide { raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() } else { raw.iter().map(|&b| b as u16).collect() };
    Some(SubMesh { skinning: 0, bones: Vec::new(), indices, primitive: 0, bool_uniforms: r16(system, entry + 0x18), culled: true })
}

/// gfx-terrain: the address of a mesh's sub-mesh culling entry.
pub fn culling_entry(system: &mut System, addr: u32, model: u32) -> u32 {
    let index = culling_index(system, addr, model);
    r32(system, model + 0x68).wrapping_add(index * 0x1C)
}

/// physical address to the game's virtual one.
fn virt(system: &System, phys: u32) -> u32 {
    crate::prim::virt(system, phys)
}

/// Translates the h3d MeshData (SPICA H3DMesh) at `addr`.
pub fn mesh(system: &mut System, addr: u32) -> Option<Mesh> {
    let material = r16(system, addr);
    let flags = r8(system, addr + 2);
    let node = r16(system, addr + 4);
    let key = r16(system, addr + 6);
    let (enable, enable_n) = (r32(system, addr + 8), r32(system, addr + 0xC));
    let (subs, subs_n) = (r32(system, addr + 0x10), r32(system, addr + 0x14));
    let model = r32(system, addr + 0x2C);
    if enable == 0 || subs_n > 256 {
        return None;
    }
    let cmds = commands(system, enable, enable_n);
    let uniforms = float_uniforms(&cmds);
    if crate::perf::var_os("FATES_GFX_DEBUG3D_DUMP").is_some() { // gfx-perf: cached env
        let regs: Vec<String> = cmds.iter().map(|(r, v)| format!("{r:03X}={v:08X}")).collect();
        eprintln!("gfx-d3 mesh {addr:08X} cmds {} uniforms {:?}", regs.join(" "), uniforms);
    }
    let mut base = 0u32;
    let mut offset = 0u32;
    let (mut formats, mut attributes, mut permutation) = (0u64, 0u64, 0u64);
    let mut stride = 0u32;
    let mut total = 0usize;
    let mut fixed_index = 0usize;
    let mut fixed_words = [[0u32; 3]; 12];
    for &(r, v) in &cmds {
        match r {
            reg::ATTRIBBUFFERS_LOC => base = v,
            reg::ATTRIBBUFFERS_FORMAT_LOW => formats |= v as u64,
            reg::ATTRIBBUFFERS_FORMAT_HIGH => formats |= (v as u64) << 32,
            reg::ATTRIBBUFFER0_OFFSET => offset = v,
            reg::ATTRIBBUFFER0_CONFIG1 => attributes |= v as u64,
            reg::ATTRIBBUFFER0_CONFIG2 => {
                attributes |= ((v & 0xFFFF) as u64) << 32;
                stride = (v >> 16) & 0xFF;
            }
            reg::FIXEDATTRIB_INDEX => fixed_index = (v & 0xF) as usize,
            reg::FIXEDATTRIB_DATA0 => fixed_words[fixed_index.min(11)][0] = v,
            reg::FIXEDATTRIB_DATA1 => fixed_words[fixed_index.min(11)][1] = v,
            reg::FIXEDATTRIB_DATA2 => fixed_words[fixed_index.min(11)][2] = v,
            reg::VSH_NUM_ATTR => total = (v as usize & 0xF) + 1,
            reg::VSH_ATTRIBUTES_PERMUTATION_LOW => permutation |= v as u64,
            reg::VSH_ATTRIBUTES_PERMUTATION_HIGH => permutation |= (v as u64) << 32,
            _ => {}
        }
    }
    let scale7 = uniforms.get(&7).copied().unwrap_or([1.0; 4]);
    let scale8 = uniforms.get(&8).copied().unwrap_or([1.0; 4]);
    let mut attrs = Vec::new();
    let mut fixed = Vec::new();
    let mut at = 0u32;
    for index in 0..total {
        if (formats >> (48 + index)) & 1 != 0 {
            let name = AttrName::from(((permutation >> (index * 4)) & 0xF) as u32);
            let [a, b, c] = fixed_words[index.min(11)];
            fixed.push((name, unpack24(a, b, c)));
        } else {
            let perm = ((attributes >> (index * 4)) & 0xF) as usize;
            let name = AttrName::from(((permutation >> (perm * 4)) & 0xF) as u32);
            let fmt = ((formats >> (perm * 4)) & 0xF) as u8;
            let (format, elements) = (fmt & 3, (fmt >> 2) + 1);
            let size = elements as u32 * [1, 1, 2, 4][format as usize];
            // components align to their own size
            let align = [1, 1, 2, 4][format as usize];
            at = at.div_ceil(align) * align;
            let scale = match name {
                AttrName::Position => scale7[0],
                AttrName::Normal => scale7[1],
                AttrName::Tangent => scale7[2],
                AttrName::Color => scale7[3],
                AttrName::TexCoord0 => scale8[0],
                AttrName::TexCoord1 => scale8[1],
                AttrName::TexCoord2 => scale8[2],
                AttrName::BoneWeight => scale8[3],
                _ => 1.0,
            };
            attrs.push(Attr { name, format, elements, offset: at, scale });
            at += size;
        }
    }
    let mut submeshes = Vec::new();
    let mut max_index = 0u32;
    for s in 0..subs_n {
        let sa = subs + s * 0x34;
        let skinning = r8(system, sa);
        let count = (r16(system, sa + 2) as usize).min(20);
        let bones = (0..count).map(|k| r16(system, sa + 4 + 2 * k as u32)).collect();
        let (cmd, cmd_n) = (r32(system, sa + 0x2C), r32(system, sa + 0x30));
        let sc = commands(system, cmd, cmd_n);
        let (mut ib, mut n, mut prim, mut bools) = (0u32, 0u32, 0u32, 0u16);
        for &(r, v) in &sc {
            match r {
                reg::INDEXBUFFER_CONFIG => ib = v,
                reg::NUMVERTICES => n = v,
                reg::PRIMITIVE_CONFIG => prim = (v >> 8) & 3,
                reg::VSH_BOOLUNIFORM => bools = v as u16,
                _ => {}
            }
        }
        let wide = ib >> 31 != 0;
        // the index buffer's offset is from the attribute buffers' base
        let ib_phys = base.wrapping_mul(8).wrapping_add(ib & 0x0FFF_FFFF);
        let ib_virt = virt(system, ib_phys);
        let n = n.min(0x10000);
        let mut raw = vec![0u8; (n * if wide { 2 } else { 1 }) as usize];
        rd(system, ib_virt, &mut raw);
        let indices: Vec<u16> =
            if wide { raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() } else { raw.iter().map(|&b| b as u16).collect() };
        max_index = max_index.max(indices.iter().copied().max().unwrap_or(0) as u32);
        submeshes.push(SubMesh { skinning, bones, indices, primitive: prim, bool_uniforms: bools, culled: false });
    }
    // gfx-terrain: a mesh without sub-meshes of its own draws through its
    // model's sub-mesh culling (battle and map terrain): one sub-mesh
    // holding every index range, in order
    if subs_n == 0 {
        if let Some(sub) = culled_submesh(system, addr, model, base) {
            max_index = max_index.max(sub.indices.iter().copied().max().unwrap_or(0) as u32);
            submeshes.push(sub);
        }
    }
    let buffer_phys = base.wrapping_mul(8).wrapping_add(offset);
    let mut vertices = vec![0u8; ((max_index + 1) * stride) as usize];
    let bv = virt(system, buffer_phys);
    rd(system, bv, &mut vertices);
    Some(Mesh {
        material,
        flags,
        node,
        key,
        stride,
        attrs,
        fixed,
        buffer_phys,
        vertices,
        position_offset: uniforms.get(&6).copied().unwrap_or([0.0; 4]),
        submeshes,
        model,
    })
}
