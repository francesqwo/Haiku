//! wide-ui: what fills the ultrawide sides of the screens whose content is
//! 5:3 by nature: menus, the title, talk scenes over still pictures, story
//! stills, telop cards.
//!
//! The game world (battle maps, 3D battles, My Castle, 3D cutscenes) is
//! drawn truly wide by the widened cameras and is left alone. A screen is
//! 5:3 by nature when its backdrop is a full-screen layer made for the
//! 3DS's 400x240: wide2 already finds those (a tall 2D layer spanning the
//! 400, a camera-facing 3D plane covering the view) and extends their edges
//! into the sides. Here, with `pc.wide_sides` at Artwork (the default) or
//! Black, such a frame instead gets the 5:3 framed: after everything is
//! drawn, the two sides are covered by side panels made from the game's own
//! ornamental wallpaper art (or black), and only what covers the whole wide
//! screen after the backdrop (fades, dims, the telops' washes) is drawn
//! again over the panels, so a fade still covers everything.
//!
//! The panel art is the dark scrollwork band of the game's own top-screen
//! wallpaper (`Wallpaper2`, the frame around the menus): its left border,
//! 100 texels by the screen's 240, at its native pixel scale (one texel per
//! 3DS pixel), with the frame's own corners at the top and bottom. Its inner
//! side meets the 5:3 picture where the wallpaper's own inner panel began,
//! so the picture sits in the frame as the menus sit in the wallpaper.
//! Sides wider than the band (32:9) repeat it, mirrored, never stretched.
//! It is shaded toward the picture, as the wallpaper's own frame is.

use std::collections::HashMap;
use std::sync::Arc;

use crate::prim::{Blend, Combiner, Draw, Sampling, TexRef};
use crate::render::Decoded;
use crate::wide2::{self, V, LONG};

/// what fills the sides, pc.wide_sides (FATES_WIDE_SIDES=artwork|extend|
/// black overrides it, for headless tests).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sides {
    Artwork,
    Extend,
    Black,
}

pub fn sides() -> Sides {
    static ENV: std::sync::OnceLock<Option<Sides>> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| match std::env::var("FATES_WIDE_SIDES").ok()?.to_ascii_lowercase().as_str() {
        "artwork" | "art" | "0" => Some(Sides::Artwork),
        "extend" | "1" => Some(Sides::Extend),
        "black" | "2" => Some(Sides::Black),
        _ => None,
    });
    env.unwrap_or(match zakuro_gpu::wide::sides() {
        1 => Sides::Extend,
        2 => Sides::Black,
        _ => Sides::Artwork,
    })
}

fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_WIDE_UI_LOG").is_some())
}

/// the panel's texture, as draws name it: an address no guest texture has.
const PANEL_ADDR: u32 = 0xFFFF_F000;
const PANEL_SIZE: u32 = 256;
/// format marks: ours, and which panel
const PANEL_FORMAT: u32 = 0x5755_0000;
const BLACK: u32 = 1;

fn panel_ref(black: bool) -> TexRef {
    TexRef { addr: PANEL_ADDR, width: PANEL_SIZE, height: PANEL_SIZE, format: PANEL_FORMAT | black as u32 * BLACK }
}

/// the key of a panel texture (made when missing), None for any other.
pub fn texture_key(decoded: &mut HashMap<(TexRef, u64), Arc<Decoded>>, tex: TexRef) -> Option<u64> {
    is_panel(tex).then(|| panel_texture(decoded, tex.format & BLACK != 0).1)
}

fn is_panel(tex: TexRef) -> bool {
    tex.addr == PANEL_ADDR && tex.format & 0xFFFF_0000 == PANEL_FORMAT
}

/// the panel textures' keys in the renderer's decoded textures.
const PANEL_KEY: u64 = 0x5769_6465_5549_5061; // "WideUIPa"

