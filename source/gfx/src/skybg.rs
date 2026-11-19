//! skybg: cube-map textures (the battles' and the castle's sky, FieldSky).
//!
//! The field's sky is a dome whose texture unit 0 is a **cube map** (PICA
//! unit type 1, GPUREG_TEXUNIT0_PARAM bits 28-30): six square faces, the
//! first at GPUREG_TEXUNIT0_ADDR1 and the other five at ADDR2-ADDR6, read
//! with texture coordinate set 0's (u, v, w) as a direction. The native
//! renderer knew only 2D textures: it sampled the first face (+X: sky over
//! a mountain horizon over rocky ground) with (u, v) as a flat coordinate,
//! so the sky showed the face's rocky lower half, huge and smeared, where
//! the sky and the far mountains belong.
//!
//! Now a cube unit's texture is the six faces stacked into one 2D picture
//! (`CUBE` in the TexRef's format marks it, `cube_key` makes it), and its
//! draws use a build of raster.frag that picks the face and the place on it
//! from the direction as the PICA does (render/shaded.rs, `cube_frag`).
//!
//! Dev: `FATES_SKYBG_TRACE=lo-hi` logs, for each draw of the field's sky
//! dome in those game frames, the field camera's eyes and far planes, the
//! dome's world matrix and the drawn vertices (clip position and texture
//! coordinates). `FATES_SKYBG_DUMP=dir` writes each stacked cube picture
//! made. `FATES_SKYBG_CUBE=0` draws cube units as before (the first face as
//! a 2D texture), for comparisons.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zakuro_core::System;

use crate::prim::{r32, Draw, TexRef};
use crate::render::Decoded;

/// the format bit marking a cube unit's texture: its six faces stacked
/// (+X, -X, +Y, -Y, +Z, -Z, top to bottom), the TexRef's address the first
/// face's. The other wide2/wide-ui marks are 0x2000_0000 and up.
pub const CUBE: u32 = 0x0800_0000;

/// the faces of each cube seen, by its first face's address (virtual)
static FACES: Mutex<Option<HashMap<u32, [u32; 6]>>> = Mutex::new(None);

/// FATES_SKYBG_CUBE=0: cube units drawn as before (their first face, flat)
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_SKYBG_CUBE").map_or(true, |v| v != "0"))
}

/// unit 0's six face addresses (physical, as GPUREG_TEXUNIT0_ADDR1-6 give
/// them), when the unit is a cube map: the first face's whole address, the
/// others' low 22 bits (8-byte units) with the first's high bits.
pub fn unit0_cube_faces(r: &[u32]) -> Option<[u32; 6]> {
    if (r[0x083] >> 28) & 7 != 1 || !enabled() {
        return None;
    }
    let main = r[0x085] & 0x0FFF_FFFF;
    let mut faces = [main << 3; 6];
    for (k, face) in faces.iter_mut().enumerate().skip(1) {
        *face = ((main & !0x003F_FFFF) | (r[0x085 + k] & 0x003F_FFFF)) << 3;
    }
    Some(faces)
}

/// notes a cube's faces (virtual addresses), its first's address the key.
pub fn note_cube(faces: [u32; 6]) {
    let mut map = FACES.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    if map.len() > 256 {
        map.clear();
    }
    map.insert(faces[0], faces);
}

/// dev, FATES_SKYBG_HAZE=1: the free camera's pole haze over a cube-mapped
/// sky dome as before (gfxcastle's, made for the flat-sampled dome)
pub fn sky_haze() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_SKYBG_HAZE").is_ok_and(|v| v == "1"))
}

/// whether a shaded draw reads a cube map (unit 0)
pub fn is_cube(s: &crate::scene::Shaded) -> bool {
    s.textures[0].is_some_and(|(t, _)| t.format & CUBE != 0) || every_draw()
}

/// dev, FATES_SKYBG_EVERY=1: every shaded draw through the cube build of
/// raster.frag, to compare that build with the plain one
fn every_draw() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_SKYBG_EVERY").is_some())
}

