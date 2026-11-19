//! Reading the engine's 2D layer, prim::Context and its prim::State list,
//! into native draws.
//!
//! Every screen the game shows is drawn by prim::Context::RenderTarget
//! walking a sorted list of 24-byte prim::State records. A state either
//! carries 2D geometry (quads, triangles, strips, fans, lines) with an index
//! into the context's tables of textures, colours, matrices, cameras,
//! scissor/viewport rectangles and depth/alpha modes, or names a callback
//! (prim::Callback) that draws something else in its place: 3D scenes,
//! particle effects, captures, post effects. The layout below was recovered
//! from prim::Context::UpdateState (0x00126AF0), UpdateTexture (0x001313B0),
//! UpdateBlend (0x001311BC), UpdateWorld (0x00131294) and RenderCall
//! (0x001267B8); see research/GFX-ARCHITECTURE.md.
//!
//! The game's own lookup tables (depth/alpha functions, culling, TEV
//! operands, filters) are read from the executable's literal pools, so the
//! native draw makes the same choices the game's code makes.

use zakuro_core::System;

/// Guest addresses of literal-pool words holding table addresses and
/// constants (USA Special Edition v2).
mod lit {
    pub const VB_SELECTOR_OFFSET: u32 = 0x0012_6954; // RenderCall: ctx offset of the vertex-buffer selector
    pub const CHUNK_INDICES: u32 = 0x0012_6958; // RenderCall: indices per draw when split
    pub const CHUNK_QUAD_VERTICES: u32 = 0x0012_695C; // RenderCall: vertex advance per split quad draw
    pub const DEPTH_FUNC: u32 = 0x0012_7318; // UpdateState: depth mode -> PICA depth func (bytes)
    pub const ALPHA_FUNC: u32 = 0x0012_731C; // UpdateState: alpha mode -> PICA alpha func (bytes)
    pub const SHAPE_INDEX_SLOT: u32 = 0x0012_7320; // UpdateState: shape -> index-buffer slot (words)
    pub const INDEX_FORMAT: u32 = 0x0012_7324; // UpdateState: GL index type
    pub const CULL: u32 = 0x0012_7328; // UpdateState: cull mode -> PICA cull (bytes)
    pub const OPERANDS: u32 = 0x0012_732C; // UpdateState: tev operand sets, 3 bytes each
    pub const TEVOP_VECTOR_OFFSET: u32 = 0x0013_15F0; // UpdateTexture: ctx offset of the tevop vector
    pub const WRAP: u32 = 0x0013_881C; // ITexture::GetPicaWarpMode table
    pub const MAG: u32 = 0x0013_8830; // ITexture::GetPicaMagFilter table
    pub const MIN: u32 = 0x0013_8844; // ITexture::GetPicaMinFilter table
    pub const COLOR_FORMAT: u32 = 0x0011_BFD4; // RenderTarget: GL type of the colour attribute
}

/// prim::Context field offsets.
mod ctxoff {
    pub const DISPLAY_LISTS: u32 = 0x1878; // 3 x std::vector<State*>, 12 bytes each
    pub const MATRICES: u32 = 0x2404; // MTX34 array, 0x30 each
    pub const COLORS: u32 = 0x2410; // Color8 array
    pub const RECTS: u32 = 0x241C; // scissor/viewport rects, 8 bytes each
    pub const CAMERAS: u32 = 0x2458; // ICamera* array
    pub const TEXTURES: u32 = 0x2464; // ITexture* array
    pub const VERTEX_BUFFERS: u32 = 0x2470; // VertexBuffer* array
    pub const CALLBACKS: u32 = 0x247C; // prim::Callback* array
    pub const DEPTH_ALPHA: u32 = 0x2488; // 4-byte depth/alpha modes
    pub const VB_PHYS: u32 = 0x29CC; // physical vertex-buffer addresses
    pub const IB_PHYS: u32 = 0x29D4; // physical index-buffer addresses
}

/// The game's tables, read once from the executable.
#[derive(Clone, Copy, Debug)]
pub struct Tables {
    vb_selector_offset: u32,
    chunk_indices: u32,
    chunk_quad_vertices: u32,
    depth_func: u32,
    alpha_func: u32,
    shape_index_slot: u32,
    index_u16: bool,
    cull: u32,
    operands: u32,
    tevop_vector_offset: u32,
    wrap: u32,
    mag: u32,
    min: u32,
    color_unsigned: bool,
}

pub fn rd(system: &mut System, vaddr: u32, out: &mut [u8]) {
    system.memory.read_bytes(vaddr, out);
}
pub fn r8(system: &mut System, vaddr: u32) -> u8 {
    let mut b = [0u8; 1];
    rd(system, vaddr, &mut b);
    b[0]
}
pub fn r16(system: &mut System, vaddr: u32) -> u16 {
    let mut b = [0u8; 2];
    rd(system, vaddr, &mut b);
    u16::from_le_bytes(b)
}
pub fn r32(system: &mut System, vaddr: u32) -> u32 {
    let mut b = [0u8; 4];
    rd(system, vaddr, &mut b);
    u32::from_le_bytes(b)
}
/// a word into guest memory (the native 3D path's compacted state list).
pub fn w32(system: &mut System, vaddr: u32, value: u32) {
    system.memory.write_bytes(vaddr, &value.to_le_bytes());
}
fn rf32s<const N: usize>(system: &mut System, vaddr: u32) -> [f32; N] {
    let mut out = [0f32; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = f32::from_bits(r32(system, vaddr + 4 * i as u32));
    }
    out
}
/// a physical address (VRAM, FCRAM) as the game's virtual one.
pub fn virt(system: &System, phys: u32) -> u32 {
    zakuro_core::services::gsp::physical_to_virtual(system, phys)
}