/// the panel's texture under its key, made (or made again, after the
/// decoded textures were cleared) when missing.
fn panel_texture(decoded: &mut HashMap<(TexRef, u64), Arc<Decoded>>, black: bool) -> (TexRef, u64) {
    let tex = panel_ref(black);
    let key = PANEL_KEY ^ black as u64;
    if !decoded.contains_key(&(tex, key)) {
        decoded.insert((tex, key), Arc::new(if black { black_panel() } else { artwork_panel() }));
    }
    (tex, key)
}

fn black_panel() -> Decoded {
    let n = (PANEL_SIZE * PANEL_SIZE) as usize;
    let mut rgba = vec![0u8; n * 4];
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Decoded { width: PANEL_SIZE, height: PANEL_SIZE, rgba, levels: 1 }
}

/// where the game's art is: <FATES_ROOT or the executable's folder>/assets,
/// as the options menu's kit finds it.
fn assets_dir() -> std::path::PathBuf {
    let root = std::env::var_os("FATES_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_owned())));
    let dir = root.unwrap_or_default().join("assets");
    if dir.join("ui").is_dir() {
        dir
    } else {
        std::path::PathBuf::from("assets")
    }
}

fn load_png(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf[..(w * h * 4) as usize].to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf[..(w * h * 3) as usize].chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return None,
    };
    Some((w, h, rgba))
}

/// the wallpaper's band: its left border, from the screen's left edge to
/// where its inner panel begins.
const BAND: usize = 100;
/// how dark the panel is next to the picture, and how far that reaches
const NEAR: f32 = 0.5;
const REACH: f32 = 36.0;
/// the band's tone: the wallpaper's own colours, a little darker, so the
/// sides stay quieter than the picture
const BRIGHT: f32 = 0.72;
/// how much darker the far edge of a wide (32:9) side is
const FAR: f32 = 0.4;

/// the side panel: column 0 at the picture's edge, outward from there;
/// rows from the screen's top. Each side is drawn from it, the left one
/// mirrored, so both meet the picture with the band's inner side.
fn artwork_panel() -> Decoded {
    let (w, h) = (PANEL_SIZE as usize, PANEL_SIZE as usize);
    let mut rgba = vec![0u8; w * h * 4];
    let path = assets_dir().join("ui").join("Wallpaper2_").join("Wallpaper2.png");
    let Some((sw, sh, src)) = load_png(&path).filter(|(sw, sh, _)| *sw as usize >= BAND && *sh >= 240) else {
        log::warn!("wide-ui: {} not found, the sides are black", path.display());
        return black_panel();
    };
    let _ = sh;
    let tone = tone();
    for y in 0..h {
        let sy = y.min(239);
        for c in 0..w {
            // mirrored repeats of the band, its inner side first
            let m = c % (2 * BAND);
            let sx = if m < BAND { BAND - 1 - m } else { m - BAND };
            let o = (sy * sw as usize + sx) * 4;
            // shading toward the picture
            let t = (c as f32 / REACH).min(1.0);
            let t = t * t * (3.0 - 2.0 * t);
            // and quieter again toward the screen's far edges where the
            // sides are wide (32:9), as the game's wallpapers darken toward
            // theirs
            let far = ((c as f32 - BAND as f32) / (PANEL_SIZE as f32 - 26.0 - BAND as f32)).clamp(0.0, 1.0);
            let far = far * far * (3.0 - 2.0 * far);
            let f = (NEAR + (1.0 - NEAR) * t) * (1.0 - FAR * far) * tone.0;
            let d = (y * w + c) * 4;
            let luma = 0.299 * src[o] as f32 + 0.587 * src[o + 1] as f32 + 0.114 * src[o + 2] as f32;
            for k in 0..3 {
                let v = luma + (src[o + k] as f32 - luma) * tone.1;
                rgba[d + k] = (v * f * tone.2[k]).round().clamp(0.0, 255.0) as u8;
            }
            rgba[d + 3] = 255;
        }
    }
    Decoded { width: PANEL_SIZE, height: PANEL_SIZE, rgba, levels: 1 }
}