impl crate::Gfx {
    /// the stacked picture of a cube unit's texture (`CUBE` in its format):
    /// each face read and decoded as a 2D texture of its own (with a
    /// texture pack's picture when there is one), the six put one under
    /// the other at the largest face's size. None for any other texture.
    pub(crate) fn cube_key(&mut self, system: &mut System, tex: TexRef) -> Option<Option<u64>> {
        if tex.format & CUBE == 0 {
            return None;
        }
        let faces = FACES.lock().unwrap().as_ref().and_then(|m| m.get(&tex.addr).copied());
        let Some(faces) = faces else { return Some(None) };
        let mut parts: Vec<(TexRef, u64)> = Vec::with_capacity(6);
        for addr in faces {
            let face = TexRef { addr, format: tex.format & 0xF, ..tex };
            let Some(key) = self.texture_mip(system, face, 1) else { return Some(None) };
            parts.push((face, key));
        }
        let mut key = 0xC0BE_5ECE_0000_0000u64;
        for (i, (face, k)) in parts.iter().enumerate() {
            key ^= k.rotate_left(9 * i as u32 + 1) ^ (face.addr as u64).rotate_left(23 + 5 * i as u32);
        }
        if !self.decoded.contains_key(&(tex, key)) {
            let pictures: Vec<Arc<Decoded>> = parts.iter().filter_map(|p| self.decoded.get(p).cloned()).collect();
            if pictures.len() != 6 {
                return Some(None);
            }
            let size = pictures.iter().map(|d| d.width.max(d.height)).max().unwrap_or(tex.width).max(1);
            let mut rgba = Vec::with_capacity((size * size * 4 * 6) as usize);
            for d in &pictures {
                face_at(d, size, &mut rgba);
            }
            if let Some(dir) = std::env::var_os("FATES_SKYBG_DUMP") {
                let _ = std::fs::create_dir_all(&dir);
                let path = std::path::Path::new(&dir).join(format!("cube-{:08X}-{size}-{key:016X}.png", tex.addr));
                let _ = crate::write_png(&path, &rgba, size, size * 6);
            }
            self.decoded.insert((tex, key), Arc::new(Decoded { width: size, height: size * 6, rgba, levels: 1 }));
        }
        Some(Some(key))
    }
}

/// a face's largest size, as `size` x `size` (nearest texel when its
/// picture is another size), appended to `out`.
fn face_at(d: &Decoded, size: u32, out: &mut Vec<u8>) {
    let (w, h) = (d.width.max(1), d.height.max(1));
    for y in 0..size {
        let sy = (y * h / size).min(h - 1);
        for x in 0..size {
            let sx = (x * w / size).min(w - 1);
            let o = ((sy * w + sx) * 4) as usize;
            out.extend_from_slice(d.rgba.get(o..o + 4).unwrap_or(&[0, 0, 0, 255]));
        }
    }
}

/// raster.frag's cube build: unit 0 of type 1 reads its face from the
/// stacked picture. The direction is texture coordinate set 0's (u, v, w);
/// the face is its largest component's, the place on it the other two over
/// that one, as the PICA (and OpenGL) put them; t is flipped as
/// sample_unit flips it (the pictures are decoded top row first). Sampled
/// at the largest size, half a texel inside the face so the faces above
/// and below never bleed in.
pub const CUBE_SAMPLE: &str = r#"
vec4 sample_cube(vec3 d) {
    vec3 a = abs(d);
    float face;
    vec2 sc;
    float ma;
    if (a.x >= a.y && a.x >= a.z) {
        ma = a.x;
        face = d.x >= 0.0 ? 0.0 : 1.0;
        sc = d.x >= 0.0 ? vec2(-d.z, -d.y) : vec2(d.z, -d.y);
    } else if (a.y >= a.z) {
        ma = a.y;
        face = d.y >= 0.0 ? 2.0 : 3.0;
        sc = d.y >= 0.0 ? vec2(d.x, d.z) : vec2(d.x, -d.z);
    } else {
        ma = a.z;
        face = d.z >= 0.0 ? 4.0 : 5.0;
        sc = d.z >= 0.0 ? vec2(d.x, -d.y) : vec2(-d.x, -d.y);
    }
    vec2 st = sc / max(ma, 1e-30) * 0.5 + 0.5;
    vec2 size = vec2(textureSize(sampler2D(texture0_image, texture0_sampler), 0));
    vec2 inner = vec2(0.5 / size.x, 3.0 / size.y);
    vec2 at = clamp(vec2(st.x, CUBE_T_FLIP), inner, vec2(1.0) - inner);
    return textureLod(sampler2D(texture0_image, texture0_sampler), vec2(at.x, (face + at.y) / 6.0), 0.0);
}

"#;

/// FieldWorld (its camera at +0x34, its sky at +0x10)
const FIELD_WORLD_PTR: u32 = 0x006B_E404;

fn f(system: &mut System, a: u32) -> f32 {
    f32::from_bits(r32(system, a))
}

