//! A geometry check of the native H3D path: the meshes Model::DrawMesh
//! draws in a frame, transformed on the CPU with the game's own skeleton,
//! view and projection matrices, shaded flat by their normals, and drawn by
//! the native renderer into an image beside the emulated one
//! (FATES_GFX_DEBUG3D=dir). It checks the mesh translation, skinning and
//! cameras before the real native 3D pipeline (materials, the game's vertex
//! shaders, fragment lighting) replaces the emulated one.

use std::sync::Arc;

use zakuro_core::System;

use crate::h3d::{AttrName, Mesh};
use crate::prim::{r32, Blend, Combiner, Draw};

/// one Model::DrawMesh call, with the matrices it would use.
pub struct MeshDraw {
    pub mesh: Arc<Mesh>,
    /// per sub-mesh, its bone palette (3x4 each)
    pub palettes: Vec<Vec<[f32; 12]>>,
    pub world: [f32; 12],
    pub view: [f32; 12],
    pub proj: [f32; 16],
}

fn m34(system: &mut System, addr: u32) -> [f32; 12] {
    std::array::from_fn(|i| f32::from_bits(r32(system, addr + 4 * i as u32)))
}

/// reads what a DrawMesh call needs: the skeleton's matrices for each
/// sub-mesh's bones (SkeletonState at Model +0x24, its matrices at +0x14,
/// 0x30 each, as MeshData::MakeCommand 0x0051DD30 uploads them), the
/// camera's view (+0x98, DrawMesh) and projection (+0x1A8,
/// RenderState::Change) from the RenderState's camera (+8).
pub fn read(system: &mut System, model: u32, state: u32, mesh: Arc<Mesh>, world: u32) -> MeshDraw {
    let skeleton = model + 0x24;
    let matrices = r32(system, skeleton + 0x14);
    let palettes = mesh
        .submeshes
        .iter()
        .map(|s| s.bones.iter().map(|&b| m34(system, matrices + b as u32 * 0x30)).collect())
        .collect();
    let camera = r32(system, state + 8);
    let proj: [f32; 16] = std::array::from_fn(|i| f32::from_bits(r32(system, camera + 0x1A8 + 4 * i as u32)));
    let view_at = std::env::var("FATES_GFX_DEBUG3D_VIEW").ok().and_then(|v| u32::from_str_radix(&v, 16).ok()).unwrap_or(0x98);
    MeshDraw { palettes, world: m34(system, world), view: m34(system, camera + view_at), proj, mesh }
}

fn mul34(m: &[f32; 12], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2] + m[r * 4 + 3])
}

fn mul34n(m: &[f32; 12], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2])
}

fn attr(mesh: &Mesh, vertex: usize, name: AttrName) -> Option<[f32; 4]> {
    let a = mesh.attrs.iter().find(|a| a.name == name)?;
    let o = vertex * mesh.stride as usize + a.offset as usize;
    let b = &mesh.vertices;
    let mut out = [0.0, 0.0, 0.0, 1.0];
    for c in 0..a.elements as usize {
        out[c] = match a.format {
            3 => f32::from_le_bytes(b.get(o + 4 * c..o + 4 * c + 4)?.try_into().ok()?),
            2 => i16::from_le_bytes([*b.get(o + 2 * c)?, *b.get(o + 2 * c + 1)?]) as f32,
            1 => *b.get(o + c)? as f32,
            _ => *b.get(o + c)? as i8 as f32,
        } * a.scale;
    }
    Some(out)
}