/// brightness, saturation and tint; FATES_WIDE_UI_TONE=b,s,r,g,b for
/// trying others.
fn tone() -> (f32, f32, [f32; 3]) {
    let def = (BRIGHT, 1.0, [1.0, 1.0, 1.0]);
    let Ok(v) = std::env::var("FATES_WIDE_UI_TONE") else { return def };
    let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    match n.as_slice() {
        [b, s, r, g, bl] => (*b, *s, [*r, *g, *bl]),
        [b, s] => (*b, *s, [1.0; 3]),
        [b] => (*b, 1.0, [1.0; 3]),
        _ => def,
    }
}

/// a texture's share of opaque texels (its largest size), cached by key.
fn opacity(decoded: &HashMap<(TexRef, u64), Arc<Decoded>>, tex: TexRef, key: Option<u64>) -> f32 {
    static SEEN: std::sync::Mutex<Option<HashMap<u64, f32>>> = std::sync::Mutex::new(None);
    let Some(key) = key else { return 0.0 };
    let mut seen = SEEN.lock().unwrap();
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some(&o) = seen.get(&key) {
        return o;
    }
    let Some(d) = decoded.get(&(tex, key)) else { return 0.0 };
    let n = (d.width * d.height) as usize;
    if n == 0 || d.rgba.len() < n * 4 {
        return 0.0;
    }
    let opaque = d.rgba[..n * 4].chunks_exact(4).filter(|p| p[3] >= 240).count();
    let o = opaque as f32 / n as f32;
    if seen.len() > 4096 {
        seen.clear();
    }
    seen.insert(key, o);
    o
}

/// how much of a backdrop's texture has to be opaque for it to be the
/// picture of a 5:3 screen (a wallpaper, a still, a talk room, a title
/// card), not a wash or a shade laid over the world.
const OPAQUE: f32 = 0.9;

/// what a frame's backdrop is, when it is one: its place in the draws and
/// the colour (and fade) the panel takes from it.
struct Backdrop {
    at: usize,
    color: [u8; 4],
    /// a 3D plane's (wide2 cut it where its sides start)
    plane: bool,
}