impl Tables {
    pub fn read(system: &mut System) -> Tables {
        let color_type = r32(system, zakuro_core::region::us(lit::COLOR_FORMAT));
        Tables {
            vb_selector_offset: r32(system, zakuro_core::region::us(lit::VB_SELECTOR_OFFSET)),
            chunk_indices: r32(system, zakuro_core::region::us(lit::CHUNK_INDICES)),
            chunk_quad_vertices: r32(system, zakuro_core::region::us(lit::CHUNK_QUAD_VERTICES)),
            depth_func: r32(system, zakuro_core::region::us(lit::DEPTH_FUNC)),
            alpha_func: r32(system, zakuro_core::region::us(lit::ALPHA_FUNC)),
            shape_index_slot: r32(system, zakuro_core::region::us(lit::SHAPE_INDEX_SLOT)),
            index_u16: r32(system, zakuro_core::region::us(lit::INDEX_FORMAT)) == 0x1403,
            cull: r32(system, zakuro_core::region::us(lit::CULL)),
            operands: r32(system, zakuro_core::region::us(lit::OPERANDS)),
            tevop_vector_offset: r32(system, zakuro_core::region::us(lit::TEVOP_VECTOR_OFFSET)),
            wrap: r32(system, zakuro_core::region::us(lit::WRAP)),
            mag: r32(system, zakuro_core::region::us(lit::MAG)),
            min: r32(system, zakuro_core::region::us(lit::MIN)),
            color_unsigned: color_type == 0x1401,
        }
    }
}

/// A texture as the game set it up (ITexture), by where its texels are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexRef {
    /// virtual address of the texels.
    pub addr: u32,
    pub width: u32,
    pub height: u32,
    /// PICA texture format number.
    pub format: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sampling {
    /// PICA wrap: 0 clamp to edge, 1 clamp to border, 2 repeat, 3 mirrored repeat.
    /// gfx-2d: this is the s (u, across) wrap; t has its own below
    pub wrap: u8,
    /// gfx-2d: the t (v, down) wrap. The PICA sets the two apart (unit
    /// parameter bits 12-14 s, 8-10 t); using s for both blended the
    /// opposite edge into planes clamped along t (the Stage Complete line)
    pub wrap_t: u8,
    /// gfx-2d: pixel art (the map's unit sprites): above scale 1 sampled
    /// "sharp bilinear", texels kept as blocks with their edges smoothed
    /// over one render pixel, so a sprite at a fraction of a pixel or at
    /// a scale that is not a whole number keeps even texels (render.rs)
    pub sharp: bool,
    pub linear_mag: bool,
    pub linear_min: bool,
}

/// One TEV stage as the game configures it (PICA source, operand and
/// combine numbering; scale is a shift: 0 x1, 1 x2, 2 x4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stage {
    pub src_rgb: [u8; 3],
    pub src_a: [u8; 3],
    pub op_rgb: [u8; 3],
    pub op_a: [u8; 3],
    pub comb_rgb: u8,
    pub comb_a: u8,
    pub scale_rgb: u8,
    pub scale_a: u8,
    pub konst: [u8; 4],
}

/// The texture combiner: six stages, which of the first four stages update
/// the combiner buffer (bit per stage), and the buffer's initial colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Combiner {
    pub stages: [Stage; 6],
    pub update_rgb: u8,
    pub update_a: u8,
    pub buffer: [u8; 4],
}

impl Combiner {
    /// the combiner as prim::Context::ResetCombiner (0x0011C06C) leaves it:
    /// stage 0 takes the vertex colour, texture 0 and the constant, the
    /// others pass the previous stage on.
    pub fn prim_reset() -> Combiner {
        let pass = Stage { src_rgb: [0xF, 0xE, 0xE], src_a: [0xF, 0xE, 0xE], ..Stage::default() };
        let mut stages = [pass; 6];
        stages[0] = Stage { src_rgb: [0, 3, 0xE], src_a: [0, 3, 0xE], ..Stage::default() };
        Combiner { stages, update_rgb: 0, update_a: 0, buffer: [0; 4] }
    }
}

/// Blending as the PICA takes it: enable, equations (rgb, alpha) and
/// factors (src rgb, dst rgb, src alpha, dst alpha), PICA numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend {
    pub enable: bool,
    pub eq: [u8; 2],
    pub func: [u8; 4],
}

impl Blend {
    /// prim::Context::UpdateBlend (0x001311BC) for a prim::BlendMode.
    pub fn prim(mode: u8) -> Blend {
        let on = |eq: u8, src: u8, dst: u8| Blend { enable: true, eq: [eq, eq], func: [src, dst, src, dst] };
        match mode {
            0 => on(0, 6, 7),
            1 => on(0, 6, 1),
            2 => on(2, 6, 1),
            4 => on(0, 2, 1),
            _ => Blend { enable: false, eq: [0, 0], func: [1, 0, 1, 0] },
        }
    }
}

