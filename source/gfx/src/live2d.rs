//! jpn: Live2D models drawn natively.
//!
//! My Room's Live2D views (a visit's close-up in every build, the Japanese
//! game's petting minigame) draw a Cubism 2 model with its 3DS backend:
//! `Live2DCallBack::OnBegin`, a prim callback, runs `Live2DModelManager::Draw`,
//! and `live2d::DrawParam_3DS::drawTexture` draws each mesh through nn::gd,
//! GPU commands the pure build has no GPU for (they showed as "unavailable"
//! GPU work, and nothing of the model was drawn). overrides/gfx_live2d.c
//! hands each mesh here instead; it becomes a native 2D draw at the
//! callback's place in its display's frame (`Gfx::live2d_draw`, lib.rs),
//! as the 3D scenes' meshes are.
//!
//! What a mesh is, from the backend's disassembly (DrawParam_3DS):
//! - `xys`, `uvs`: `vertex_count` (x, y) and (u, v) float pairs; `indices`:
//!   `index_count` u16 triangle-list indices;
//! - the model matrix, an nn::math::MTX44 at +0x1C (setupDraw hands it to
//!   the shader as its matrix uniform): model space to clip space, the
//!   console's screen rotation included;
//! - the textures, nn::gd Texture2D pointers at +0x7C (by texNo); the
//!   texture colours at +0x68 (per texNo: r, g, b at +4, +8, +0xC); the
//!   base colour at +8 (RGBA floats); the opacity multiplies all of it into
//!   the combiner's constant colour;
//! - `composition`: 0 normal, 1 additive, 2 multiplicative.

use zakuro_core::System;

use crate::prim::{r16, r32, Blend, Combiner, Draw, Sampling, Stage, TexRef};

/// Live2DCallBack::OnBegin (USA), the callback class drawing a model.
pub const ON_BEGIN: u32 = 0x001D_2978;

/// The hook table overrides/gfx_live2d.c calls through, `fates_gfx_hooks_live2d`.
#[repr(C)]
#[derive(Default)]
pub struct Hooks {
    pub draw: Option<unsafe extern "C" fn(*mut std::ffi::c_void, u32, u32, u32, u32, u32, u32, u32, u32, u32) -> u32>,
}

/// FATES_GFX_LIVE2D=0 leaves Live2D to the GPU (none in the pure build).
pub fn enabled() -> bool {
    crate::perf::var("FATES_GFX_LIVE2D").map_or(true, |v| v != "0")
}

fn f32_at(system: &mut System, a: u32) -> f32 {
    f32::from_bits(r32(system, a))
}

/// dev: FATES_GFX_LIVE2D_TRACE=1 logs the first meshes and the objects
/// they come from.
fn trace(system: &mut System, this: u32, tex: u32) {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if crate::perf::var_os("FATES_GFX_LIVE2D_TRACE").is_none() || COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 3 {
        return;
    }
    let words = |system: &mut System, a: u32, n: u32| (0..n).map(|k| format!("{:08X}", r32(system, a + 4 * k))).collect::<Vec<_>>().join(" ");
    let dw = words(system, this, 0x30);
    eprintln!("live2d: drawparam {this:08X}: {dw}");
    let textures = r32(system, this + 0x7C);
    let t = r32(system, textures + tex * 4);
    let tw = words(system, t, 16);
    eprintln!("live2d: texture {tex} object {t:08X}: {tw}");
    for k in 0..4 {
        let p = r32(system, t + 4 * k);
        if p > 0x0010_0000 && p < 0x4000_0000 {
            let pw = words(system, p, 16);
            eprintln!("live2d:   +{:X} -> {p:08X}: {pw}", 4 * k);
        }
    }
    let colors = r32(system, this + 0x68);
    let c = r32(system, colors + tex * 4);
    let cw = words(system, c, 6);
    eprintln!("live2d: texture colour {c:08X}: {cw}");
}

