//! gfx-models: the H3D meshes the native path translated, kept only while
//! guest memory still holds what they were translated from.
//!
//! The native path translates a mesh (h3d::mesh: its vertex layout, vertex
//! bytes, sub-meshes, bone lists and index buffers) once, by the address of
//! its MeshData. That address is heap memory: when a battle ends, its
//! models are freed, and the next battle's (other characters, another
//! map's terrain) are loaded into the same places. Before this module the
//! translation was kept by address forever, so a mesh at an address that
//! had held another one was drawn with the old mesh's vertices, indices and
//! bone lists under the new model's skeleton: vertices bound to the wrong
//! bones shot out as long beams, heads lost their hair, legs and horses'
//! legs disappeared, terrain showed another map's pieces. A replay loaded
//! from a state just before one battle never saw it (nothing had been
//! translated before), a session played from boot did.
//!
//! A translation is now kept with:
//! - a signature of the bytes it was read from that are cheap to read
//!   every draw: the MeshData itself, its vertex-layout command blob, its
//!   sub-mesh table and each sub-mesh's command blob (bone lists, index
//!   buffer, primitive), and for a culled mesh its model's culling entry;
//! - page watches (Memory::watch_physical, as the texture cache's) on its
//!   vertex buffer and index buffers, which any write ends: the CPU's, a
//!   DMA, a file read.
//! A draw whose signature differs or whose watches ended translates the
//! mesh again.

use std::collections::HashMap;
use std::sync::Arc;

use zakuro_core::System;

use crate::h3d;
use crate::prim::{r32, rd};

/// the most bytes of one command blob the signature reads.
const MOST_BLOB: u32 = 0x1000;

struct Entry {
    mesh: Arc<h3d::Mesh>,
    vertices: Arc<Vec<u8>>,
    signature: u64,
    /// the vertex and index buffers' physical ranges, and the watch on them
    buffers: Vec<(u32, u32)>,
    watch: u64,
}

/// the meshes translated so far, by MeshData address, see the module docs.
#[derive(Default)]
pub struct Meshes {
    entries: HashMap<u32, Entry>,
    /// draws that found their translation still good
    pub hits: u64,
    /// translations made
    pub translated: u64,
    /// translations made again because the address held another mesh now
    /// (the signature changed): each was drawn wrong before gfx-models
    pub replaced: u64,
    /// translations made again because the buffers were written
    pub rewritten: u64,
    /// of those, the ones whose vertices or indices differed (drawn wrong
    /// before gfx-models)
    pub rewritten_differ: u64,
    /// of the replaced, the ones whose translation differs (drawn wrong
    /// before gfx-models)
    pub replaced_differ: u64,
    /// time spent checking and translating, nanoseconds
    pub spent_ns: u64,
}

fn mix(hash: u64, bytes: &[u8]) -> u64 {
    let mut h = hash;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
    h
}

fn blob(system: &mut System, hash: u64, addr: u32, words: u32) -> u64 {
    let len = (words.saturating_mul(4)).min(MOST_BLOB);
    let mut raw = vec![0u8; len as usize];
    rd(system, addr, &mut raw);
    mix(mix(hash, &addr.to_le_bytes()), &raw)
}

/// the signature of a mesh's small structures, see the module docs.
fn signature(system: &mut System, addr: u32) -> u64 {
    let mut header = [0u8; 0x30];
    rd(system, addr, &mut header);
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    let (enable, enable_n) = (word(0x8), word(0xC));
    let (subs, subs_n) = (word(0x10), word(0x14).min(256));
    let model = word(0x2C);
    let mut h = mix(0x9E37_79B9_7F4A_7C15, &header);
    h = blob(system, h, enable, enable_n);
    let mut table = vec![0u8; (subs_n * 0x34) as usize];
    rd(system, subs, &mut table);
    h = mix(h, &table);
    for s in 0..subs_n as usize {
        let at = s * 0x34;
        let cmd = u32::from_le_bytes(table[at + 0x2C..at + 0x30].try_into().unwrap());
        let cmd_n = u32::from_le_bytes(table[at + 0x30..at + 0x34].try_into().unwrap());
        h = blob(system, h, cmd, cmd_n);
    }
    if subs_n == 0 && model != 0 {
        // gfx-terrain's culled meshes: the model's culling list and this
        // mesh's entry in it
        let (list, count) = (r32(system, model + 0x68), r32(system, model + 0x6C));
        h = mix(h, &[list.to_le_bytes(), count.to_le_bytes()].concat());
        let entry = h3d::culling_entry(system, addr, model);
        let mut raw = [0u8; 0x1C];
        rd(system, entry, &mut raw);
        h = mix(h, &raw);
    }
    h
}