/// OnBegin of the prim::Callback classes the native renderer draws itself.
pub const FACE_CALLBACK_ON_BEGIN: u32 = 0x001A_DCF4;
pub const FACE_OPTION_ON_BEGIN: u32 = 0x0020_B59C;
pub const LOCUS_ON_BEGIN: u32 = 0x0020_410C;
pub const MOVIE_ON_BEGIN: u32 = 0x0022_5F74;

/// One native 2D draw: an indexed triangle list with the state the game's
/// prim layer chose for it.
#[derive(Clone, Debug)]
pub struct Draw {
    /// 24-byte vertices: x, y, z, u, v (f32) and RGBA8.
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    pub world: [f32; 12],
    pub view_proj: [f32; 16],
    pub tex_matrix: [f32; 12],
    /// the colour uniform, multiplied with the vertex colour.
    pub color: [f32; 4],
    /// texture units 0..2 (unit 0 is the state's texture).
    pub textures: [Option<(TexRef, Sampling)>; 3],
    /// the full texture combiner.
    pub combiner: Combiner,
    pub blend: Blend,
    pub depth_test: bool,
    pub depth_func: u8,
    pub depth_write: bool,
    pub alpha_test: bool,
    pub alpha_func: u8,
    pub alpha_ref: u8,
    pub cull: u8,
    /// framebuffer-space rectangles (x, y, w, h), in the display's 240-wide
    /// portrait framebuffer units.
    pub scissor: [i32; 4],
    pub viewport: [i32; 4],
    /// an H3D mesh draw instead (scene.rs); the fields above unused
    pub shaded: Option<std::sync::Arc<crate::scene::Shaded>>,
    /// gfx-post: a post effect recorded natively (post.rs) instead; the
    /// fields above unused
    pub post: Option<std::sync::Arc<crate::post::Block>>,
    /// gfx-gaps: the state's polygon offset, added to the picture's depth
    /// after clipping (Pica::PolygonOffset's depth-map offset)
    pub depth_offset: f32,
}

/// Why a state can't be drawn natively yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Island {
    /// a prim::Callback draws in its place (3D scene, effect, capture...);
    /// the address of its OnBegin, which names its class.
    Callback(u32),
    /// lines, drawn with a geometry shader (gfx-lines: drawn natively
    /// unless FATES_GFX_LINES=0, lines.rs).
    Lines,
    /// depth-tested against what an island drew.
    Depth,
}

pub enum Decoded {
    Draw(Draw),
    Skip,
    Island(Island),
}

/// the display's framebuffer size (width, height) in portrait units:
/// 240 x 400 upper, 240 x 320 lower.
pub fn display_size(disp: u32) -> (i32, i32) {
    if disp == 0 { (240, 400) } else { (240, 320) }
}

/// The display's state list: the State pointers RenderTarget will walk.
pub fn state_list(system: &mut System, prim: u32, disp: u32) -> Vec<u32> {
    let list = prim + ctxoff::DISPLAY_LISTS + disp * 12;
    let begin = r32(system, list);
    let end = r32(system, list + 4);
    if end < begin || end - begin > 0x10000 {
        return Vec::new();
    }
    (0..(end - begin) / 4).map(|i| r32(system, begin + 4 * i)).collect()
}

/// Whether RenderTarget would draw anything for the state, and its callback.
pub fn state_callback(system: &mut System, prim: u32, state: u32) -> (bool, u32) {
    let w0 = r32(system, state);
    let vb = r8(system, state + 0xD) as u32;
    let count = r16(system, state + 0x12) as u32;
    let cb_index = (w0 & 0xFF_FFFF) >> 17;
    let drawn = vb + count + cb_index != 0;
    let table = r32(system, prim + ctxoff::CALLBACKS);
    let callback = if cb_index != 0 || drawn { r32(system, table + cb_index * 4) } else { 0 };
    (drawn, callback)
}

fn mtx34_mul(a: &[f32; 12], b: &[f32; 12]) -> [f32; 12] {
    let mut out = [0f32; 12];
    for r in 0..3 {
        for c in 0..4 {
            let mut v = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c];
            if c == 3 {
                v += a[r * 4 + 3];
            }
            out[r * 4 + c] = v;
        }
    }
    out
}