/// the frame's last backdrop.
fn backdrop(draws: &[Draw], keys: &[[Option<u64>; 3]], decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> Option<Backdrop> {
    let mut found = None;
    for (n, d) in draws.iter().enumerate() {
        if d.post.is_some() {
            continue;
        }
        if let Some(s) = d.shaded.as_deref() {
            // a 3D plane wide2 extended: a talk's room, a story picture,
            // a telop's card. Blended ones (a phase banner's shade, the
            // Dragon Vein fading in over the map) stay extended.
            if !wide2::is_side(d) {
                continue;
            }
            let Some((tex, _)) = s.textures[0] else { continue };
            let o = opacity(decoded, tex, keys[n][0]);
            // blending that only replaces (source one, destination zero,
            // as a telop's title card has) is none
            let blended = s.key.blend.is_some_and(|f| !((f >> 16) & 0xF == 1 && (f >> 20) & 0xF == 0));
            if log_on() {
                eprintln!(
                    "wide-ui frame {} 3D side {n}: tex {:08X} {}x{} f{} opaque {o:.2} blend {:?}",
                    zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
                    tex.addr,
                    tex.width,
                    tex.height,
                    tex.format,
                    s.key.blend
                );
            }
            if o >= OPAQUE && !blended {
                found = Some(Backdrop { at: n, color: [255; 4], plane: true });
            }
            continue;
        }
        // a 2D full-screen layer's side (wide2 marks its softened texture
        // as tall): a wallpaper, a still, a shop's room
        let Some((tex, _)) = d.textures[0] else { continue };
        if tex.format & wide2::SOFT_T == 0 || is_panel(tex) {
            continue;
        }
        let o = opacity(decoded, tex, keys[n][0]);
        let c = average_color(d);
        if log_on() {
            eprintln!(
                "wide-ui frame {} 2D side {n}: tex {:08X} {}x{} f{:X} opaque {o:.2} colour {c:?}",
                zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
                tex.addr,
                tex.width,
                tex.height,
                tex.format,
            );
        }
        if o >= OPAQUE {
            found = Some(Backdrop { at: n, color: c, plane: false });
        }
    }
    found
}

/// a 3D draw again for each side it reaches into past the view's edge,
/// scissored to that side.
fn past_edge(d: &Draw, edge: f32) -> Vec<Draw> {
    let Some(s) = d.shaded.as_deref() else { return Vec::new() };
    let Some(points) = wide2::projected(s) else { return Vec::new() };
    let lo = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let hi = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let [_, vy, _, vh] = s.viewport;
    let row = |e: f32| vy as f32 + (e + 1.0) * 0.5 * vh as f32;
    let [sx, sy, sw, sh] = s.scissor;
    let mut out = Vec::new();
    for (reaches, a, b) in [(hi > edge, (row(edge).ceil() as i32).max(sy), sy + sh), (lo < -edge, sy, (row(-edge).floor() as i32).min(sy + sh))] {
        if !reaches || b <= a {
            continue;
        }
        let mut t = s.clone();
        t.scissor = [sx, a, sw, b - a];
        let mut c = d.clone();
        c.shaded = Some(Arc::new(t));
        out.push(c);
    }
    out
}

/// a framed 3D backdrop drawn whole again: wide2 cut the plane 12 units
/// inside the view's edge, where its widened outer strip (its side) takes
/// over, and that strip inside the 400 is not the game's picture. The plane
/// gets back the scissor its pieces shared (the panels cover the rest).
fn whole_plane(draws: &mut [Draw], side: usize) {
    let Some(main) = (0..side).rev().find(|&i| draws[i].shaded.is_some() && !wide2::is_side(&draws[i])) else { return };
    let mut sc = match draws[main].shaded.as_deref() {
        Some(s) => s.scissor,
        None => return,
    };
    for d in draws[main + 1..].iter_mut().take_while(|d| wide2::is_side(d)) {
        let side = d.shaded.as_deref().unwrap();
        let o = side.scissor;
        let (x0, y0) = (sc[0].min(o[0]), sc[1].min(o[1]));
        let (x1, y1) = ((sc[0] + sc[2]).max(o[0] + o[2]), (sc[1] + sc[3]).max(o[1] + o[3]));
        sc = [x0, y0, x1 - x0, y1 - y0];
        // and the side starts past the view's edge, under the panel
        let e = f32::from_bits(side.shading[wide2::PAD + 1]);
        let [_, vy, _, vh] = side.viewport;
        let row = vy as f32 + (e + 1.0) * 0.5 * vh as f32;
        let (mut a, mut b) = (o[1], o[1] + o[3]);
        if e > 0.0 {
            a = a.max(row.ceil() as i32);
        } else {
            b = b.min(row.floor() as i32);
        }
        let mut t = side.clone();
        t.scissor = [o[0], a, o[2], (b - a).max(0)];
        d.shaded = Some(Arc::new(t));
    }
    if let Some(s) = draws[main].shaded.as_deref() {
        let mut s = s.clone();
        s.scissor = sc;
        draws[main].shaded = Some(Arc::new(s));
    }
}

/// a 2D draw's vertex colour (averaged) times its colour uniform.
fn average_color(d: &Draw) -> [u8; 4] {
    let mut sum = [0f32; 4];
    let mut n = 0f32;
    for v in d.vertices.chunks_exact(24) {
        for k in 0..4 {
            sum[k] += v[20 + k] as f32;
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [255; 4];
    }
    std::array::from_fn(|k| (sum[k] / n * d.color[k].clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8)
}

/// the frame's draws complete (wide2's extensions in), their textures
/// decoded: a 5:3 screen gets its side panels, drawn last, with the
/// full-screen layers after its backdrop drawn again over them.
pub fn finish(draws: &mut Vec<Draw>, keys: &mut Vec<[Option<u64>; 3]>, decoded: &mut HashMap<(TexRef, u64), Arc<Decoded>>, k: f32) {
    if k <= 1.001 || draws.len() != keys.len() {
        return;
    }
    let mode = sides();
    if mode == Sides::Extend {
        return;
    }
    // wideart: a translucent 5:3 picture laid over the wide world (the
    // Dragon Vein) gets the panels through its own sides
    telop_panels(draws, keys, decoded, mode == Sides::Black, 1.0 / k);
    if survey_on() {
        survey(draws, keys, decoded);
    }
    let Some(back) = backdrop(draws, keys, decoded) else { return };
    if back.plane {
        whole_plane(draws, back.at);
    }
    let edge = 1.0 / k;
    let black = mode == Sides::Black;
    let (tex, key) = panel_texture(decoded, black);
    let panel = panel_draw(tex, edge, k, back.color);
    let mut extra = vec![(panel, [Some(key), None, None])];
    // what covers the whole screen after the backdrop: again, over the
    // panels, cut to them
    for n in back.at + 1..draws.len() {
        let d = &draws[n];
        if d.post.is_some() {
            continue;
        }
        if d.shaded.is_some() {
            // a full-screen 3D layer's sides when it is a wash (a telop's
            // shade or fade: untextured, or a small texture of tones); a
            // larger picture over the backdrop (the conversation log's
            // second picture) is part of the 5:3 picture, not a veil
            let wash = d.shaded.as_deref().is_some_and(|s| s.textures[0].map_or(true, |(t, _)| t.width.max(t.height) <= WASH));
            if wide2::is_side(d) && wash {
                extra.push((d.clone(), keys[n]));
            }
            // after a 3D backdrop (a telop's card), the telop's own layers
            // that reach past the 400 (the white wipe and its petals that
            // end a chapter card) go on over the panels, cut to the sides
            if back.plane && !wide2::is_side(d) {
                for cut in past_edge(d, edge) {
                    extra.push((cut, keys[n]));
                }
            }
            continue;
        }
        if let Some(cut) = sides_of(d, edge) {
            extra.push((cut, keys[n]));
        }
    }
    if log_on() {
        eprintln!(
            "wide-ui frame {}: backdrop at {} of {}, panel {:?}, {} drawn again over it",
            zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
            back.at,
            draws.len(),
            mode,
            extra.len() - 1
        );
    }
    for (d, k) in extra {
        draws.push(d);
        keys.push(k);
    }
}

/// the largest texture a 3D wash drawn again over the panels has.
const WASH: u32 = 128;

// ---- wideart: the Dragon Vein's sides ----
//
// A Dragon Vein (Tlp_DragonGround/Fire/Water/Wind/...) lays a 5:3
// wallpaper picture (512x256, its 400x240 drawn) over the battle map,
// fading in and out through its material's alpha, which the CPU does not
// evaluate. wide2 widened the plane's outer strip across the sides, a
// blurred continuation of its vignette. Here such a plane's sides keep
// the plane's own program, material, blending and animation, and only
// what they sample changes: the side panel's art, at its native scale,
// through texture coordinates shade.vert works out from where each vertex
// lands (`SHADE_VERT_PANEL`), the same mapping `panel_draw` gives the
// framed 2D screens. So the panels fade in and out with the picture, and
// the plane itself is drawn whole to the 400's edge, as on the 3DS.

/// shade.vert's Shading block: the output semantics' spare slot 22, which
/// freecam.rs uses for its haze modes 1 and 2 (low byte); this marker's
/// low byte is 3.
const PANEL_MODE_SLOT: usize = 404 + 22;
const PANEL_MODE: u32 = 0x5741_0003;

/// how opaque a translucent side's picture must be to be a 5:3 picture
/// (the Dragon Vein's wallpaper is opaque where it is drawn; a shade or a
/// glow is not)
const TELOP_OPAQUE: f32 = 0.5;

/// the output semantics' other spare slot (freecam.rs's haze parameter):
/// for a panel side, the picture's edge, 1/k, the screen's own (the
/// plane's end, wide2's pad[1], may miss it by a unit of the 400).
const PANEL_EDGE_SLOT: usize = 404 + 23;

/// shade.vert's last position line with a panel side's start before it
/// (render/shaded.rs patches it in after wide2's widening): every vertex
/// short of the picture's edge is moved onto it, so the side starts on the
/// edge to the sub-pixel, wherever the scissor's whole rows fall, and the
/// plane's own last columns past the edge (its border, which the 3DS
/// never showed) go under the panel.
pub const SHADE_VERT_PANEL_START: &str = "if (semantics[22] == 0x57410003u && pad[0] != 0u) {
        float s = sign(uintBitsToFloat(pad[1]));
        float edge = uintBitsToFloat(semantics[23]) * position.w;
        position.y = s * max(s * position.y, edge);
    }
    gl_Position = vec4(position.xy, -position.z, position.w);";

/// shade.vert's texture coordinate lines with a panel side's coordinates
/// after them (render/shaded.rs patches it in): u from the picture's edge
/// outward, one texel of the panel per 3DS pixel (200 of them from the
/// middle to the picture's edge E), v from the screen's top (x = +1 before
/// the upright turn), in the PICA's convention (v = 0 at the bottom row,
/// which raster.frag turns over). A half texel in from the picture's edge,
/// so no fragment there samples left of the panel.
pub const SHADE_VERT_PANEL: &str = "out_texcoord2 = vec2(semantic(12u, 0.0), semantic(13u, 0.0));
    if (semantics[22] == 0x57410003u && pad[0] != 0u && position.w > 1e-6) {
        float s = sign(uintBitsToFloat(pad[1]));
        float edge = uintBitsToFloat(semantics[23]);
        float u = (s * position.y / position.w - edge) * (200.0 / (256.0 * edge)) + 0.5 / 256.0;
        float v = (1.0 - position.x / position.w) * 0.5 * (240.0 / 256.0);
        out_texcoords01.xy = vec2(u, 1.0 - v);
    }";

/// wideart: whether a draw is the side wide2 made for a translucent 5:3
/// picture ending on the view's edge (a Dragon Vein's wallpaper).
fn telop_side(d: &Draw, key: Option<u64>, decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> bool {
    if !wide2::is_side(d) {
        return false;
    }
    let Some(s) = d.shaded.as_deref() else { return false };
    // a plane ending on the view's edge: its outer strip widened (negative)
    let widen = s.shading.get(wide2::PAD).map_or(0.0, |&w| f32::from_bits(w));
    if !(widen < 0.0) || s.shading.len() <= PANEL_EDGE_SLOT {
        return false;
    }
    let Some((tex, _)) = s.textures[0] else { return false };
    if is_panel(tex) || tex.width.max(tex.height) <= WASH {
        return false;
    }
    // blended (a picture that replaces is a backdrop, framed by `finish`)
    let blended = s.key.blend.is_some_and(|f| !((f >> 16) & 0xF == 1 && (f >> 20) & 0xF == 0));
    blended && opacity(decoded, tex, key) >= TELOP_OPAQUE
}

/// wideart: each translucent 5:3 picture's sides sample the panel.
fn telop_panels(draws: &mut [Draw], keys: &mut [[Option<u64>; 3]], decoded: &mut HashMap<(TexRef, u64), Arc<Decoded>>, black: bool, edge: f32) {
    let mut n = 0;
    while n < draws.len() {
        if !telop_side(&draws[n], keys[n][0], decoded) {
            n += 1;
            continue;
        }
        // the plane whole again to the 400's edge, its sides past it
        whole_plane(draws, n);
        let (tex, key) = panel_texture(decoded, black);
        while n < draws.len() && wide2::is_side(&draws[n]) {
            if let Some(s) = draws[n].shaded.as_deref() {
                let mut s = s.clone();
                s.textures[0] = Some((tex, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: true, linear_min: true }));
                s.mips[0] = crate::scene::Mip { levels: 1, min: 0, max: 0, bias: 0, linear: true, between: false };
                s.shading[PANEL_MODE_SLOT] = PANEL_MODE;
                s.shading[PANEL_EDGE_SLOT] = edge.to_bits();
                // the scissor from the whole row the edge falls in: the
                // vertices start the side on the edge itself
                let high = f32::from_bits(s.shading[wide2::PAD + 1]) > 0.0;
                let [_, vy, _, vh] = s.viewport;
                let row = |e: f32| vy as f32 + (e + 1.0) * 0.5 * vh as f32;
                let [x, y0, w, h] = s.scissor;
                let (mut a, mut b) = (y0, y0 + h);
                if high {
                    a = row(edge).floor() as i32;
                } else {
                    b = row(-edge).ceil() as i32;
                }
                s.scissor = [x, a, w, (b - a).max(0)];
                draws[n].shaded = Some(Arc::new(s));
                keys[n][0] = Some(key);
            }
            if log_on() {
                eprintln!("wideart frame {}: telop side {n} samples the panel", zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed));
            }
            n += 1;
        }
    }
}

fn survey_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_WIDEART_SURVEY").is_some())
}