fn range() -> Option<(u64, u64)> {
    static R: std::sync::OnceLock<Option<(u64, u64)>> = std::sync::OnceLock::new();
    *R.get_or_init(|| {
        let v = std::env::var("FATES_SKYBG_TRACE").ok()?;
        let mut it = v.split('-').filter_map(|v| v.trim().parse::<u64>().ok());
        Some((it.next().unwrap_or(0), it.next().unwrap_or(u64::MAX)))
    })
}

pub(crate) fn trace(system: &mut System, model: u32, state: u32, mesh: u32, made: &[(Draw, [Option<u64>; 3])]) {
    let Some((lo, hi)) = range() else { return };
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    if game < lo || game > hi {
        return;
    }
    let world = r32(system, zakuro_core::region::us(FIELD_WORLD_PTR));
    let sky_actor = if world != 0 { r32(system, world + 0x10) } else { 0 };
    if sky_actor == 0 || sky_actor.wrapping_add(0x80) != model {
        return;
    }
    let cam = r32(system, state + 8);
    let field_cam = r32(system, world + 0x34);
    let v3 = |system: &mut System, a: u32| -> [f32; 3] { [f(system, a), f(system, a + 4), f(system, a + 8)] };
    let eye4 = v3(system, cam + 4);
    let tgt = v3(system, cam + 16);
    let bind = r32(system, cam + 0x5C8);
    let eye618 = v3(system, cam + 0x618);
    let tgt618 = v3(system, cam + 0x624);
    let far34 = f(system, cam + 0x34);
    let far648 = f(system, cam + 0x648);
    let m: [f32; 12] = std::array::from_fn(|i| f(system, cam + 0x98 + 4 * i as u32));
    let t = [m[3], m[7], m[11]];
    let view_eye = [
        -(m[0] * t[0] + m[4] * t[1] + m[8] * t[2]),
        -(m[1] * t[0] + m[5] * t[1] + m[9] * t[2]),
        -(m[2] * t[0] + m[6] * t[1] + m[10] * t[2]),
    ];
    let dome: [f32; 12] = std::array::from_fn(|i| f(system, sky_actor + 0x50 + 4 * i as u32));
    let bone = crate::h3d::mesh(system, mesh).and_then(|m| m.submeshes.first().and_then(|s| s.bones.first().copied())).unwrap_or(0);
    let matrices = r32(system, model + 0x24 + 0x14);
    let bm: [f32; 12] = std::array::from_fn(|i| f(system, matrices + bone as u32 * 0x30 + 4 * i as u32));
    eprintln!(
        "skybg g{game} cam {cam:08X} field {field_cam:08X} bind {bind} eye4 {eye4:?} tgt4 {tgt:?} eye618 {eye618:?} tgt618 {tgt618:?} view_eye {view_eye:?} far34 {far34} far648 {far648}"
    );
    eprintln!("skybg   dome {dome:?}");
    eprintln!("skybg   bone {bone} {bm:?}");
    if game % 50 > 1 {
        return;
    }
    for (d, _) in made {
        let Some(s) = d.shaded.as_deref() else { continue };
        let Some(pts) = crate::wide2::dev_vertices(s) else { continue };
        eprintln!("skybg   draw n{} verts {} key {:?} sem {:?}", s.indices.len(), pts.len(), s.key, &s.shading[404..428]);
        let t: Vec<String> = (0..3).filter_map(|u| s.textures[u].map(|(t, smp)| format!("u{u} {:08X} {}x{} fmt {} wrap {}/{} mip {:?}", t.addr, t.width, t.height, t.format, smp.wrap, smp.wrap_t, s.mips[u]))).collect();
        eprintln!("skybg   textures {} draw[48..52] {:08X?}", t.join(" | "), &s.draw[48..64.min(s.draw.len())]);
        for (inp, out) in pts {
            eprintln!("skybg     in {:?} out {:?}", inp, out);
        }
    }
}

/// the sampler bindings naga is given (it does not let two variables share
/// a binding), put back on the image's after: a combined image sampler
/// descriptor may be read as an image and a sampler at its binding.
const SAMPLER_BINDING: u32 = 16;