/// Decodes one state into a native draw, as UpdateState + RenderCall would
/// set it up.
pub fn decode(system: &mut System, t: &Tables, prim: u32, disp: u32, state: u32) -> Decoded {
    let (drawn, callback) = state_callback(system, prim, state);
    if !drawn {
        return Decoded::Skip;
    }
    let mut native_callback = 0;
    if callback != 0 {
        // the callback's class, by its OnBegin (first virtual function)
        let vtable = r32(system, callback);
        let on_begin = zakuro_core::region::to_us(r32(system, vtable)); // eur: the USA OnBegin
        let known = matches!(on_begin, FACE_CALLBACK_ON_BEGIN | FACE_OPTION_ON_BEGIN | LOCUS_ON_BEGIN | MOVIE_ON_BEGIN);
        // gfx-gaps: UnitIcon::DrawCallback sets the combiner for the state's own quad
        let known = known || (on_begin == crate::unit_icon::DRAW_ON_BEGIN && crate::unit_icon::enabled());
        if known && crate::perf::var("FATES_GFX_FACE").map_or(true, |v| v != "0") { // gfx-perf: cached env
            native_callback = on_begin;
        } else {
            return Decoded::Island(Island::Callback(on_begin));
        }
    }
    let w0 = r32(system, state);
    let w1 = r32(system, state + 4);
    let w2 = r32(system, state + 8);
    let tex_index = r8(system, state + 0xC) as u32;
    let vb_index = r8(system, state + 0xD) as u32;
    let mut first = r16(system, state + 0x10) as u32;
    let mut vcount = r16(system, state + 0x12) as u32;
    let shape = w0 & 7;

    // RenderCall: where the vertices are
    let selector = r32(system, prim + t.vb_selector_offset);
    let mut vb_phys = r32(system, prim + selector * 4 + ctxoff::VB_PHYS);
    if vb_index != 0 {
        let vbs = r32(system, prim + ctxoff::VERTEX_BUFFERS);
        let ext = r32(system, vbs + vb_index * 4);
        if ext != 0 {
            let inner = r32(system, ext + 0xC);
            first = 0;
            vcount = if inner != 0 { r32(system, inner + 0x14) } else { 0 };
            vb_phys = if inner != 0 { r32(system, inner + 4) } else { 0 };
        }
    }
    let index_count = match shape {
        0 => (vcount * 6) / 4,
        1 => vcount,
        2 | 3 => vcount.saturating_sub(2) * 3,
        // gfx-lines: Pica::GeoDraw of the state's indices (lines.rs)
        4 if crate::lines::enabled() => vcount,
        4 => return Decoded::Island(Island::Lines),
        _ => return Decoded::Skip,
    };
    if index_count == 0 || vb_phys == 0 {
        return Decoded::Skip;
    }
    // UpdateState: the index buffer for the shape
    let slot = r32(system, t.shape_index_slot + shape * 4);
    let ib_phys = r32(system, prim + slot * 4 + ctxoff::IB_PHYS);
    let ib = virt(system, ib_phys);
    let vb = virt(system, vb_phys);

    // RenderCall splits draws of more than 0x2000 indices, moving the vertex
    // base along; the index buffer is read from its start each time.
    let mut chunks = Vec::new();
    if index_count <= 0x2000 {
        chunks.push((first, index_count));
    } else {
        let (per, advance) = match shape {
            0 => (t.chunk_indices, t.chunk_quad_vertices),
            1 => (t.chunk_indices, t.chunk_indices),
            4 => (0x2000, 0x2000), // gfx-lines: RenderCall's GeoDraw split
            _ => return Decoded::Skip,
        };
        let mut left = index_count;
        while left > 0 {
            let n = per.min(left);
            chunks.push((first, n));
            left -= n;
            first += advance;
        }
    }
    let isize = if t.index_u16 { 2 } else { 1 };
    let mut raw_indices = vec![0u8; (chunks.iter().map(|c| c.1).max().unwrap_or(0) * isize) as usize];
    rd(system, ib, &mut raw_indices);
    let index_at = |i: usize| -> u32 {
        if isize == 2 { u16::from_le_bytes([raw_indices[2 * i], raw_indices[2 * i + 1]]) as u32 } else { raw_indices[i] as u32 }
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for &(base, n) in &chunks {
        let max = (0..n as usize).map(index_at).max().unwrap_or(0);
        let start = (vertices.len() / 24) as u32;
        let mut bytes = vec![0u8; ((max + 1) * 24) as usize];
        rd(system, vb + base * 24, &mut bytes);
        vertices.extend_from_slice(&bytes);
        indices.extend((0..n as usize).map(|i| start + index_at(i)));
    }
    if !t.color_unsigned {
        log::warn!("gfx: prim colour attribute is not unsigned bytes");
    }

    // UpdateState: depth and alpha test
    let modes = r32(system, prim + ctxoff::DEPTH_ALPHA);
    let m = (w2 & 0x3FFF) >> 8;
    let mut da = [0u8; 4];
    rd(system, modes + m * 4, &mut da);
    let depth_test = !(da[0] == 1 && da[1] == 0);
    let depth_func = r8(system, t.depth_func + da[0] as u32);
    let alpha_func = r8(system, t.alpha_func + da[2] as u32);

    // colour, world matrix, camera
    let colors = r32(system, prim + ctxoff::COLORS);
    let mut c8 = [0u8; 4];
    rd(system, colors + (w0 >> 24) * 4, &mut c8);
    let color = c8.map(|c| c as f32 / 255.0);
    let matrices = r32(system, prim + ctxoff::MATRICES);
    let world: [f32; 12] = rf32s(system, matrices + (w1 & 0xFF) * 0x30);
    let cam_index = if w0 & 0x1E000 == 0 { (w0 & 0x1F) >> 3 } else { (w0 & 0x1FFFF) >> 13 };
    let cameras = r32(system, prim + ctxoff::CAMERAS);
    let camera = r32(system, cameras + cam_index * 4);
    let view_proj: [f32; 16] = rf32s(system, camera + 0x228);
    let depth_offset = polygon_offset(system, prim, w2); // gfx-gaps

    // rectangles
    let (fw, fh) = display_size(disp);
    let rects = r32(system, prim + ctxoff::RECTS);
    let rect = |system: &mut System, index: u32| -> [i32; 4] {
        let mut b = [0u8; 8];
        rd(system, rects + index * 8, &mut b);
        let s = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as i32;
        [s(0), s(2), s(4), s(6)]
    };
    let si = (w1 & 0x3FF_FFFF) >> 20;
    let scissor = if si == 0 { [0, 0, fw, fh] } else { rect(system, si) };
    let vi = w1 >> 26;
    let viewport = if vi == 0 { [0, 0, fw, fh] } else { rect(system, vi) };
    let cull = r8(system, t.cull + ((w0 & 0x1FFF) >> 11));

    // TEV operands
    let oi = (w0 & 0x7FF) >> 8;
    let mut operands = [0u8; 3];
    rd(system, t.operands + oi * 3, &mut operands);

    // UpdateTexture
    let textures = r32(system, prim + ctxoff::TEXTURES);
    let tex = r32(system, textures + tex_index * 4);
    let (texture, tex_matrix, combine_rgb, combine_alpha) = if tex == 0 {
        (None, [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.], 0, 0)
    } else {
        let tevops = r32(system, prim + t.tevop_vector_offset);
        let flags = r32(system, tevops + (w2 & 0xFF) * 4);
        let filter = r8(system, tex + 0x12) as u32;
        let wrap_index = r8(system, tex + 0x13) as u32;
        let mut wrap = r8(system, t.wrap + wrap_index);
        let mut min = r8(system, t.min + filter);
        let mut mag = r8(system, t.mag + filter);
        let mut rgb_modulate = true;
        let mut alpha_modulate = true;
        if flags & 1 != 0 {
            mag = 0;
            min = 0;
        }
        if flags & 2 != 0 {
            min = 3;
            mag = 1;
        }
        if flags & 4 != 0 {
            wrap = 0;
        }
        if flags & 8 != 0 {
            wrap = 2;
        }
        if flags & 0x10 != 0 {
            wrap = 3;
        }
        if flags & 0x60 != 0 {
            rgb_modulate = flags & 0x20 == 0;
            if flags & 0x40 != 0 {
                alpha_modulate = false;
            }
        }
        let format = r8(system, tex + 0x11) as u32;
        if format == 8 || format == 11 {
            rgb_modulate = false;
        }
        let base: [f32; 12] = rf32s(system, tex + 0x18);
        let tex_matrix = if w1 & 0xFF00 == 0 {
            base
        } else {
            let m: [f32; 12] = rf32s(system, matrices + ((w1 & 0xFFFF) >> 8) * 0x30);
            mtx34_mul(&base, &m)
        };
        let texref = TexRef {
            addr: {
                let a = r32(system, tex + 8);
                if crate::perf::var_os("FATES_GFX_TEXTRACE").is_some() { // gfx-perf: cached env
                    let mut raw = [0u8; 0x48];
                    rd(system, tex, &mut raw);
                    eprintln!("gfx-tex {tex:08X} +8 {a:08X} -> {:08X} raw {:02X?}", virt(system, a), raw);
                }
                virt(system, a)
            },
            width: r16(system, tex + 0xC) as u32,
            height: r16(system, tex + 0xE) as u32,
            format,
        };
        let sampling = Sampling { wrap, wrap_t: wrap, sharp: false, linear_mag: mag == 1, linear_min: matches!(min, 1 | 3 | 5) };
        (Some((texref, sampling)), tex_matrix, rgb_modulate as u8, alpha_modulate as u8)
    };

    // gfx-lines: the line programs' geometry, in clip space (lines.rs)
    let (vertices, indices, world, view_proj, tex_matrix) = if shape == 4 {
        let width = crate::lines::width(system, prim, w1);
        crate::lines::trace(system, state, &vertices, &indices, &world, &view_proj, width, disp, cull, texture.is_some());
        let (mut v, i) = crate::lines::expand(&vertices, &indices, 0x2000, &world, &view_proj, width, disp);
        crate::lines::nudge(&mut v);
        if i.is_empty() {
            return Decoded::Skip;
        }
        (v, i, crate::lines::IDENTITY_34, crate::lines::IDENTITY_44, crate::lines::IDENTITY_34)
    } else {
        (vertices, indices, world, view_proj, tex_matrix)
    };
    let mut combiner = Combiner::prim_reset();
    combiner.stages[0].op_rgb = operands;
    combiner.stages[0].comb_rgb = combine_rgb;
    combiner.stages[0].comb_a = combine_alpha;
    let mut textures = [texture, None, None];
    let mut blend = Blend::prim(((w0 & 0xFF) >> 5) as u8);
    match native_callback {
        FACE_CALLBACK_ON_BEGIN => face_callback(system, prim, w2, &mut combiner, &mut textures),
        FACE_OPTION_ON_BEGIN => face_option(&mut combiner),
        LOCUS_ON_BEGIN => locus(&mut combiner, &mut blend),
        crate::unit_icon::DRAW_ON_BEGIN => crate::unit_icon::draw_callback(system, prim, w2, &mut combiner), // gfx-gaps
        MOVIE_ON_BEGIN => {
            // MovieStereoDrawCallback::OnBegin (0x00225F74): texture unit 0
            // is the movie frame (the callback's ITexture at +8, which y2r
            // fills), linear filtered, clamped
            let frame = r32(system, callback + 8);
            if let Some((tex, _)) = texref(system, frame) {
                textures[0] = Some((tex, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: true, linear_min: true }));
            }
        }
        _ => {}
    }
    #[allow(unused_mut)]
    let mut draw = Draw {
        vertices,
        indices,
        world,
        view_proj,
        tex_matrix,
        color,
        textures,
        combiner,
        blend,
        depth_test,
        depth_func,
        depth_write: da[1] != 0,
        alpha_test: da[2] != 1,
        alpha_func,
        alpha_ref: da[3],
        cull,
        scissor,
        viewport,
        shaded: None,
        post: None,
        depth_offset,
    };
    // gfx-2d: a talk portrait's art cut where the console's screen ended
    // gfx-2d: portraits magnified smoothly above the console's resolution
    if matches!(native_callback, FACE_CALLBACK_ON_BEGIN | FACE_OPTION_ON_BEGIN) && crate::portrait::smooth(system) {
        for t in draw.textures.iter_mut().flatten() {
            t.1.linear_mag = true;
            t.1.linear_min = true;
        }
    }
    if native_callback == FACE_CALLBACK_ON_BEGIN && disp == 0 {
        crate::portrait::fade_cut(&mut draw, zakuro_gpu::wide::factor());
    }
    Decoded::Draw(draw)
}

