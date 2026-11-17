//! drawdist: far grass drawn in batches (Full Draw Distance,
//! overrides/drawdist.c).
//!
//! A field's grass is hundreds to thousands of small tufts (the prologue's
//! battle field: 3,969). The game draws only the few its pools hold; drawn
//! one by one (a Model::DrawOpa each), the rest cost the game's thread about
//! 30 us a tuft and the presenter one draw each, every picture. So
//! drawdist.c draws a kind of tuft once, with a model of that kind placed at
//! one of them (the carrier), and hands the renderer, in `Batch`, every
//! other tuft that draw stands for: its offset from the carrier and its own
//! wind. Each sub-mesh draw the carrier makes becomes one instanced draw:
//!   - the instances go after the vertex bytes (the Inputs buffer): a
//!     header (count, the bone palette's first float uniform and its rows,
//!     the wind's uniform) and 32 bytes an instance (offset x y z, wind x y
//!     z);
//!   - the Shading block's pad[1] (wide2's, read by wide2 only when pad[0]
//!     is set, which a 3D draw never has) holds the header's byte + 1;
//!   - shade.vert (`SHADE_VERT_*`, replaced in render/shaded.rs as the
//!     haze is) adds the offset to every palette row's translation and puts
//!     the instance's wind in the wind uniform;
//!   - the draw is recorded with that many instances (`instances`).
//! The tufts' world matrices are translations only (PlantModel's), so moving
//! a palette's translation moves the tuft. The draw is `loose` for the
//! pictures in between (matched by program and state, as particles are),
//! since the set of tufts in it changes as the camera moves.

use std::sync::atomic::{AtomicPtr, Ordering};

use zakuro_core::System;

use crate::scene::Shaded;

/// the most instances one batch holds (drawdist.c's DD_BATCH_MAX)
pub const MAX: usize = 4096;

/// drawdist.c's `drawdist_batch`, filled before the carrier's DrawOpa
#[repr(C)]
pub struct Batch {
    /// nonzero while the carrier draws
    pub active: u32,
    pub count: u32,
    /// the float uniform the wind goes in (WindCallback +0x1A), or 0xFF
    pub wind_reg: u32,
    /// nonzero: the tufts are far; alpha-tested at one sample a pixel
    pub far: u32,
    /// offset x y z, 0, wind x y z, 0
    pub inst: [[f32; 8]; MAX],
}

static BATCH: AtomicPtr<Batch> = AtomicPtr::new(std::ptr::null_mut());

/// the program registers drawdist.c's batch (main.rs)
pub fn set_batch(batch: *mut Batch) {
    BATCH.store(batch, Ordering::Relaxed);
}

fn r8(system: &mut System, a: u32) -> u32 {
    crate::prim::r8(system, a) as u32
}