/// the physical ranges of a translated mesh's vertex buffer and index
/// buffers.
fn buffers(system: &mut System, addr: u32, mesh: &h3d::Mesh) -> Vec<(u32, u32)> {
    let mut out = vec![(mesh.buffer_phys, mesh.vertices.len() as u32)];
    let (enable, enable_n) = (r32(system, addr + 8), r32(system, addr + 0xC));
    let base = h3d::commands(system, enable, enable_n).iter().rev().find(|(r, _)| *r == 0x200).map_or(0, |&(_, v)| v);
    let (subs, subs_n) = (r32(system, addr + 0x10), r32(system, addr + 0x14).min(256));
    for s in 0..subs_n {
        let sa = subs + s * 0x34;
        let (cmd, cmd_n) = (r32(system, sa + 0x2C), r32(system, sa + 0x30));
        let (mut ib, mut n) = (0u32, 0u32);
        for (r, v) in h3d::commands(system, cmd, cmd_n) {
            match r {
                0x227 => ib = v,
                0x228 => n = v,
                _ => {}
            }
        }
        let wide = ib >> 31 != 0;
        out.push((base.wrapping_mul(8).wrapping_add(ib & 0x0FFF_FFFF), n.min(0x10000) << wide as u32));
    }
    if subs_n == 0 {
        // a culled mesh's one index range (h3d culled_submesh)
        if let Some(sub) = mesh.submeshes.first().filter(|s| s.culled) {
            let model = mesh.model;
            let entry = h3d::culling_entry(system, addr, model);
            let ranges = r32(system, entry + 0x10);
            let first = r32(system, ranges);
            let wide = first >> 31 != 0;
            out.push((base.wrapping_mul(8).wrapping_add(first & 0x0FFF_FFFF), (sub.indices.len() as u32) << wide as u32));
        }
    }
    out.retain(|&(_, len)| len > 0);
    out
}

/// whether two translations draw differently.
fn differs(a: &h3d::Mesh, b: &h3d::Mesh) -> bool {
    a.vertices != b.vertices
        || a.stride != b.stride
        || a.attrs.len() != b.attrs.len()
        || a.attrs.iter().zip(b.attrs.iter()).any(|(x, y)| x.offset != y.offset || x.format != y.format || x.elements != y.elements || x.scale != y.scale)
        || a.submeshes.len() != b.submeshes.len()
        || a.submeshes.iter().zip(b.submeshes.iter()).any(|(x, y)| x.indices != y.indices || x.bones != y.bones || x.skinning != y.skinning || x.primitive != y.primitive)
}

impl Meshes {
    /// the translation of the mesh at `addr` and its vertex bytes, as
    /// memory holds it now: the one kept when nothing it was read from
    /// changed, else translated again (the flag: translated now).
    pub fn get(&mut self, system: &mut System, addr: u32) -> Option<(Arc<h3d::Mesh>, Arc<Vec<u8>>, bool)> {
        let start = std::time::Instant::now();
        let out = self.get_timed(system, addr);
        self.spent_ns += start.elapsed().as_nanos() as u64;
        out
    }

    fn get_timed(&mut self, system: &mut System, addr: u32) -> Option<(Arc<h3d::Mesh>, Arc<Vec<u8>>, bool)> {
        // FATES_GFX_MODELS=0: dev, the old way (a translation kept by
        // address however memory changed), for before/after checks
        if crate::perf::var("FATES_GFX_MODELS").is_ok_and(|v| v == "0") {
            if let Some(e) = self.entries.get(&addr) {
                self.hits += 1;
                return Some((e.mesh.clone(), e.vertices.clone(), false));
            }
        }
        let signature = signature(system, addr);
        let mut old_mesh = None;
        if let Some(e) = self.entries.get(&addr) {
            let same = e.signature == signature;
            if same && e.buffers.iter().all(|&(p, len)| system.memory.unchanged_physical(p, len, e.watch)) {
                self.hits += 1;
                return Some((e.mesh.clone(), e.vertices.clone(), false));
            }
            let old = Some((e.mesh.clone(), same));
            if same {
                self.rewritten += 1;
            } else {
                self.replaced += 1;
                if crate::perf::var_os("FATES_GFX_MESH_LOG").is_some() {
                    eprintln!("gfx-models: mesh {addr:08X} holds another mesh now, translated again");
                }
            }
            self.entries.remove(&addr);
            old_mesh = old;
        }
        let mesh = Arc::new(h3d::mesh(system, addr)?);
        self.translated += 1;
        if let Some((old, same)) = old_mesh {
            let differ = differs(&old, &mesh);
            match same {
                true => self.rewritten_differ += differ as u64,
                false => self.replaced_differ += differ as u64,
            }
            if differ && crate::perf::var_os("FATES_GFX_MESH_LOG").is_some() {
                eprintln!("gfx-models: mesh {addr:08X} is another mesh now ({} vertex bytes, was {}), translated again", mesh.vertices.len(), old.vertices.len());
            }
        }
        let buffers = buffers(system, addr, &mesh);
        let mut watch = 0;
        for &(p, len) in &buffers {
            watch = system.memory.watch_physical(p, len);
        }
        // the watches are numbered in order: the last covers the others
        // (a page already watched keeps its older number, which is lower)
        let vertices = Arc::new(mesh.vertices.clone());
        if self.entries.len() > 16384 {
            self.entries.clear();
        }
        self.entries.insert(addr, Entry { mesh: mesh.clone(), vertices: vertices.clone(), signature, buffers, watch });
        Some((mesh, vertices, true))
    }

    /// the report line's counts.
    pub fn line(&self) -> String {
        format!(
            "gfx-models: meshes translated {} ({} again for another mesh at the same address, {} of them drawn otherwise; {} for written buffers, {} of them with other contents), draws that kept theirs {}",
            self.translated, self.replaced, self.replaced_differ, self.rewritten, self.rewritten_differ, self.hits
        ) + &format!(
            ", {:.1} ms in all, skeleton matrices that place nothing {}",
            self.spent_ns as f64 / 1e6,
            crate::setup::BAD_BONES.load(std::sync::atomic::Ordering::Relaxed)
        )
    }
}