/// a FaceInstance texture slot (TexFile at +0x44 hair, +0x4C accessory 1),
/// as FaceInstance::GetTextureHair (0x004EBBA0) / GetTextureAccessory1
/// (0x001B060C) read it: the file's first texture once it is loaded, else
/// ITexture::GetNullTex (literal 0x004EBBDC).
fn face_texture(system: &mut System, file: u32) -> u32 {
    let object = r32(system, file + 4);
    if object != 0 && r32(system, object + 0x7C) & 0x0400_0000 != 0 {
        // TexFile::GetTexture(0) (0x0011D074)
        if r32(system, object + 0xB4) & 0x4000 != 0 && r32(system, object + 0xC4) as i32 > 0 {
            return r32(system, object + 0xC0);
        }
    }
    let null = r32(system, zakuro_core::region::us(0x004E_BBDC));
    r32(system, null)
}

fn texref(system: &mut System, tex: u32) -> Option<(TexRef, Sampling)> {
    if tex == 0 {
        return None;
    }
    let addr = r32(system, tex + 8);
    let texref = TexRef {
        addr: virt(system, addr),
        width: r16(system, tex + 0xC) as u32,
        height: r16(system, tex + 0xE) as u32,
        format: r8(system, tex + 0x11) as u32,
    };
    // FaceCallback::SetupTexture: clamp to edge, nearest
    Some((texref, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: false, linear_min: false }))
}