/// The texture of an nn::gd Texture2D, as the backend's textures are made
/// (Live2DModelManager::ReadTex), read from a run (FATES_GFX_LIVE2D_TRACE):
/// +4 width, +8 height, +0x14 the PICA format (0xD, ETC1A4, for the
/// models' 1024x1024 sheets), +0x20 the texels' virtual address (+0x2C the
/// same as the PICA register holds it, physical >> 3).
fn texture(system: &mut System, object: u32) -> Option<TexRef> {
    if object == 0 {
        return None;
    }
    let width = r32(system, object + 4);
    let height = r32(system, object + 8);
    let format = r32(system, object + 0x14);
    let addr = r32(system, object + 0x20);
    if addr == 0 || !(8..=1024).contains(&width) || !(8..=1024).contains(&height) || format > 0xD {
        return None;
    }
    Some(TexRef { addr, width, height, format })
}

/// A mesh as a native draw for its callback's display.
#[allow(clippy::too_many_arguments)]
pub fn mesh(system: &mut System, this: u32, tex: u32, index_count: u32, vertex_count: u32, indices: u32, xys: u32, uvs: u32, opacity: f32, composition: u32, disp: u32) -> Option<Draw> {
    trace(system, this, tex);
    if index_count == 0 || vertex_count == 0 || index_count > 0x10000 || vertex_count > 0x10000 || opacity <= 0.0 {
        return None;
    }
    let textures = r32(system, this + 0x7C);
    let object = r32(system, textures + tex * 4);
    let texture = texture(system, object)?;
    // the constant colour: base colour x texture colour x opacity
    let colors = r32(system, this + 0x68);
    let tc = r32(system, colors + tex * 4);
    let base = [f32_at(system, this + 8), f32_at(system, this + 0xC), f32_at(system, this + 0x10), f32_at(system, this + 0x14)];
    let tint = if tc != 0 { [f32_at(system, tc + 4), f32_at(system, tc + 8), f32_at(system, tc + 0xC)] } else { [1.0; 3] };
    let color = [
        (base[0] * tint[0]).clamp(0.0, 1.0),
        (base[1] * tint[1]).clamp(0.0, 1.0),
        (base[2] * tint[2]).clamp(0.0, 1.0),
        (base[3] * opacity).clamp(0.0, 1.0),
    ];
    let mut vertices = Vec::with_capacity(vertex_count as usize * 24);
    for i in 0..vertex_count {
        let x = f32_at(system, xys + i * 8);
        let y = f32_at(system, xys + i * 8 + 4);
        let u = f32_at(system, uvs + i * 8);
        let v = f32_at(system, uvs + i * 8 + 4);
        for f in [x, y, 0.0, u, v] {
            vertices.extend_from_slice(&f.to_le_bytes());
        }
        vertices.extend_from_slice(&[255, 255, 255, 255]);
    }
    let indices: Vec<u32> = (0..index_count).map(|i| r16(system, indices + i * 2) as u32).filter(|&i| i < vertex_count).collect();
    if indices.len() < 3 {
        return None;
    }
    let mut view_proj = [0f32; 16];
    for (k, m) in view_proj.iter_mut().enumerate() {
        *m = f32_at(system, this + 0x1C + 4 * k as u32);
    }
    let on = |src: u8, dst: u8| Blend { enable: true, eq: [0, 0], func: [src, dst, src, dst] };
    let blend = match composition {
        1 => on(6, 1), // additive: src alpha, one
        2 => on(4, 7), // multiplicative: dst colour, one minus src alpha
        _ => on(6, 7), // normal: src alpha, one minus src alpha
    };
    // the texture modulated by the primary colour (the vertex colour, white,
    // times `color`), every later stage passing it on
    let mut combiner = Combiner::prim_reset();
    combiner.stages[0] = Stage { src_rgb: [3, 0, 0], src_a: [3, 0, 0], comb_rgb: 1, comb_a: 1, ..Stage::default() };
    let (fw, fh) = crate::prim::display_size(disp);
    Some(Draw {
        vertices,
        indices,
        world: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        view_proj,
        tex_matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        color,
        textures: [Some((texture, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: true, linear_min: true })), None, None],
        combiner,
        blend,
        depth_test: false,
        depth_func: 0,
        depth_write: false,
        alpha_test: false,
        alpha_func: 0,
        alpha_ref: 0,
        cull: 0,
        scissor: [0, 0, fw, fh],
        viewport: [0, 0, fw, fh],
        shaded: None,
        post: None,
        depth_offset: 0.0,
    })
}