/// the draws for a frame's meshes, vertices already in clip space divided
/// by w (the native 2D pipeline takes them with identity matrices).
pub fn draws(list: &[MeshDraw], with_world: bool) -> Vec<Draw> {
    if std::env::var_os("FATES_GFX_DEBUG3D_DUMP").is_some() {
        let want = std::env::var("FATES_GFX_ORACLE").ok().and_then(|v| u32::from_str_radix(&v, 16).ok());
        for md in list.iter().filter(|md| want.is_none_or(|w| md.mesh.buffer_phys == w)).take(4) {
            eprintln!("gfx-d3 buffer {:08X} world {:?}", md.mesh.buffer_phys, md.world);
            eprintln!("gfx-d3 world {:?}
  view {:?}
  proj {:?}", md.world, md.view, md.proj);
            for (s, p) in md.palettes.iter().enumerate().take(2) {
                eprintln!("  sub {s} skin {} bones {:?} palette[0] {:?}", md.mesh.submeshes[s].skinning, md.mesh.submeshes[s].bones, p.first());
            }
        }
    }
    let mut out = Vec::new();
    for md in list {
        // the main pass only: the shadow pass (ShadowScene) draws the same
        // meshes with an orthographic camera into its shadow texture
        if md.proj[15] != 0.0 && std::env::var_os("FATES_GFX_DEBUG3D_ALL").is_none() {
            continue;
        }
        // FATES_GFX_DEBUG3D_P4=v: only the passes whose projection has
        // this y scale (one camera of several)
        if let Some(p4) = std::env::var("FATES_GFX_DEBUG3D_P4").ok().and_then(|v| v.parse::<f32>().ok()) {
            if (md.proj[4] - p4).abs() > 0.01 {
                continue;
            }
        }
        let m = &md.mesh;
        for (s, sub) in m.submeshes.iter().enumerate() {
            let palette = &md.palettes[s];
            if palette.is_empty() || sub.primitive != 0 {
                continue;
            }
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            let used: Vec<u16> = {
                let mut u = sub.indices.clone();
                u.sort_unstable();
                u.dedup();
                u
            };
            let mut remap = std::collections::HashMap::new();
            for &v in &used {
                let v = v as usize;
                let Some(p) = attr(m, v, AttrName::Position) else { continue };
                let n = attr(m, v, AttrName::Normal).unwrap_or([0.0, 0.0, 1.0, 0.0]);
                let p3 = [p[0] + m.position_offset[0], p[1] + m.position_offset[1], p[2] + m.position_offset[2]];
                // skinning: smooth blends the palette by the weights, rigid
                // and none take one matrix
                let (pos, nrm) = if sub.skinning == 2 {
                    let bi = attr(m, v, AttrName::BoneIndex).unwrap_or([0.0; 4]);
                    let bw = attr(m, v, AttrName::BoneWeight).unwrap_or([1.0, 0.0, 0.0, 0.0]);
                    let (mut pos, mut nrm) = ([0f32; 3], [0f32; 3]);
                    for k in 0..4 {
                        if bw[k] == 0.0 {
                            continue;
                        }
                        let mat = &palette[(bi[k] as usize).min(palette.len() - 1)];
                        let a = mul34(mat, p3);
                        let b = mul34n(mat, [n[0], n[1], n[2]]);
                        for c in 0..3 {
                            pos[c] += a[c] * bw[k];
                            nrm[c] += b[c] * bw[k];
                        }
                    }
                    (pos, nrm)
                } else {
                    let index = if sub.skinning == 1 {
                        attr(m, v, AttrName::BoneIndex).map_or(0, |b| b[0] as usize)
                    } else {
                        0
                    };
                    let mat = &palette[index.min(palette.len() - 1)];
                    (mul34(mat, p3), mul34n(mat, [n[0], n[1], n[2]]))
                };
                // the GPU applies the camera (view, projection) so triangles
                // that cross the near plane clip as on the PICA
                let pos = if with_world { mul34(&md.world, pos) } else { pos };
                let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt().max(1e-6);
                let light = (0.35 + 0.65 * ((nrm[0] * 0.3 + nrm[1] * 0.7 + nrm[2] * 0.6) / len).max(0.0)).min(1.0);
                let shade = (light * 255.0) as u8;
                remap.insert(v as u16, (vertices.len() / 24) as u32);
                for f in [pos[0], pos[1], pos[2], 0.0, 0.0] {
                    vertices.extend_from_slice(&f.to_le_bytes());
                }
                vertices.extend_from_slice(&[shade, shade, shade, 255]);
            }
            for tri in sub.indices.chunks_exact(3) {
                if let (Some(&a), Some(&b), Some(&c)) = (remap.get(&tri[0]), remap.get(&tri[1]), remap.get(&tri[2])) {
                    indices.extend_from_slice(&[a, b, c]);
                }
            }
            if indices.is_empty() {
                continue;
            }
            // the screen-space bounds; screen-filling quads (backdrops and
            // overlays the real materials blend) would hide everything in a
            // flat-shaded picture, so the check leaves them out
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in vertices.chunks_exact(24) {
                let p: [f32; 3] = std::array::from_fn(|c| f32::from_le_bytes(v[4 * c..4 * c + 4].try_into().unwrap()));
                let e = mul34(&md.view, p);
                let pr = &md.proj;
                let clip: [f32; 4] = std::array::from_fn(|r| pr[r * 4] * e[0] + pr[r * 4 + 1] * e[1] + pr[r * 4 + 2] * e[2] + pr[r * 4 + 3]);
                for c in 0..3 {
                    let f = clip[c] / clip[3];
                    lo[c] = lo[c].min(f);
                    hi[c] = hi[c].max(f);
                }
            }
            if std::env::var_os("FATES_GFX_DEBUG3D_DUMP").is_some() {
                eprintln!("gfx-d3 draw mesh {:08X} sub {s} skin {} verts {} ndc {lo:?}..{hi:?}", m.buffer_phys, sub.skinning, vertices.len() / 24);
            }
            if lo[0] <= -0.9 && hi[0] >= 0.9 && lo[1] <= -0.9 && hi[1] >= 0.9 {
                continue;
            }
            let identity34 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];
            out.push(Draw {
                vertices,
                indices,
                world: md.view,
                view_proj: md.proj,
                tex_matrix: identity34,
                color: [1.0; 4],
                textures: [None, None, None],
                combiner: Combiner::prim_reset(),
                blend: Blend::prim(7),
                depth_test: true,
                depth_func: 4,
                depth_write: true,
                alpha_test: false,
                alpha_func: 1,
                alpha_ref: 0,
                cull: 0,
                scissor: [0, 0, 240, 400],
                viewport: [0, 0, 240, 400],
                shaded: None,
                post: None,
                depth_offset: 0.0, // gfx-gaps
            });
        }
    }
    out
}