/// FaceCallback::OnBegin (0x001ADCF4) and SetupCore (0x001ADD68), as their
/// disassembly sets the units and the combiner: the face (unit 0) under the
/// accessory (unit 2), the hair (unit 1) tinted by the hair colour at
/// FaceInstance +0x10C and laid over them, then the vertex colour applied.
fn face_callback(system: &mut System, prim: u32, w2: u32, c: &mut Combiner, textures: &mut [Option<(TexRef, Sampling)>; 3]) {
    // prim::Callback::GetBufferAddr (0x0053A4B4): the state's buffer slot
    let table = r32(system, prim + 0x2494);
    // gfx-terrain: the slot is w2 bits 20-25 ((w2 & 0x3FFFFFF) >> 20, as
    // GetBufferAddr reads it), not bits 26-31 (the polygon offset index);
    // talk-scene portraits use slots other than 0
    let entry = r32(system, table + ((w2 & 0x3FF_FFFF) >> 20) * 8);
    let instance = if entry != 0 { r32(system, entry) } else { 0 };
    if instance == 0 {
        return;
    }
    let hair = face_texture(system, instance + 0x44);
    let accessory = face_texture(system, instance + 0x4C);
    textures[1] = texref(system, hair);
    textures[2] = texref(system, accessory);
    let mut tint = [0u8; 4];
    rd(system, instance + 0x10C, &mut tint);
    c.update_rgb = 1;
    c.update_a = 1;
    let half = [0x80, 0x80, 0x80, 0xFF];
    let sunglasses = textures[2].is_some_and(|(t, _)| is_sunglasses(system, &t)); // sunglasses
    let s = &mut c.stages;
    s[0] = Stage { src_rgb: [5, 3, 5], src_a: [5, 3, 0], op_rgb: [0, 0, 2], op_a: [0, 0, 0], comb_rgb: 4, comb_a: 2, konst: half, ..s[0] };
    s[1] = Stage { src_rgb: [4, 0xE, 0], src_a: [4, 0xE, 0], comb_rgb: 1, comb_a: 1, scale_rgb: 1, scale_a: 0, konst: tint, ..s[1] };
    s[2] = Stage { src_rgb: [0xF, 0xD, 0xF], src_a: [0xF, 0xD, 0], op_rgb: [0, 0, 2], op_a: [0, 0, 0], comb_rgb: 4, comb_a: 2, konst: half, ..s[2] };
    s[3] = Stage { src_rgb: [0, 0, 0xF], src_a: [0, 0xF, 0], comb_rgb: 9, comb_a: 1, konst: [0xFF; 4], ..s[3] };
    s[4] = Stage { src_rgb: [0xF, 0, 0], src_a: [0xF, 0, 0], comb_rgb: 3, comb_a: 0, konst: [0xFF; 4], ..s[4] };
    // sunglasses: Haiku's Sunglasses detail goes over the hair: the face
    // alone in the buffer, the hair over it, then the accessory over both,
    // then the vertex colour as above. Every other detail keeps the game's
    // order (under the hair).
    if sunglasses {
        s[0] = Stage { src_rgb: [3, 0, 0], src_a: [3, 0, 0], op_rgb: [0, 0, 0], op_a: [0, 0, 0], comb_rgb: 0, comb_a: 0, konst: half, ..s[0] };
        s[3] = Stage { src_rgb: [5, 0xF, 5], src_a: [5, 0xF, 0], op_rgb: [0, 0, 2], op_a: [0, 0, 0], comb_rgb: 4, comb_a: 2, scale_rgb: 0, scale_a: 0, konst: half, ..s[3] };
        s[4] = Stage { src_rgb: [0, 0, 0xF], src_a: [0, 0xF, 0], op_rgb: [0, 0, 0], op_a: [0, 0, 0], comb_rgb: 9, comb_a: 1, scale_rgb: 0, scale_a: 0, konst: [0xFF; 4], ..s[4] };
        s[5] = Stage { src_rgb: [0xF, 0, 0], src_a: [0xF, 0, 0], op_rgb: [0, 0, 0], op_a: [0, 0, 0], comb_rgb: 3, comb_a: 0, scale_rgb: 0, scale_a: 0, konst: [0xFF; 4], ..s[5] };
    }
}