/// wideart (dev, FATES_WIDEART_SURVEY=1): a frame that still has extended
/// sides (wide2's widened strips of a full-screen picture) and no framed
/// backdrop, once per distinct set of textures.
fn survey(draws: &[Draw], keys: &[[Option<u64>; 3]], decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    if backdrop(draws, keys, decoded).is_some() {
        return;
    }
    let mut found = Vec::new();
    for (n, d) in draws.iter().enumerate() {
        if d.post.is_some() {
            continue;
        }
        if let Some(s) = d.shaded.as_deref() {
            if !wide2::is_side(d) {
                continue;
            }
            let Some((tex, _)) = s.textures[0] else {
                found.push("3D untextured".to_string());
                continue;
            };
            let widen = f32::from_bits(s.shading[wide2::PAD]);
            let panel = is_panel(tex);
            found.push(format!(
                "3D {:08X} {}x{} {} opaque {:.2} blend {:?}{}",
                tex.addr,
                tex.width,
                tex.height,
                if widen < 0.0 { "over" } else { "spread" },
                opacity(decoded, tex, keys[n][0]),
                s.key.blend,
                if panel { " PANEL" } else { "" }
            ));
            continue;
        }
        let Some((tex, _)) = d.textures[0] else { continue };
        if tex.format & (wide2::SOFT_T | wide2::SOFT_S) == 0 {
            continue;
        }
        found.push(format!(
            "2D {} {:08X} {}x{} opaque {:.2}",
            if tex.format & wide2::SOFT_T != 0 { "tall" } else { "band" },
            tex.addr,
            tex.width,
            tex.height,
            opacity(decoded, tex, keys[n][0])
        ));
    }
    if found.is_empty() {
        return;
    }
    let line = found.join(" | ");
    let mut seen = SEEN.lock().unwrap();
    if seen.contains(&line) {
        return;
    }
    seen.push(line.clone());
    eprintln!("wideart survey frame {}: unframed: {line}", zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed));
}