/// the carrier's sub-mesh draw made into a batch's instanced draw, when a
/// batch is being drawn (the game's thread, inside the carrier's DrawOpa)
pub fn apply(system: &mut System, o: &crate::setup::Objects, sub: u32, shaded: &mut Shaded) {
    let p = BATCH.load(Ordering::Relaxed);
    if p.is_null() {
        return;
    }
    // SAFETY: drawdist.c's static, written by the same thread before the call
    let batch = unsafe { &*p };
    if batch.active == 0 || batch.count == 0 {
        return;
    }
    if shaded.shading.len() < 404 || shaded.shading[402] != 0 {
        return;
    }
    // the palette: DrawMesh's world matrix or the sub-mesh's bones, 3 rows each
    let palette = r8(system, o.bind + 0x22);
    let skinned = r8(system, o.res_model) & 2 != 0;
    let rows = if crate::prim::r32(system, o.mesh + 0x14) == 0 || !skinned {
        3
    } else {
        let sa = crate::prim::r32(system, o.mesh + 0x10) + sub * 0x34;
        let flags = r8(system, sa);
        let count = crate::prim::r16(system, sa + 2) as u32;
        if flags & 3 != 0 { 3 * count.clamp(1, 20) } else { 3 }
    };
    let n = (batch.count as usize).min(MAX);
    let mut bytes = Vec::with_capacity(shaded.vertices.len() + 24 + n * 32);
    bytes.extend_from_slice(&shaded.vertices);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    let header = bytes.len() as u32;
    for w in [n as u32, palette, rows, (batch.wind_reg & 0xFFFF) | ((batch.far != 0) as u32) << 16] {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    for i in &batch.inst[..n] {
        for f in i {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0; 8]);
    shaded.vertices = std::sync::Arc::new(bytes);
    shaded.shading[403] = header + 1;
    shaded.loose = true;
}

/// how many instances a shaded draw is recorded with
pub fn instances(s: &Shaded) -> u32 {
    if s.shading.len() < 404 || s.shading[402] != 0 || s.shading[403] == 0 {
        return 1;
    }
    let at = (s.shading[403] - 1) as usize;
    s.vertices.get(at..at + 4).map_or(1, |b| u32::from_le_bytes(b.try_into().unwrap()).max(1))
}

/// whether a shaded draw is a batch of far tufts: their alpha test is not
/// shaded per sample (a far tuft is a few pixels; at 8x MSAA per-sample
/// shading of thousands of them is most of the GPU's time)
pub fn far_batch(s: &Shaded) -> bool {
    if s.shading.len() < 404 || s.shading[402] != 0 || s.shading[403] == 0 {
        return false;
    }
    let at = (s.shading[403] - 1) as usize + 12;
    s.vertices.get(at..at + 4).is_some_and(|b| u32::from_le_bytes(b.try_into().unwrap()) >> 16 & 1 != 0)
}

/// whether a shaded draw is a batch of tufts
pub fn is_batch(s: &Shaded) -> bool {
    instances(s) > 1 || (s.shading.len() >= 404 && s.shading[402] == 0 && s.shading[403] != 0)
}

/// shade.vert: the instance's palette offset and wind, before the input
/// registers are read (main's first lines)
pub const SHADE_VERT_MAIN: (&str, &str) = (
    "void main() {\n    // each input register worked out once, operands read them many times\n",
    "void main() {
    // drawdist: a batch of tufts, the instance's offset and wind
    if (pad[0] == 0u && pad[1] != 0u) {
        uint h = pad[1] - 1u;
        dd_pal = int(word_at(h + 4u));
        dd_rows = int(word_at(h + 8u));
        dd_wind = int(word_at(h + 12u) & 0xFFFFu);
        uint at = h + 16u + uint(gl_InstanceIndex) * 32u;
        dd_offsets = float[3](uintBitsToFloat(word_at(at)), uintBitsToFloat(word_at(at + 4u)), uintBitsToFloat(word_at(at + 8u)));
        dd_wind_value = vec3(uintBitsToFloat(word_at(at + 16u)), uintBitsToFloat(word_at(at + 20u)), uintBitsToFloat(word_at(at + 24u)));
        dd_on = true;
    }
    // each input register worked out once, operands read them many times\n",
);

/// shade.vert: the globals, before input_register
pub const SHADE_VERT_GLOBALS: (&str, &str) = (
    "vec4 input_register(uint r) {",
    "// drawdist: a batch's instance (drawdist.rs)
bool dd_on = false;
int dd_pal = 0;
int dd_rows = 0;
int dd_wind = -1;
float dd_offsets[3] = float[3](0.0, 0.0, 0.0);
vec3 dd_wind_value = vec3(0.0);

vec4 input_register(uint r) {",
);

/// shade.vert: a uniform as the instance has it
pub const SHADE_VERT_UNIFORM: (&str, &str) = (
    "return uniform_index >= 0 && uniform_index < 96 ? floats[uniform_index] : vec4(0.0);",
    "vec4 dd_u = uniform_index >= 0 && uniform_index < 96 ? floats[uniform_index] : vec4(0.0);
    if (dd_on) {
        uint k = uint(uniform_index - dd_pal);
        if (k < uint(dd_rows)) {
            dd_u.w += dd_offsets[k % 3u];
        }
        if (uniform_index == dd_wind) {
            dd_u.xyz = dd_wind_value;
        }
    }
    return dd_u;",
);