/// sunglasses: Haiku's Sunglasses portrait layers (assets/builtin, made by
/// dev/sunglasses/make_art.py) carry this in their first ETC1A4 block,
/// whose alpha is 0 (a transparent corner, never seen): the renderer knows
/// them by it, in the game's memory, whatever texture pack draws them.
pub const SUNGLASSES_MAGIC: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, b'S', b'U', b'N', b'G', b'L', b'A', b'S', b'S'];

/// sunglasses: whether a texture is one of Haiku's Sunglasses layers.
pub fn is_sunglasses(system: &mut System, t: &TexRef) -> bool {
    if t.format != 13 || t.addr == 0 || !system.memory.is_readable(t.addr, 16) {
        return false;
    }
    let mut head = [0u8; 16];
    rd(system, t.addr, &mut head);
    // FATES_SUNGLASSES_UNDER=1 (dev A/B): the game's own order, the layer
    // under the hair like every other detail
    head == SUNGLASSES_MAGIC && crate::perf::var_os("FATES_SUNGLASSES_UNDER").is_none()
}

/// FaceCallbackOption::OnBegin (0x0020B59C), from its disassembly: no buffer
/// updates; stage 0 = (vertex colour + vertex colour) * texture 0 (with the
/// state's operands), alpha vertex * texture; stage 1 = previous + vertex
/// colour - 0.5, alpha previous.
fn face_option(c: &mut Combiner) {
    c.update_rgb = 0;
    c.update_a = 0;
    let s = &mut c.stages;
    s[0] = Stage { src_rgb: [0, 0, 3], src_a: [0, 3, 0], comb_rgb: 9, comb_a: 1, konst: [0xFF; 4], ..s[0] };
    s[1] = Stage { src_rgb: [0xF, 0, 0], src_a: [0xF, 0, 0], comb_rgb: 3, comb_a: 0, konst: [0xFF; 4], ..s[1] };
}

/// PrimLocusCallback::OnBegin (0x0020410C), from its disassembly: additive
/// colour (one, one), alpha reverse-subtracted; TEV stage 4 = texture 0 *
/// vertex colour (alpha from texture red), stage 5 scales by vertex alpha.
fn locus(c: &mut Combiner, blend: &mut Blend) {
    *blend = Blend { enable: true, eq: [0, 2], func: [1, 1, 7, 8] };
    let s = &mut c.stages;
    s[4] = Stage { src_rgb: [3, 0, 0xE], src_a: [3, 0, 0xE], op_rgb: [0, 0, 0], op_a: [2, 0, 0], comb_rgb: 1, comb_a: 1, ..s[4] };
    s[5] = Stage { src_rgb: [0xF, 0, 0xE], src_a: [0xF, 0xE, 0xE], op_rgb: [0, 2, 0], op_a: [0, 0, 0], comb_rgb: 1, comb_a: 0, ..s[5] };
}