/// the panels: one quad a side, from the picture's edge to the screen's,
/// in clip space (world and view-projection the identity).
fn panel_draw(tex: TexRef, edge: f32, k: f32, color: [u8; 4]) -> Draw {
    let mut vertices = Vec::with_capacity(8 * 24);
    let mut indices = Vec::with_capacity(12);
    // texels of the panel per unit of clip space along the long axis: one
    // per 3DS pixel, 200·k of them from the middle to an edge
    let per = 200.0 * k / PANEL_SIZE as f32;
    let rows = 240.0 / PANEL_SIZE as f32;
    for (q, sign) in [-1.0f32, 1.0].into_iter().enumerate() {
        let inner = sign * edge;
        let outer = sign * 1.02;
        let u_out = (1.02 - edge) * per;
        // short axis: +1 is the screen's top (see the check in the report)
        for (long, u) in [(inner, 0.0), (outer, u_out)] {
            for (short, v) in [(1.0f32, 0.0f32), (-1.0, rows)] {
                let mut p = [0f32; 3];
                p[LONG] = long;
                p[1 - LONG] = short;
                p[2] = -0.5;
                wide2::write_v(&V { p, uv: [u, v], c: color }, &mut vertices);
            }
        }
        let b = 4 * q as u32;
        indices.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
    }
    let mut combiner = Combiner::prim_reset();
    // vertex colour times the panel
    combiner.stages[0].comb_rgb = 1;
    combiner.stages[0].comb_a = 1;
    let identity3x4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let identity4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    // the whole upper display
    let (viewport, scissor) = ([0, 0, 240, 400], [0, 0, 240, 400]);
    Draw {
        vertices,
        indices,
        world: identity3x4,
        view_proj: identity4,
        tex_matrix: identity3x4,
        color: [1.0; 4],
        textures: [Some((tex, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: true, linear_min: true })), None, None],
        combiner,
        blend: Blend::prim(0),
        depth_test: false,
        depth_func: 0,
        depth_write: false,
        alpha_test: false,
        alpha_func: 0,
        alpha_ref: 0,
        cull: 0,
        scissor,
        viewport,
        shaded: None,
        post: None,
        depth_offset: 0.0,
    }
}