/// raster.frag built again for draws with a cube unit: everything read from
/// the draw's registers (its generic build, so no specialization
/// constants), unit 0 of type 1 sampled from the stacked faces. Built with
/// naga, which wants separate images and samplers and does not know
/// `precise` (a cube draw is a sky; the combiners' last bit there is not
/// worth a compiler). `depth`: the WRITES_DEPTH build.
pub fn cube_frag(depth: bool) -> Result<Vec<u32>, String> {
    let mut s = zakuro_gpu::native3d::RASTER_FRAG_SOURCE.replace('\r', "").replace("precise ", "");
    for i in 0..3 {
        let (old, new) = (
            format!("layout(set = 0, binding = {i}) uniform sampler2D texture{i};"),
            format!("layout(set = 0, binding = {i}) uniform texture2D texture{i}_image;\nlayout(set = 0, binding = {}) uniform sampler texture{i}_sampler;", SAMPLER_BINDING + i),
        );
        if !s.contains(&old) {
            return Err(format!("raster.frag: no texture{i}"));
        }
        s = s.replace(&old, &new).replace(&format!("texture(texture{i}, "), &format!("texture(sampler2D(texture{i}_image, texture{i}_sampler), "));
    }
    let lines: Vec<String> = s
        .lines()
        .map(|l| match l.strip_prefix("layout(constant_id = ").and_then(|r| r.split_once(") ")) {
            Some((_, rest)) if rest.starts_with("const uint DYNAMIC") => "const uint DYNAMIC = 1u;".to_string(),
            Some((_, rest)) => rest.to_string(),
            None => l.to_string(),
        })
        .collect();
    let mut s = lines.join("\n");
    let flip = if std::env::var("FATES_SKYBG_TFLIP").is_ok_and(|v| v == "0") { "st.y" } else { "1.0 - st.y" };
    let sample = format!("{}\nvoid main() {{", CUBE_SAMPLE.replace("CUBE_T_FLIP", flip));
    let unit0 = "textures[0] = sample_unit(0u, projected ? in_texcoords01.xy / in_view.w : in_texcoords01.xy);";
    for (old, new) in [
        ("if (LIGHTING != 0u)", "if (flags.y != 0u)".to_string()),
        ("(TEXTURE_CONFIG & 0x400u)", "(misc.w & 0x400u)".to_string()),
        ("\nvoid main() {", format!("\n{sample}")),
        (unit0, "textures[0] = ((units[0].x >> 28) & 7u) == 1u ? sample_cube(vec3(in_texcoords01.xy, in_view.w)) : sample_unit(0u, projected ? in_texcoords01.xy / in_view.w : in_texcoords01.xy);".to_string()),
    ] {
        if !s.contains(old) {
            return Err(format!("raster.frag: no {old:?}"));
        }
        s = s.replacen(old, &new, 1);
    }
    let mut options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    if depth {
        options.defines.insert("WRITES_DEPTH".into(), "1".into());
    }
    let module = naga::front::glsl::Frontend::default().parse(&options, &s).map_err(|e| format!("glsl: {}", e.emit_to_string(&s)))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("validate: {e:?}"))?;
    let options = naga::back::spv::Options { lang_version: (1, 0), ..Default::default() };
    let pipeline = naga::back::spv::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: "main".into() };
    let mut words = naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|e| format!("spv: {e:?}"))?;
    // OpDecorate <var> Binding 16+i -> i
    let (mut at, mut moved) = (5usize, 0);
    while at < words.len() {
        let (count, op) = ((words[at] >> 16) as usize, words[at] & 0xFFFF);
        if count == 0 {
            break;
        }
        if op == 71 && count == 4 && words[at + 2] == 33 && (SAMPLER_BINDING..SAMPLER_BINDING + 3).contains(&words[at + 3]) {
            words[at + 3] -= SAMPLER_BINDING;
            moved += 1;
        }
        at += count;
    }
    if moved != 3 {
        return Err(format!("raster.frag cube build: {moved} sampler bindings"));
    }
    Ok(words)
}

/// dev, FATES_SKYBG_TRACE=lo-hi: every shaded draw of the frames in that
/// range (game frames): its indices, textures, unit 0's type and key.
pub(crate) fn trace_frame(disp: u32, draws: &[Draw]) {
    let Some((lo, hi)) = range() else { return };
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    if game < lo || game > hi {
        return;
    }
    for (i, d) in draws.iter().enumerate() {
        let Some(s) = d.shaded.as_deref() else { continue };
        let t: Vec<String> = (0..3).filter_map(|u| s.textures[u].map(|(t, smp)| format!("u{u} {:08X} {}x{} f{:X} w{}/{}", t.addr, t.width, t.height, t.format, smp.wrap, smp.wrap_t))).collect();
        let unit0 = s.draw.get(52).copied().unwrap_or(0);
        eprintln!("skybg frame g{game} d{disp} {i}: n{} type0 {} tex [{}] key {:?} tev0 {:08X?}", s.indices.len(), (unit0 >> 28) & 7, t.join(", "), s.key, &s.draw[0..5]);
    }
}