/// gfx-gaps: the state UpdateState sets up for a state before its
/// callback's OnBegin runs (camera, world matrix, colour, rectangles,
/// depth/alpha modes, cull, blend, texture and stage 0), without its
/// geometry: what a callback that draws through its own Pica calls
/// (UnitIcon::BatchCallback, unit_icon.rs) starts from. The same reads as
/// `decode`, which bails out at the callback.
pub fn state_setup(system: &mut System, t: &Tables, prim: u32, disp: u32, state: u32) -> Draw {
    let w0 = r32(system, state);
    let w1 = r32(system, state + 4);
    let w2 = r32(system, state + 8);
    let tex_index = r8(system, state + 0xC) as u32;
    let modes = r32(system, prim + ctxoff::DEPTH_ALPHA);
    let m = (w2 & 0x3FFF) >> 8;
    let mut da = [0u8; 4];
    rd(system, modes + m * 4, &mut da);
    let depth_test = !(da[0] == 1 && da[1] == 0);
    let depth_func = r8(system, t.depth_func + da[0] as u32);
    let alpha_func = r8(system, t.alpha_func + da[2] as u32);
    let colors = r32(system, prim + ctxoff::COLORS);
    let mut c8 = [0u8; 4];
    rd(system, colors + (w0 >> 24) * 4, &mut c8);
    let matrices = r32(system, prim + ctxoff::MATRICES);
    let world: [f32; 12] = rf32s(system, matrices + (w1 & 0xFF) * 0x30);
    let cam_index = if w0 & 0x1E000 == 0 { (w0 & 0x1F) >> 3 } else { (w0 & 0x1FFFF) >> 13 };
    let cameras = r32(system, prim + ctxoff::CAMERAS);
    let camera = r32(system, cameras + cam_index * 4);
    let view_proj: [f32; 16] = rf32s(system, camera + 0x228);
    let depth_offset = polygon_offset(system, prim, w2); // gfx-gaps
    let (fw, fh) = display_size(disp);
    let rects = r32(system, prim + ctxoff::RECTS);
    let rect = |system: &mut System, index: u32| -> [i32; 4] {
        let mut b = [0u8; 8];
        rd(system, rects + index * 8, &mut b);
        let s = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as i32;
        [s(0), s(2), s(4), s(6)]
    };
    let si = (w1 & 0x3FF_FFFF) >> 20;
    let scissor = if si == 0 { [0, 0, fw, fh] } else { rect(system, si) };
    let vi = w1 >> 26;
    let viewport = if vi == 0 { [0, 0, fw, fh] } else { rect(system, vi) };
    let cull = r8(system, t.cull + ((w0 & 0x1FFF) >> 11));
    let oi = (w0 & 0x7FF) >> 8;
    let mut operands = [0u8; 3];
    rd(system, t.operands + oi * 3, &mut operands);
    let textures = r32(system, prim + ctxoff::TEXTURES);
    let tex = r32(system, textures + tex_index * 4);
    let mut combiner = Combiner::prim_reset();
    combiner.stages[0].op_rgb = operands;
    let mut tex_matrix = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];
    let mut texture = None;
    if tex != 0 {
        combiner.stages[0].comb_rgb = 1;
        combiner.stages[0].comb_a = 1;
        tex_matrix = rf32s(system, tex + 0x18);
        texture = texref(system, tex);
    }
    Draw {
        vertices: Vec::new(),
        indices: Vec::new(),
        world,
        view_proj,
        tex_matrix,
        color: c8.map(|c| c as f32 / 255.0),
        textures: [texture, None, None],
        combiner,
        blend: Blend::prim(((w0 & 0xFF) >> 5) as u8),
        depth_test,
        depth_func,
        depth_write: da[1] != 0,
        alpha_test: da[2] != 1,
        alpha_func,
        alpha_ref: da[3],
        cull,
        scissor,
        viewport,
        shaded: None,
        post: None,
        depth_offset,
    }
}

/// gfx-gaps: the prim quad index buffer's first `count` indices (the order
/// PicaHelper::DrawQuad draws a vertex array's quads in).
pub fn quad_indices(system: &mut System, t: &Tables, prim: u32, count: u32) -> Vec<u32> {
    let slot = r32(system, t.shape_index_slot);
    let ib_phys = r32(system, prim + slot * 4 + ctxoff::IB_PHYS);
    let ib = virt(system, ib_phys);
    (0..count)
        .map(|i| if t.index_u16 { r16(system, ib + 2 * i) as u32 } else { r8(system, ib + i) as u32 })
        .collect()
}


/// gfx-gaps: the state's polygon offset (w2 bits 26-31 index the floats at
/// prim context +0x2440, prim::Context::SetPolygonOffset 0x00416320).
/// UpdateState hands it to Pica::PolygonOffset (0x0012FDE4), which writes
/// the depth map's offset as `k1 - k2 * offset / k3` (its literals): the
/// picture's depth is `-z/w + that`, after clipping. The native 2D
/// pipeline maps depth as `-z/w` (scale -1, offset 0, which the scenes use
/// too) and adds this in the fragment shader (render.rs). Without it the
/// map's movement-range tiles, which lie on the terrain and are lifted by
/// their polygon offset, lost the depth test against it in most places.
fn polygon_offset(system: &mut System, prim: u32, w2: u32) -> f32 {
    let index = w2 >> 26;
    if index == 0 || crate::perf::var("FATES_GFX_POLYGON_OFFSET").is_ok_and(|v| v == "0") {
        return 0.0;
    }
    let table = r32(system, prim + 0x2440);
    let f = f32::from_bits(r32(system, table + index * 4));
    let k = |system: &mut System, at: u32| f32::from_bits(r32(system, zakuro_core::region::us(at))); // eur: USA literals
    let (k2, k1, k3) = (k(system, 0x0012_FE24), k(system, 0x0012_FE28), k(system, 0x0012_FE2C));
    if k3 == 0.0 || !f.is_finite() {
        return 0.0;
    }
    let offset = k1 - k2 * f / k3;
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if crate::perf::var_os("FATES_GFX_POLYGON_OFFSET_TRACE").is_some() && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("gfx-polygon-offset index {index} value {f} -> depth offset {offset} (k {k1} {k2} {k3})");
    }
    offset
}