/// a 2D draw covering the whole wide screen along its long axis (a fade, a
/// dim, a full-screen fill), cut to the two sides; None for anything else.
fn sides_of(d: &Draw, edge: f32) -> Option<Draw> {
    let n = d.vertices.len() / 24;
    if n == 0 || d.indices.len() < 3 || d.indices.len() % 3 != 0 {
        return None;
    }
    let vs: Vec<V> = (0..n).map(|i| wide2::read_v(&d.vertices[i * 24..i * 24 + 24])).collect();
    let long: Vec<f32> = vs.iter().map(|v| wide2::clip(d, v.p)).map(|c| if c[3].abs() > 1e-6 { c[LONG] / c[3] } else { 0.0 }).collect();
    // one of its triangles reaches across the whole screen, as a
    // stretched fade's does (a band's two side pieces, drawn together, do
    // not), and it is not one of wide2's extensions
    if d.textures[0].is_some_and(|(t, _)| t.format & (wide2::SOFT_T | wide2::SOFT_S) != 0) {
        return None;
    }
    let across = d.indices.chunks_exact(3).any(|t| {
        let l = t.iter().filter_map(|&i| long.get(i as usize).copied());
        let (lo, hi) = l.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
        lo <= -0.995 && hi >= 0.995
    });
    if !across {
        return None;
    }
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for tri in d.indices.chunks_exact(3) {
        let Some(poly) = tri.iter().map(|&i| vs.get(i as usize).map(|v| (*v, long[i as usize]))).collect::<Option<Vec<_>>>() else { continue };
        for sign in [-1.0f32, 1.0] {
            // the part of the triangle past the picture's edge on this side
            let cut = clip_poly(&poly, sign, edge);
            if cut.len() < 3 {
                continue;
            }
            let base = (vertices.len() / 24) as u32;
            for (v, _) in &cut {
                wide2::write_v(v, &mut vertices);
            }
            for i in 1..cut.len() as u32 - 1 {
                indices.extend_from_slice(&[base, base + i, base + i + 1]);
            }
        }
    }
    if indices.is_empty() {
        return None;
    }
    let mut out = d.clone();
    out.vertices = vertices;
    out.indices = indices;
    // the fan may turn some triangles over
    out.cull = 0;
    Some(out)
}

/// Sutherland-Hodgman against sign·long >= edge.
fn clip_poly(poly: &[(V, f32)], sign: f32, edge: f32) -> Vec<(V, f32)> {
    let inside = |l: f32| sign * l >= edge;
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, la) = poly[i];
        let (b, lb) = poly[(i + 1) % poly.len()];
        if inside(la) {
            out.push((a, la));
        }
        if inside(la) != inside(lb) {
            let t = (sign * edge - la) / (lb - la);
            out.push((wide2::lerp_v(&a, &b, t), sign * edge));
        }
    }
    out
}
