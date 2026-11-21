//! The native Vulkan renderer: its own instance and device, render targets
//! of any size, textures decoded once and cached, and pipelines made from
//! GLSL written for the game's prim layer (the game's prim.shbin vertex
//! program, recompiled by hand: world * position, view-projection, texture
//! matrix, colour = uniform * vertex colour) and TEV stage 0 as the prim
//! layer configures it.
//!
//! Nothing here emulates the PICA200: draws arrive as decoded engine-level
//! draws (prim::Draw) and go straight to Vulkan.

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;

use ash::vk;

use crate::prim::{Draw, Sampling, TexRef};

mod shaded;
// gfx-post: surfaces and post-effect draws
mod post;
// gfx-perf: texture uploads recorded with the frame
mod upload;
// msaa: the displays drawn multisampled (the options' Anti-Aliasing)
mod msaa;

const VERTEX_GLSL: &str = r#"#version 450
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color;
// gfx2d3: the texture rectangle of the vertex's atlas quad (edges.rs)
layout(location = 3) in vec4 a_rect;
// per draw, 26 vec4: world[3], view_proj[4], tex_matrix[3], color,
// 6 TEV stages x (packed sources/operands/combine, constant), buffer
// update bits, buffer colour, alpha test
layout(std430, set = 0, binding = 0) readonly buffer DrawParams {
    vec4 pv[];
};
layout(push_constant) uniform Push { uint index; } pc;
layout(location = 0) out vec4 v_color;
layout(location = 1) out vec2 v_uv;
layout(location = 2) flat out vec4 v_rect;
void main() {
    int b = int(pc.index) * 26;
    v_rect = a_rect;
    vec4 pos = vec4(a_pos, 1.0);
    vec4 w = vec4(dot(pv[b + 0], pos), dot(pv[b + 1], pos), dot(pv[b + 2], pos), 1.0);
    vec4 clip = vec4(dot(pv[b + 3], w), dot(pv[b + 4], w), dot(pv[b + 5], w), dot(pv[b + 6], w));
    vec4 t = vec4(a_uv, 0.0, 1.0);
    v_uv = vec2(dot(pv[b + 7], t), dot(pv[b + 8], t));
    v_color = pv[b + 10] * a_color;
    // the PICA's portrait framebuffer turned upright: the target is the
    // screen as it is seen, so no picture is ever rotated on the CPU
    gl_Position = vec4(-clip.y, clip.x, -clip.z, clip.w);
}
"#;

const FRAGMENT_GLSL: &str = r#"#version 450
layout(std430, set = 0, binding = 0) readonly buffer DrawParams {
    vec4 pv[];
};
layout(set = 0, binding = 1) uniform texture2D tex0;
layout(set = 0, binding = 2) uniform texture2D tex1;
layout(set = 0, binding = 3) uniform texture2D tex2;
layout(set = 0, binding = 4) uniform sampler samp0;
layout(set = 0, binding = 5) uniform sampler samp1;
layout(set = 0, binding = 6) uniform sampler samp2;
layout(push_constant) uniform Push { uint index; } pc;
layout(location = 0) in vec4 v_color;
layout(location = 1) in vec2 v_uv;
layout(location = 2) flat in vec4 v_rect; // gfx2d3
layout(location = 0) out vec4 o_color;

vec4 pick(int s, vec4 prim, vec4 t0, vec4 t1, vec4 t2, vec4 k, vec4 prev, vec4 buf) {
    if (s == 0) return prim;
    if (s == 1) return prim;
    if (s == 2) return vec4(0.0, 0.0, 0.0, 1.0);
    if (s == 3) return t0;
    if (s == 4) return t1;
    if (s == 5) return t2;
    if (s == 13) return buf;
    if (s == 14) return k;
    if (s == 15) return prev;
    return prim;
}

vec3 op_rgb(int op, vec4 c) {
    if (op == 0) return c.rgb;
    if (op == 1) return vec3(1.0) - c.rgb;
    if (op == 2) return vec3(c.a);
    if (op == 3) return vec3(1.0 - c.a);
    if (op == 4) return vec3(c.r);
    if (op == 5) return vec3(1.0 - c.r);
    if (op == 8) return vec3(c.g);
    if (op == 9) return vec3(1.0 - c.g);
    if (op == 12) return vec3(c.b);
    if (op == 13) return vec3(1.0 - c.b);
    return c.rgb;
}

float op_a(int op, vec4 c) {
    if (op == 0) return c.a;
    if (op == 1) return 1.0 - c.a;
    if (op == 2) return c.r;
    if (op == 3) return 1.0 - c.r;
    if (op == 4) return c.g;
    if (op == 5) return 1.0 - c.g;
    if (op == 6) return c.b;
    if (op == 7) return 1.0 - c.b;
    return c.a;
}

// gfx-bottom: sums saturate inside a stage, as the PICA's do (and the
// emulated GPU's raster.frag): (a + b) * c with a doubled vertex colour of
// 0x80 multiplied by 1.004, not 1, and portraits came out a step brighter
vec3 combine3(int f, vec3 a, vec3 b, vec3 c) {
    if (f == 0) return a;
    if (f == 1) return a * b;
    if (f == 2) return min(a + b, vec3(1.0));
    if (f == 3) return clamp(a + b - vec3(0.5), 0.0, 1.0);
    if (f == 4) return a * c + b * (vec3(1.0) - c);
    if (f == 5) return max(a - b, vec3(0.0));
    if (f == 6 || f == 7) return vec3(clamp(4.0 * dot(a - vec3(0.5), b - vec3(0.5)), 0.0, 1.0));
    if (f == 8) return min(a * b + c, vec3(1.0));
    if (f >= 9) return min(a + b, vec3(1.0)) * c;
    return a;
}

float combine1(int f, float a, float b, float c) {
    if (f == 0) return a;
    if (f == 1) return a * b;
    if (f == 2) return min(a + b, 1.0);
    if (f == 3) return clamp(a + b - 0.5, 0.0, 1.0);
    if (f == 4) return a * c + b * (1.0 - c);
    if (f == 5) return max(a - b, 0.0);
    if (f == 8) return min(a * b + c, 1.0);
    if (f >= 9) return min(a + b, 1.0) * c;
    return a;
}

bool compare(int f, float a, float r) {
    if (f == 0) return false;
    if (f == 1) return true;
    if (f == 2) return a == r;
    if (f == 3) return a != r;
    if (f == 4) return a < r;
    if (f == 5) return a <= r;
    if (f == 6) return a > r;
    if (f == 7) return a >= r;
    return true;
}

void main() {
    int b = int(pc.index) * 26;
    vec4 prim = clamp(v_color, 0.0, 1.0);
    // gfx2d3: an atlas quad's own cell only (edges.rs): unit 0's
    // coordinate kept half a texel inside the quad's texture rectangle,
    // where the console's pixel centres sample it, so linear filtering
    // takes nothing from the next cell (the dark lines along battle text
    // at 8x). An axis less than two texels long is left as it is.
    vec2 uv0 = v_uv;
    if ((int(pv[b + 23].w) & 1) != 0) {
        vec2 h = 0.5 / vec2(textureSize(sampler2D(tex0, samp0), 0));
        vec2 lo = v_rect.xy + h;
        vec2 hi = v_rect.zw - h;
        bvec2 on = greaterThanEqual(hi - lo, 2.0 * h);
        uv0 = vec2(on.x ? clamp(uv0.x, lo.x, hi.x) : uv0.x, on.y ? clamp(uv0.y, lo.y, hi.y) : uv0.y);
    }
    // gfx-2d: pixel art sampled sharp (update.z bit per unit): the texel
    // under the pixel, blended with its neighbour only over the one render
    // pixel their edge crosses
    int sharp = int(pv[b + 23].z);
    // gfx2d3: the level of detail from the unclamped coordinate
    vec4 t0 = textureGrad(sampler2D(tex0, samp0), uv0, dFdx(v_uv), dFdy(v_uv));
    // gfx2d3: above 1x, the bilinear filter written out (game textures,
    // one level; texture-pack pictures keep their own filtering):
    // - weighted by the texels' alpha, as premultiplied pictures filter,
    //   so a glyph fades to the picture behind instead of through the
    //   black of its transparent texels. Straight filtering spread that
    //   dark rim over half a texel, four pixels at 8x, where the
    //   console's 1x keeps it inside one pixel.
    // - at a cell's clamped edge, a texel past the edge gives the edge
    //   texel's colour and the smaller of the two alphas: no colour comes
    //   from the next cell (the dark line), but a glyph whose art runs to
    //   the edge of its cell still fades out where the atlas is empty past
    //   it, as it did, instead of ending in a hard cut four pixels thick.
    if ((int(pv[b + 23].w) & 2) != 0 && textureQueryLevels(sampler2D(tex0, samp0)) == 1) {
        ivec2 isz = textureSize(sampler2D(tex0, samp0), 0);
        ivec2 top = isz - 1;
        // the cell in texels; open edges and axes under two texels reach
        // the texture's own edges
        vec2 cl = clamp(v_rect.xy * vec2(isz), vec2(-1.0), vec2(isz) + 1.0);
        vec2 ch = clamp(v_rect.zw * vec2(isz), vec2(-1.0), vec2(isz) + 1.0);
        bvec2 on = greaterThanEqual(ch - cl, vec2(2.0));
        ivec2 c0 = clamp(ivec2(floor(cl + 1e-3)), ivec2(0), top);
        ivec2 c1 = clamp(ivec2(ceil(ch - 1e-3)) - 1, ivec2(0), top);
        c0 = ivec2(on.x ? c0.x : 0, on.y ? c0.y : 0);
        c1 = ivec2(on.x ? c1.x : top.x, on.y ? c1.y : top.y);
        vec2 tp = v_uv * vec2(isz) - 0.5;
        ivec2 i0 = ivec2(floor(tp));
        vec2 fr = tp - vec2(i0);
        vec4 tap[4];
        for (int k = 0; k < 4; k++) {
            ivec2 at = clamp(i0 + ivec2(k & 1, k >> 1), ivec2(0), top);
            ivec2 in_cell = clamp(at, c0, c1);
            vec4 s = texelFetch(sampler2D(tex0, samp0), in_cell, 0);
            if (at != in_cell) {
                s.a = min(s.a, texelFetch(sampler2D(tex0, samp0), at, 0).a);
            }
            tap[k] = vec4(s.rgb * s.a, s.a);
        }
        vec4 c = mix(mix(tap[0], tap[1], fr.x), mix(tap[2], tap[3], fr.x), fr.y);
        t0 = vec4(c.a > 0.0 ? c.rgb / c.a : vec3(0.0), c.a);
    }
    {
        // mapsprites: area-aware. The texels as square blocks, averaged
        // over the render pixel's footprint (fwidth, in texels): magnified,
        // a pixel covers one block or the edge between two, so blocks stay
        // hard-edged with a one-pixel edge; minified (the map zoomed out at
        // a low render scale), it averages every block it covers instead of
        // picking two, so nothing shimmers. Footprints are capped at 4
        // texels (5 fetches an axis).
        ivec2 isize = textureSize(sampler2D(tex0, samp0), 0);
        vec2 tp = uv0 * vec2(isize);
        vec2 w = clamp(fwidth(tp), vec2(1e-4), vec2(4.0));
        // a texture pack's picture (it comes with its smaller sizes) is
        // drawn as the pack made it, filtered through those sizes: it is
        // not the console's pixel art, and it can be minified far past
        // the 4-texel footprint
        if ((sharp & 1) != 0 && textureQueryLevels(sampler2D(tex0, samp0)) == 1) {
            // blended with their alpha (premultiplied), so a sprite's edge
            // fades out without a dark rim
            ivec2 hi = isize - 1;
            vec2 lo = tp - 0.5 * w;
            vec2 up = tp + 0.5 * w;
            ivec2 k0 = ivec2(floor(lo));
            ivec2 k1 = ivec2(floor(up - 1e-4));
            vec4 c = vec4(0.0);
            for (int j = 0; j < 5; j++) {
                int y = k0.y + j;
                if (y > k1.y) break;
                float wy = (min(up.y, float(y + 1)) - max(lo.y, float(y))) / w.y;
                for (int i = 0; i < 5; i++) {
                    int x = k0.x + i;
                    if (x > k1.x) break;
                    float wx = (min(up.x, float(x + 1)) - max(lo.x, float(x))) / w.x;
                    vec4 s = texelFetch(sampler2D(tex0, samp0), clamp(ivec2(x, y), ivec2(0), hi), 0);
                    c += vec4(s.rgb * s.a, s.a) * (wx * wy);
                }
            }
            t0 = vec4(c.a > 0.0 ? c.rgb / c.a : vec3(0.0), c.a);
        }
    }
    vec4 t1 = texture(sampler2D(tex1, samp1), v_uv);
    vec4 t2 = texture(sampler2D(tex2, samp2), v_uv);
    ivec4 upd = ivec4(pv[b + 23]);
    vec4 prev = prim;
    vec4 buf = vec4(0.0);
    vec4 next_buf = pv[b + 24];
    for (int i = 0; i < 6; i++) {
        ivec4 st = ivec4(pv[b + 11 + 2 * i]);
        vec4 k = pv[b + 12 + 2 * i];
        int srcs = st.x;
        int ops = st.y;
        int cs = st.z;
        vec4 s0 = pick(srcs & 15, prim, t0, t1, t2, k, prev, buf);
        vec4 s1 = pick((srcs >> 4) & 15, prim, t0, t1, t2, k, prev, buf);
        vec4 s2 = pick((srcs >> 8) & 15, prim, t0, t1, t2, k, prev, buf);
        vec4 a0 = pick((srcs >> 12) & 15, prim, t0, t1, t2, k, prev, buf);
        vec4 a1 = pick((srcs >> 16) & 15, prim, t0, t1, t2, k, prev, buf);
        vec4 a2 = pick((srcs >> 20) & 15, prim, t0, t1, t2, k, prev, buf);
        int crgb = cs & 15;
        int ca = (cs >> 4) & 15;
        vec3 rgb = combine3(crgb, op_rgb(ops & 15, s0), op_rgb((ops >> 4) & 15, s1), op_rgb((ops >> 8) & 15, s2));
        float a = combine1(ca, op_a((ops >> 12) & 15, a0), op_a((ops >> 16) & 15, a1), op_a((ops >> 20) & 15, a2));
        if (crgb == 7) {
            a = rgb.r;
        }
        float srgb = float(1 << ((cs >> 8) & 3));
        float sa = float(1 << ((cs >> 10) & 3));
        prev = clamp(vec4(rgb * srgb, a * sa), 0.0, 1.0);
        buf = next_buf;
        if (i < 4) {
            if (((upd.x >> i) & 1) != 0) {
                next_buf = vec4(prev.rgb, next_buf.a);
            }
            if (((upd.y >> i) & 1) != 0) {
                next_buf = vec4(next_buf.rgb, prev.a);
            }
        }
    }
    // gfx-bottom: the colour leaves as whole bytes, truncated, as the PICA
    // (and the emulated GPU's raster.frag) leave it; written as it was, the
    // UNORM target rounded it, one step brighter on half the pixels
    vec4 outc = floor(prev * 255.0) / 255.0;
    ivec4 at = ivec4(pv[b + 25]);
    if (at.x != 0) {
        float a8 = floor(outc.a * 255.0 + 0.5);
        if (!compare(at.y, a8, float(at.z))) {
            discard;
        }
    }
    o_color = outc;
    // gfx-gaps: the state's polygon offset, after clipping: map unit
    // sprites lie on the near plane (z = 0) with an offset, and an offset
    // folded into z clipped them away
    gl_FragDepth = clamp(gl_FragCoord.z + pv[b + 25].w, 0.0, 1.0);
}
"#;

/// The emulated picture under a hybrid frame, drawn from its GPU image
/// (`area`: where the screen lies in it, as texture coordinates).
const BLIT_VERTEX_GLSL: &str = r#"#version 450
layout(push_constant) uniform Push { vec4 area; } pc;
layout(location = 0) out vec2 v_uv;
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    // the emulated upright images run bottom-up
    v_uv = pc.area.xy + vec2(p.x, 1.0 - p.y) * pc.area.zw;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const BLIT_FRAGMENT_GLSL: &str = r#"#version 450
layout(set = 0, binding = 0) uniform texture2D base;
layout(set = 0, binding = 1) uniform sampler samp;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 o_color;
void main() {
    o_color = texture(sampler2D(base, samp), v_uv);
}
"#;

/// What a frame starts from.
pub enum Start<'a> {
    /// the game's clear colour
    Clear([f32; 4]),
    /// the display's previous native picture, kept in its target
    Keep,
    /// these upright RGBA pixels
    Pixels(&'a [u8]),
    /// the emulated picture's GPU image (in the general layout) and where
    /// the screen lies in it
    View(vk::ImageView, [f32; 4]),
}

/// Where a frame goes.
pub enum Finish {
    /// read back to the CPU
    Readback,
    /// copied to an image the presenter samples (general layout)
    Present,
    /// gfx-perf: left in the display's target (transfer-source layout),
    /// submitted without waiting; `read_target` reads it back when asked
    Keep,
}

pub enum Output {
    Pixels(Vec<u8>),
    /// the image view, its width and height
    Screen(vk::ImageView, u32, u32),
    /// gfx-perf: Finish::Keep
    Kept,
}

/// The per-draw parameters, std430, matching `Params` above.
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    world: [f32; 12],
    view_proj: [f32; 16],
    tex_matrix: [f32; 12],
    color: [f32; 4],
    /// per stage: packed sources, packed operands, combine and scale, 0;
    /// then the constant colour
    stages: [[f32; 8]; 6],
    update: [f32; 4],
    buffer: [f32; 4],
    alpha: [f32; 4],
}
const PARAMS_SIZE: usize = std::mem::size_of::<Params>();
/// gfx2d3: a 2D vertex on the GPU: prim's 24 bytes, then the texture
/// rectangle its quad is clamped to (edges.rs)
const VERTEX_STRIDE: usize = 40;

/// A texture decoded to RGBA8, rows in the order the game's texture
/// coordinates address them (row 0 at t = 0).
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// gfx-terrain: how many sizes `rgba` holds, the largest first, each
    /// half the last
    pub levels: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    blend: crate::prim::Blend,
    depth_test: bool,
    depth_func: u8,
    depth_write: bool,
    cull: u8,
    /// msaa: the samples the pipeline draws (1: the renderer's own pass)
    samples: u8,
}

struct Image {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct Target {
    width: u32,
    height: u32,
    color: Image,
    depth: Image,
    framebuffer: vk::Framebuffer,
}

struct Buffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
    ptr: *mut u8,
}

struct Texture {
    image: Image,
    used: u64,
    /// hdpacks: its size, sizes and bytes, for a texture pack's pictures
    /// (larger than the game's texture) and the GPU budget
    width: u32,
    levels: u32,
    bytes: u64,
}

pub struct Renderer {
    _entry: Option<ash::Entry>,
    /// the presenter's device when drawing on it (it owns the device)
    shared: Option<Arc<zakuro_gpu::SharedDevice>>,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    descriptor_pool: vk::DescriptorPool,
    vs: vk::ShaderModule,
    fs: vk::ShaderModule,
    pipelines: HashMap<PipeKey, vk::Pipeline>,
    samplers: HashMap<Sampling, vk::Sampler>,
    /// hdpacks: the 2D layer's samplers for pictures with smaller sizes
    hd_samplers: HashMap<Sampling, vk::Sampler>,
    textures: HashMap<(TexRef, u64), Texture>,
    white: Image,
    /// one target per display (0 top, 1 bottom); gfx-shadow: 2 offscreen
    /// (a shadow map, its portrait framebuffer width / scale wide)
    // gfx-hfr: 3 and 4 the pictures in between's targets (render_between)
    targets: [Option<Target>; 5],
    /// images handed to the presenter, a few per display in turn
    present: [Vec<Image>; 2],
    present_size: [(u32, u32); 2],
    present_next: [usize; 2],
    blit_set_layout: vk::DescriptorSetLayout,
    blit_layout: vk::PipelineLayout,
    blit_pipeline: vk::Pipeline,
    blit_modules: [vk::ShaderModule; 2],
    vertex: Option<Buffer>,
    index: Option<Buffer>,
    params: Option<Buffer>,
    staging: Option<Buffer>,
    frame: u64,
    pub name: String,
    /// env FATES_GFX_CULL=0 turns culling off (for checking the mapping).
    honor_cull: bool,
    /// the H3D pipeline (render/shaded.rs), made on first use
    shaded: Option<shaded::ShadedGpu>,
    /// gfx-post: surfaces and post-effect pipelines (render/post.rs)
    post: Option<post::PostGpu>,
    /// gfx-perf: pipelines kept between runs
    pub(crate) pipeline_disk: crate::cache::PipelineDisk,
    /// gfx-perf: the last submission was not waited for
    in_flight: bool,
    /// gfx-perf: texture uploads waiting for the frame's commands
    uploads: upload::Uploads,
    /// msaa: the multisampled scratch and its pipelines (render/msaa.rs)
    msaa: Option<msaa::MsaaGpu>,
    /// msaa: FATES_GFX_GPU_TIME's timestamps (render/msaa.rs)
    gpu_timer: Option<msaa::GpuTimer>,
    /// perf3: this renderer draws the pictures in between (hfr.rs), whose
    /// sample count msaa.rs may cap (between_samples)
    pub between: bool,
}

// SAFETY: the renderer is used from one thread at a time behind a mutex;
// the raw pointers are persistent mappings of its own buffers.
unsafe impl Send for Renderer {}

fn vkerr(what: &'static str) -> impl Fn(vk::Result) -> String {
    move |e| format!("{what}: {e:?}")
}

fn glsl_to_spirv(source: &str, stage: naga::ShaderStage) -> Result<Vec<u32>, String> {
    // a checkout with Windows line ends puts \r in the sources, which
    // naga's GLSL lexer trips on
    let source = source.replace('\r', "");
    let mut frontend = naga::front::glsl::Frontend::default();
    let module = frontend
        .parse(&naga::front::glsl::Options::from(stage), &source)
        .map_err(|e| format!("glsl: {e:?}"))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("validate: {e:?}"))?;
    let options = naga::back::spv::Options { lang_version: (1, 0), ..Default::default() };
    let pipeline = naga::back::spv::PipelineOptions { shader_stage: stage, entry_point: "main".into() };
    naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|e| format!("spv: {e:?}"))
}

impl Renderer {
    /// a renderer on the presenter's device, so its pictures are shown
    /// straight from the GPU.
    pub fn new_shared(shared: Arc<zakuro_gpu::SharedDevice>) -> Result<Renderer, String> {
        let instance = shared.instance().clone();
        let device = shared.device().clone();
        let physical = shared.physical();
        let props = unsafe { instance.get_physical_device_properties(physical) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy().into_owned();
        let (queue, family) = (shared.queue(), shared.family());
        unsafe { Self::build(None, Some(shared), instance, physical, device, queue, family, name) }
    }

    /// perf3: a renderer on the presenter's device that submits to its side
    /// queue (zakuro_gpu SharedDevice::side_queue), for the small pictures
    /// the game's thread reads back at once (shadow maps, icon captures):
    /// their wait is then their own, not behind the frames and the pictures
    /// in between on the first queue. Its pictures are only read back, never
    /// shown, so nothing crosses queues.
    pub fn new_side(shared: Arc<zakuro_gpu::SharedDevice>) -> Result<Renderer, String> {
        let queue = shared.side_queue().ok_or("no side queue")?;
        let instance = shared.instance().clone();
        let device = shared.device().clone();
        let physical = shared.physical();
        let props = unsafe { instance.get_physical_device_properties(physical) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy().into_owned();
        let family = shared.family();
        unsafe { Self::build(None, Some(shared), instance, physical, device, queue, family, name) }
    }

    /// a renderer on a device of its own (headless runs, or no shared device).
    pub fn new() -> Result<Renderer, String> {
        unsafe {
            let entry = ash::Entry::load().map_err(|e| format!("no Vulkan loader: {e}"))?;
            let app = vk::ApplicationInfo::default()
                .application_name(c"fates-gfx")
                .api_version(vk::make_api_version(0, 1, 1, 0));
            let instance = entry
                .create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)
                .map_err(vkerr("create instance"))?;
            let physicals = instance.enumerate_physical_devices().map_err(vkerr("enumerate devices"))?;
            let mut pick = None;
            for &p in &physicals {
                let props = instance.get_physical_device_properties(p);
                let families = instance.get_physical_device_queue_family_properties(p);
                if let Some(f) = families.iter().position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)) {
                    let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                    if pick.is_none() || discrete {
                        pick = Some((p, f as u32, props));
                    }
                    if discrete {
                        break;
                    }
                }
            }
            let (physical, family, props) = pick.ok_or("no Vulkan device draws")?;
            let name = CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy().into_owned();
            let priorities = [1.0f32];
            let queue_info = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
            // gfx2d3: anisotropic filtering, as the presenter's device has it
            let features = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(instance.get_physical_device_features(physical).sampler_anisotropy == vk::TRUE);
            // msaa: alpha-tested 3D shaded per sample, as on the presenter's device
            let features = features.sample_rate_shading(instance.get_physical_device_features(physical).sample_rate_shading == vk::TRUE);
            let device = instance
                .create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queue_info).enabled_features(&features), None)
                .map_err(vkerr("create device"))?;
            let queue = device.get_device_queue(family, 0);
            Self::build(Some(entry), None, instance, physical, device, queue, family, name)
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn build(
        entry: Option<ash::Entry>,
        shared: Option<Arc<zakuro_gpu::SharedDevice>>,
        instance: ash::Instance,
        physical: vk::PhysicalDevice,
        device: ash::Device,
        queue: vk::Queue,
        family: u32,
        name: String,
    ) -> Result<Renderer, String> {
        unsafe {
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(vkerr("command pool"))?;
            let commands = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1),
                )
                .map_err(vkerr("command buffer"))?[0];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(vkerr("fence"))?;

            let attachments = [
                vk::AttachmentDescription::default()
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
                vk::AttachmentDescription::default()
                    .format(vk::Format::D32_SFLOAT)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            ];
            let color_ref = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let depth_ref =
                vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            let subpass = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_ref)
                .depth_stencil_attachment(&depth_ref)];
            let render_pass = device
                .create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpass), None)
                .map_err(vkerr("render pass"))?;

            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(3)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(4)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(5)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(6)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let set_layout = device
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)
                .map_err(vkerr("set layout"))?;
            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(4)];
            let set_layouts = [set_layout];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&push),
                    None,
                )
                .map_err(vkerr("pipeline layout"))?;
            let sizes = [
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(4096 + 4 * 4096),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(3 * 4096),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLER).descriptor_count(3 * 4096),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::UNIFORM_BUFFER).descriptor_count(4096),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(3 * 4096),
            ];
            let descriptor_pool = device
                .create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(8192).pool_sizes(&sizes), None)
                .map_err(vkerr("descriptor pool"))?;

            let vs_code = glsl_to_spirv(VERTEX_GLSL, naga::ShaderStage::Vertex)?;
            let fs_code = glsl_to_spirv(FRAGMENT_GLSL, naga::ShaderStage::Fragment)?;
            let vs = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vs_code), None)
                .map_err(vkerr("vertex module"))?;
            let fs = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&fs_code), None)
                .map_err(vkerr("fragment module"))?;

            // the blit of the emulated picture under a hybrid frame
            let blit_bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let blit_set_layout = device
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&blit_bindings), None)
                .map_err(vkerr("blit set layout"))?;
            let blit_push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX).size(16)];
            let blit_set_layouts = [blit_set_layout];
            let blit_layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&blit_set_layouts).push_constant_ranges(&blit_push),
                    None,
                )
                .map_err(vkerr("blit layout"))?;
            let bvs = glsl_to_spirv(BLIT_VERTEX_GLSL, naga::ShaderStage::Vertex)?;
            let bfs = glsl_to_spirv(BLIT_FRAGMENT_GLSL, naga::ShaderStage::Fragment)?;
            let blit_modules = [
                device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&bvs), None).map_err(vkerr("blit vs"))?,
                device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&bfs), None).map_err(vkerr("blit fs"))?,
            ];
            let blit_pipeline = {
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(blit_modules[0]).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(blit_modules[1]).name(c"main"),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
                let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default()
                    .polygon_mode(vk::PolygonMode::FILL)
                    .cull_mode(vk::CullModeFlags::NONE)
                    .line_width(1.0);
                let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
                let depth = vk::PipelineDepthStencilStateCreateInfo::default();
                let attachment = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
                let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
                let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
                let info = [vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&assembly)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&multisample)
                    .depth_stencil_state(&depth)
                    .color_blend_state(&blend)
                    .dynamic_state(&dynamic)
                    .layout(blit_layout)
                    .render_pass(render_pass)];
                device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &info, None)
                    .map_err(|(_, e)| format!("blit pipeline: {e:?}"))?[0]
            };

            let pipeline_disk = crate::cache::PipelineDisk::new(&device); // gfx-perf
            let mut renderer = Renderer {
                _entry: entry,
                shared,
                instance,
                physical,
                device,
                queue,
                pool,
                commands,
                fence,
                render_pass,
                set_layout,
                layout,
                descriptor_pool,
                vs,
                fs,
                pipelines: HashMap::new(),
                samplers: HashMap::new(),
                hd_samplers: HashMap::new(), // hdpacks
                textures: HashMap::new(),
                white: Image { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
                targets: [None, None, None, None, None], // gfx-hfr
                present: [Vec::new(), Vec::new()],
                present_size: [(0, 0); 2],
                present_next: [0; 2],
                blit_set_layout,
                blit_layout,
                blit_pipeline,
                blit_modules,
                vertex: None,
                index: None,
                params: None,
                staging: None,
                frame: 0,
                name,
                honor_cull: std::env::var("FATES_GFX_CULL").map_or(true, |v| v != "0"),
                shaded: None,
                post: None,
                pipeline_disk, // gfx-perf
                in_flight: false,
                uploads: Default::default(),
                msaa: None, // msaa
                gpu_timer: None, // msaa
                between: false, // perf3
            };
            renderer.white = renderer.upload_image(1, 1, &[255, 255, 255, 255])?;
            renderer.upload_now()?; // gfx-perf
            Ok(renderer)
        }
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32, String> {
        let props = unsafe { self.instance.get_physical_device_memory_properties(self.physical) };
        (0..props.memory_type_count)
            .find(|&i| bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(flags))
            .ok_or_else(|| "no memory type".to_owned())
    }

    fn buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<Buffer, String> {
        unsafe {
            let buffer = self
                .device
                .create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage), None)
                .map_err(vkerr("buffer"))?;
            let req = self.device.get_buffer_memory_requirements(buffer);
            // the staging buffer is read back every frame: host-cached
            // memory reads at memory speed, write-combined memory does not
            let coherent = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let readback = usage.contains(vk::BufferUsageFlags::TRANSFER_DST);
            let ty = if readback {
                self.memory_type(req.memory_type_bits, coherent | vk::MemoryPropertyFlags::HOST_CACHED)
                    .or_else(|_| self.memory_type(req.memory_type_bits, coherent))?
            } else {
                self.memory_type(req.memory_type_bits, coherent)?
            };
            let memory = self
                .device
                .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None)
                .map_err(vkerr("buffer memory"))?;
            self.device.bind_buffer_memory(buffer, memory, 0).map_err(vkerr("bind buffer"))?;
            let ptr = self
                .device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(vkerr("map"))? as *mut u8;
            Ok(Buffer { buffer, memory, size, ptr })
        }
    }

    fn destroy_buffer(&self, b: Buffer) {
        unsafe {
            self.device.destroy_buffer(b.buffer, None);
            self.device.free_memory(b.memory, None);
        }
    }

    /// a buffer of at least `size` bytes in `slot`, made again when too small.
    fn ensure(&mut self, which: u8, size: u64) -> Result<(), String> {
        let usage = match which {
            0 => vk::BufferUsageFlags::VERTEX_BUFFER,
            1 => vk::BufferUsageFlags::INDEX_BUFFER,
            2 => vk::BufferUsageFlags::STORAGE_BUFFER,
            _ => vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST,
        };
        let slot = match which {
            0 => &mut self.vertex,
            1 => &mut self.index,
            2 => &mut self.params,
            _ => &mut self.staging,
        };
        if slot.as_ref().is_some_and(|b| b.size >= size) {
            return Ok(());
        }
        let old = slot.take();
        let size = size.max(64 * 1024).next_power_of_two();
        let new = self.buffer(size, usage)?;
        if let Some(old) = old {
            self.destroy_buffer(old);
        }
        match which {
            0 => self.vertex = Some(new),
            1 => self.index = Some(new),
            2 => self.params = Some(new),
            _ => self.staging = Some(new),
        }
        Ok(())
    }

    fn image(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<Image, String> {
        unsafe {
            let image = self
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D { width, height, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(usage)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(vkerr("image"))?;
            let req = self.device.get_image_memory_requirements(image);
            let ty = self.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
            let memory = self
                .device
                .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None)
                .map_err(vkerr("image memory"))?;
            self.device.bind_image_memory(image, memory, 0).map_err(vkerr("bind image"))?;
            let view = self
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: aspect,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        }),
                    None,
                )
                .map_err(vkerr("image view"))?;
            Ok(Image { image, memory, view })
        }
    }

    fn destroy_image(&self, image: &Image) {
        unsafe {
            self.device.destroy_image_view(image.view, None);
            self.device.destroy_image(image.image, None);
            self.device.free_memory(image.memory, None);
        }
    }

    /// gfx-perf: waits for a submission left in flight, before the command
    /// buffer, the buffers it read or the images it used are touched again.
    pub(crate) fn settle(&mut self) {
        if !self.in_flight {
            return;
        }
        self.in_flight = false;
        let _t = crate::perf::scope(crate::perf::C::Wait);
        unsafe {
            let _ = self.device.wait_for_fences(&[self.fence], true, u64::MAX);
            let _ = self.device.reset_fences(&[self.fence]);
        }
        self.images_free();
    }

    /// gfx-perf: ends and submits the commands without waiting for them.
    fn submit(&mut self) -> Result<(), String> {
        unsafe {
            self.device.end_command_buffer(self.commands).map_err(vkerr("end commands"))?;
            let cmds = [self.commands];
            let submits = zakuro_gpu::submit_lock(); // rtpace
            self.device
                .queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)
                .map_err(vkerr("submit"))?;
            drop(submits);
        }
        self.in_flight = true;
        Ok(())
    }

    /// gfx-hfr: the size of display `disp`'s presented images.
    pub fn present_size(&self, disp: usize) -> (u32, u32) {
        self.present_size[disp]
    }

    /// gfx-final: whether a view is one of this renderer's presented
    /// images still (a new size makes the ring again, and the old views
    /// are destroyed: a picture kept by its view must not be shown or
    /// drawn from after that)
    pub fn presents(&self, view: vk::ImageView) -> bool {
        self.present.iter().flatten().any(|i| i.view == view)
    }

    /// rtpace: the views of its presented images (see presents), for while
    /// the renderer is lent to the drawing thread.
    pub fn present_views(&self) -> Vec<vk::ImageView> {
        self.present.iter().flatten().map(|i| i.view).collect()
    }

    /// gfx-hfr: renders a picture in between into a target of its own
    /// (3 + disp), so the display's target keeps the frame a later frame
    /// drawn without a clear starts from.
    #[allow(clippy::too_many_arguments)]
    pub fn render_between(
        &mut self,
        disp: usize,
        width: u32,
        height: u32,
        scale: f32,
        start: Start,
        depth_clear: f32,
        draws: &[Draw],
        texture_keys: &[[Option<u64>; 3]],
        decoded: &HashMap<(TexRef, u64), Arc<Decoded>>,
        finish: Finish,
    ) -> Result<Output, String> {
        if disp > 1 || matches!(start, Start::Keep) {
            return Err("a picture in between starts from a picture".into());
        }
        self.targets.swap(disp, 3 + disp);
        let out = self.render(disp, width, height, scale, start, depth_clear, draws, texture_keys, decoded, finish);
        self.targets.swap(disp, 3 + disp);
        out
    }

    /// gfx-perf: a presented picture (Finish::Present, general layout) read
    /// back, by its view, while it is still one of the display's images.
    pub fn read_present(&mut self, disp: usize, view: vk::ImageView) -> Result<Vec<u8>, String> {
        let _t = crate::perf::scope(crate::perf::C::Readback);
        self.settle();
        let Some(image) = self.present[disp].iter().find(|i| i.view == view).map(|i| i.image) else { return Err("picture gone".into()) };
        let (width, height) = self.present_size[disp];
        self.read_image(image, width, height, vk::ImageLayout::GENERAL)
    }

    /// vein2: dev, a rectangle of a presented picture read back (FATES_GFX_SEQ
    /// in real time: the whole picture's readback held the presenter up)
    pub fn read_present_rect(&mut self, disp: usize, view: vk::ImageView, (x, y, w, h): (u32, u32, u32, u32)) -> Result<Vec<u8>, String> {
        let _t = crate::perf::scope(crate::perf::C::Readback);
        self.settle();
        let Some(image) = self.present[disp].iter().find(|i| i.view == view).map(|i| i.image) else { return Err("picture gone".into()) };
        let pixels = (w * h * 4) as u64;
        self.ensure(3, pixels)?;
        let sbuf = self.staging.as_ref().unwrap().buffer;
        self.begin()?;
        let region = copy_region(w, h, vk::ImageAspectFlags::COLOR).image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 });
        unsafe { self.device.cmd_copy_image_to_buffer(self.commands, image, vk::ImageLayout::GENERAL, sbuf, &[region]) };
        self.submit_and_wait()?;
        let mut out = vec![0u8; pixels as usize];
        unsafe { std::ptr::copy_nonoverlapping(self.staging.as_ref().unwrap().ptr, out.as_mut_ptr(), pixels as usize) };
        Ok(out)
    }

    /// gfx-perf: the picture a Finish::Keep render left in display `disp`'s
    /// target, read back.
    pub fn read_target(&mut self, disp: usize) -> Result<Vec<u8>, String> {
        let _t = crate::perf::scope(crate::perf::C::Readback);
        self.settle();
        let Some(target) = self.targets[disp].as_ref() else { return Err("no target".into()) };
        let (width, height, color) = (target.width, target.height, target.color.image);
        self.read_image(color, width, height, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
    }

    fn read_image(&mut self, color: vk::Image, width: u32, height: u32, layout: vk::ImageLayout) -> Result<Vec<u8>, String> {
        let pixels = (width * height * 4) as u64;
        self.ensure(3, pixels)?;
        let sbuf = self.staging.as_ref().unwrap().buffer;
        self.begin()?;
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                self.commands,
                color,
                layout,
                sbuf,
                &[copy_region(width, height, vk::ImageAspectFlags::COLOR)],
            );
        }
        self.submit_and_wait()?;
        let mut out = vec![0u8; pixels as usize];
        unsafe { std::ptr::copy_nonoverlapping(self.staging.as_ref().unwrap().ptr, out.as_mut_ptr(), pixels as usize) };
        Ok(out)
    }

    fn begin(&self) -> Result<(), String> {
        unsafe {
            self.device
                .reset_command_buffer(self.commands, vk::CommandBufferResetFlags::empty())
                .map_err(vkerr("reset commands"))?;
            self.device
                .begin_command_buffer(
                    self.commands,
                    &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(vkerr("begin commands"))
        }
    }

    fn submit_and_wait(&self) -> Result<(), String> {
        unsafe {
            self.device.end_command_buffer(self.commands).map_err(vkerr("end commands"))?;
            let cmds = [self.commands];
            let submits = zakuro_gpu::submit_lock(); // rtpace
            self.device
                .queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)
                .map_err(vkerr("submit"))?;
            drop(submits);
            self.device.wait_for_fences(&[self.fence], true, u64::MAX).map_err(vkerr("wait"))?;
            self.device.reset_fences(&[self.fence]).map_err(vkerr("reset fence"))
        }
    }

    fn barrier(
        &self,
        image: vk::Image,
        aspect: vk::ImageAspectFlags,
        from: vk::ImageLayout,
        to: vk::ImageLayout,
    ) {
        let b = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(from)
            .new_layout(to)
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.commands,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[b],
            );
        }
    }

    /// an RGBA8 sampled image with the given texels.
    fn upload_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Image, String> {
        let _t = crate::perf::scope(crate::perf::C::Upload); // gfx-perf
        // gfx-perf: recorded with the frame's commands, not waited for on its
        // own (FATES_GFX_UPLOAD_WAIT=1: as before)
        if !upload_wait() {
            return self.upload_deferred(width, height, rgba);
        }
        self.settle(); // gfx-perf: the staging buffer may be in use
        let image = self.image(
            width,
            height,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        self.ensure(3, rgba.len() as u64)?;
        let staging = self.staging.as_ref().unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), staging.ptr, rgba.len());
        }
        let sb = staging.buffer;
        self.begin()?;
        self.barrier(image.image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                self.commands,
                sb,
                image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy_region(width, height, vk::ImageAspectFlags::COLOR)],
            );
        }
        self.barrier(
            image.image,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        self.submit_and_wait()?;
        Ok(image)
    }

    fn sampler(&mut self, s: Sampling) -> Result<vk::Sampler, String> {
        if let Some(&x) = self.samplers.get(&s) {
            return Ok(x);
        }
        // gfx-2d: s and t wrap apart, as the PICA does
        let address = |wrap: u8| match wrap {
            0 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            1 => vk::SamplerAddressMode::CLAMP_TO_BORDER,
            2 => vk::SamplerAddressMode::REPEAT,
            _ => vk::SamplerAddressMode::MIRRORED_REPEAT,
        };
        let (mode, mode_t) = (address(s.wrap), address(s.wrap_t));
        let f = |linear: bool| if linear { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
        let sampler = unsafe {
            self.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(f(s.linear_mag))
                    .min_filter(f(s.linear_min))
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(mode)
                    .address_mode_v(mode_t)
                    .address_mode_w(mode)
                    .border_color(vk::BorderColor::FLOAT_TRANSPARENT_BLACK)
                    .max_lod(0.0),
                None,
            )
        }
        .map_err(vkerr("sampler"))?;
        self.samplers.insert(s, sampler);
        Ok(sampler)
    }

    /// hdpacks: a 2D sampler for a picture with its smaller sizes: as
    /// `sampler`, going between the sizes when minified (a 4x pack's
    /// picture at a low render scale), the game's own filter otherwise.
    fn hd_sampler(&mut self, s: Sampling) -> Result<vk::Sampler, String> {
        if let Some(&x) = self.hd_samplers.get(&s) {
            return Ok(x);
        }
        let address = |wrap: u8| match wrap {
            0 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            1 => vk::SamplerAddressMode::CLAMP_TO_BORDER,
            2 => vk::SamplerAddressMode::REPEAT,
            _ => vk::SamplerAddressMode::MIRRORED_REPEAT,
        };
        let f = |linear: bool| if linear { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
        let linear = s.linear_mag || s.linear_min;
        let sampler = unsafe {
            self.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(f(s.linear_mag))
                    .min_filter(f(linear))
                    .mipmap_mode(if linear { vk::SamplerMipmapMode::LINEAR } else { vk::SamplerMipmapMode::NEAREST })
                    .address_mode_u(address(s.wrap))
                    .address_mode_v(address(s.wrap_t))
                    .address_mode_w(address(s.wrap))
                    .border_color(vk::BorderColor::FLOAT_TRANSPARENT_BLACK)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .map_err(vkerr("sampler"))?;
        self.hd_samplers.insert(s, sampler);
        Ok(sampler)
    }

    fn pipeline(&mut self, key: PipeKey) -> Result<vk::Pipeline, String> {
        if let Some(&p) = self.pipelines.get(&key) {
            return Ok(p);
        }
        let _t = crate::perf::scope(crate::perf::C::Pipeline); // gfx-perf
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(self.vs).name(c"main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(self.fs).name(c"main"),
        ];
        // gfx2d3: 40-byte vertices, prim's 24 and the quad's texture rectangle (edges.rs)
        let binding = [vk::VertexInputBindingDescription::default().binding(0).stride(VERTEX_STRIDE as u32).input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default().location(0).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
            vk::VertexInputAttributeDescription::default().location(1).binding(0).format(vk::Format::R32G32_SFLOAT).offset(12),
            vk::VertexInputAttributeDescription::default().location(2).binding(0).format(vk::Format::R8G8B8A8_UNORM).offset(20),
            vk::VertexInputAttributeDescription::default().location(3).binding(0).format(vk::Format::R32G32B32A32_SFLOAT).offset(24),
        ];
        let vertex_input =
            vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&binding).vertex_attribute_descriptions(&attrs);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let cull = if !self.honor_cull {
            vk::CullModeFlags::NONE
        } else {
            match key.cull {
                1 => vk::CullModeFlags::FRONT,
                2 => vk::CullModeFlags::BACK,
                _ => vk::CullModeFlags::NONE,
            }
        };
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(cull)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        // msaa: at the key's samples, into their pass
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::from_raw((key.samples as u32).max(1)));
        let pass = if key.samples > 1 { self.msaa_pass(key.samples as u32)? } else { self.render_pass };
        let compare = |f: u8| match f {
            0 => vk::CompareOp::NEVER,
            1 => vk::CompareOp::ALWAYS,
            2 => vk::CompareOp::EQUAL,
            3 => vk::CompareOp::NOT_EQUAL,
            4 => vk::CompareOp::LESS,
            5 => vk::CompareOp::LESS_OR_EQUAL,
            6 => vk::CompareOp::GREATER,
            _ => vk::CompareOp::GREATER_OR_EQUAL,
        };
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(key.depth_test)
            .depth_write_enable(key.depth_test && key.depth_write)
            .depth_compare_op(compare(key.depth_func));
        // prim::Context::UpdateBlend, PICA factors/equations as Vulkan's
        let factor = |f: u8| match f {
            0 => vk::BlendFactor::ZERO,
            1 => vk::BlendFactor::ONE,
            2 => vk::BlendFactor::SRC_COLOR,
            3 => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
            4 => vk::BlendFactor::DST_COLOR,
            5 => vk::BlendFactor::ONE_MINUS_DST_COLOR,
            6 => vk::BlendFactor::SRC_ALPHA,
            7 => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            8 => vk::BlendFactor::DST_ALPHA,
            9 => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
            _ => vk::BlendFactor::ONE,
        };
        let eq = |e: u8| match e {
            1 => vk::BlendOp::SUBTRACT,
            2 => vk::BlendOp::REVERSE_SUBTRACT,
            3 => vk::BlendOp::MIN,
            4 => vk::BlendOp::MAX,
            _ => vk::BlendOp::ADD,
        };
        let b = key.blend;
        let attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(b.enable)
            .color_blend_op(eq(b.eq[0]))
            .alpha_blend_op(eq(b.eq[1]))
            .src_color_blend_factor(factor(b.func[0]))
            .dst_color_blend_factor(factor(b.func[1]))
            .src_alpha_blend_factor(factor(b.func[2]))
            .dst_alpha_blend_factor(factor(b.func[3]))
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = [vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(self.layout)
            .render_pass(pass)]; // msaa
        // smooth: through the logged maker (kind 0: 2D)
        let p = crate::cache::make_pipeline(&self.device, self.pipeline_disk.cache, info[0], 0.0) // gfx-perf: kept on disk
            .map_err(|e| format!("pipeline: {e:?}"))?;
        self.pipeline_disk.created(&self.device);
        self.pipelines.insert(key, p);
        Ok(p)
    }

    fn ensure_target(&mut self, disp: usize, width: u32, height: u32) -> Result<(), String> {
        if self.targets[disp].as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return Ok(());
        }
        let _smooth = Realloc::new(1, width, height); // smooth: in the timing log
        if let Some(t) = self.targets[disp].take() {
            unsafe {
                let _ = zakuro_gpu::wait_idle(&self.device); // smooth: keeps off the present thread
                self.device.destroy_framebuffer(t.framebuffer, None);
            }
            self.destroy_image(&t.color);
            self.destroy_image(&t.depth);
        }
        let color = self.image(
            width,
            height,
            vk::Format::R8G8B8A8_UNORM,
            // gfx-post: SAMPLED, a post effect captures it
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            vk::ImageAspectFlags::COLOR,
        )?;
        let depth = self.image(
            width,
            height,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::DEPTH,
        )?;
        let views = [color.view, depth.view];
        let framebuffer = unsafe {
            self.device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(self.render_pass)
                    .attachments(&views)
                    .width(width)
                    .height(height)
                    .layers(1),
                None,
            )
        }
        .map_err(vkerr("framebuffer"))?;
        self.targets[disp] = Some(Target { width, height, color, depth, framebuffer });
        Ok(())
    }

    /// the next of the display's presented images, made again at a new size.
    fn present_image(&mut self, disp: usize, width: u32, height: u32) -> Result<(vk::Image, vk::ImageView), String> {
        if self.present_size[disp] != (width, height) || self.present[disp].is_empty() {
            let _smooth = Realloc::new(2, width, height); // smooth: in the timing log
            let _ = zakuro_gpu::wait_idle(&self.device); // smooth: keeps off the present thread
            for image in std::mem::take(&mut self.present[disp]) {
                self.destroy_image(&image);
            }
            for _ in 0..8 { // gfx-hfr: 8, room for the pictures in between
                let image = self.image(
                    width,
                    height,
                    vk::Format::R8G8B8A8_UNORM,
                    // gfx-perf: TRANSFER_SRC, read back when a headless run asks
                    vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC,
                    vk::ImageAspectFlags::COLOR,
                )?;
                self.present[disp].push(image);
            }
            self.present_size[disp] = (width, height);
        }
        let n = self.present_next[disp] % self.present[disp].len();
        self.present_next[disp] = n + 1;
        let image = &self.present[disp][n];
        Ok((image.image, image.view))
    }

    /// the texture for a draw, uploaded the first time its texels are seen.
    fn texture_view(&mut self, key: (TexRef, u64), decoded: Option<&Arc<Decoded>>) -> Result<vk::ImageView, String> {
        if let Some(t) = self.textures.get_mut(&key) {
            t.used = self.frame;
            return Ok(t.image.view);
        }
        let Some(d) = decoded else { return Ok(self.white.view) };
        // gfx-terrain: a texture with its smaller sizes
        let image = if d.levels > 1 { self.upload_levels(d.width, d.height, d.levels, &d.rgba)? } else { self.upload_image(d.width, d.height, &d.rgba)? };
        let view = image.view;
        // hdpacks: sizes kept for the samplers and the GPU budget
        self.textures.insert(key, Texture { image, used: self.frame, width: d.width, levels: d.levels.max(1), bytes: d.rgba.len() as u64 });
        self.moflexhd_keep_few(key); // moflexhd
        Ok(view)
    }

    /// moflexhd: a movie's picture (a texture the YUV-to-RGB converter
    /// writes) keeps its last few pictures on the GPU; the older ones go
    /// (destroyed once no submission uses them), not 600 frames later, an
    /// image of up to 4 MB each.
    fn moflexhd_keep_few(&mut self, key: (TexRef, u64)) {
        let tex = key.0;
        if !crate::moflexhd::few_kept(tex) {
            return;
        }
        let mut versions: Vec<((TexRef, u64), u64)> = self.textures.iter().filter(|(k, _)| k.0 == tex).map(|(k, t)| (*k, t.used)).collect();
        if versions.len() <= crate::moflexhd::KEPT_VERSIONS {
            return;
        }
        versions.sort_by_key(|&(_, used)| std::cmp::Reverse(used));
        let frame = self.frame;
        for (k, used) in versions.into_iter().skip(crate::moflexhd::KEPT_VERSIONS) {
            if k != key && used + 4 <= frame {
                self.forget_texture(k);
            }
        }
    }

    /// gfx-shadow: drops one texture now (a shadow map no frame draws with
    /// any more). Every render waits for its commands, so none uses it.
    pub fn forget_texture(&mut self, key: (TexRef, u64)) {
        if let Some(t) = self.textures.remove(&key) {
            // gfx-perf: destroyed once the GPU is done with it, not waited for
            self.upload_cancel(t.image.image);
            self.retire_image(t.image);
        }
    }

    /// drops textures no frame has used for a while.
    fn evict(&mut self) {
        let _smooth = Realloc::new(3, 0, 0); // smooth: in the timing log
        let frame = self.frame;
        let mut old: Vec<_> = self.textures.iter().filter(|(_, t)| frame - t.used > 600).map(|(k, _)| *k).collect();
        // hdpacks: past the GPU budget (texture packs' pictures are large),
        // the least recently used go too, never one of the last frames'
        let budget = crate::hd::gpu_budget();
        let total: u64 = self.textures.iter().filter(|(k, _)| !old.contains(k)).map(|(_, t)| t.bytes).sum();
        if total > budget {
            let mut by_use: Vec<_> = self.textures.iter().filter(|(k, t)| !old.contains(k) && frame - t.used > 2).map(|(k, t)| (t.used, t.bytes, *k)).collect();
            by_use.sort_by_key(|(used, _, _)| *used);
            let mut left = total;
            for (_, bytes, k) in by_use {
                if left <= budget * 3 / 4 {
                    break;
                }
                left -= bytes;
                old.push(k);
            }
        }
        if old.is_empty() {
            return;
        }
        // smooth: no wait for the GPU, which last used them 600 frames ago
        // (each frame settles the one before), and destroyed off the
        // frame's path (cache::reap)
        let images = old.into_iter().filter_map(|k| self.textures.remove(&k)).map(|t| (t.image.view, t.image.image, t.image.memory)).collect();
        crate::cache::reap(&self.device, images);
    }

    /// Draws a frame of display `disp` into its `width` x `height` upright
    /// target (`scale` framebuffer units to pixels), starting as `start`
    /// says, with the depth buffer cleared to `depth_clear`, and reads it
    /// back or hands it to the presenter.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        disp: usize,
        width: u32,
        height: u32,
        scale: f32,
        start: Start,
        depth_clear: f32,
        draws: &[Draw],
        texture_keys: &[[Option<u64>; 3]],
        decoded: &HashMap<(TexRef, u64), Arc<Decoded>>,
        finish: Finish,
    ) -> Result<Output, String> {
        let _t = crate::perf::scope(crate::perf::C::Render); // gfx-perf
        let t_prepare = crate::perf::scope(crate::perf::C::RenderPrepare); // gfx-perf
        self.settle(); // gfx-perf: the last frame's buffers are free again
        self.uploads.recording = false;
        self.uploads_free();
        self.frame += 1;
        self.ensure_target(disp, width, height)?;
        let pixels = (width * height * 4) as u64;
        self.ensure(3, pixels)?;
        // msaa: the samples this frame is drawn with (1: exactly as before)
        let samples = self.msaa_samples(disp, scale);
        let samples = if samples > 1 && self.msaa_ensure(width, height, samples).map_err(|e| eprintln!("gfx-msaa: {e}; drawn with one sample")).is_err() { 1 } else { samples };
        if crate::perf::var_os("FATES_GFX_MSAA_TRACE").is_some() {
            let s = match start { Start::Clear(_) => "clear", Start::Keep => "keep", Start::Pixels(_) => "pixels", Start::View(..) => "view" };
            let f = match finish { Finish::Readback => "readback", Finish::Present => "present", Finish::Keep => "keep" };
            eprintln!("gfx-msaa-trace: {} frame {} disp {disp} {width}x{height} samples {samples} start {s} finish {f} draws {} shaded {} posts {}", self.name, self.frame, draws.len(), draws.iter().filter(|d| d.shaded.is_some()).count(), draws.iter().filter(|d| d.post.is_some()).count());
        }

        // geometry and parameters for every draw
        let vbytes: usize = draws.iter().map(|d| d.vertices.len() / 24 * VERTEX_STRIDE).sum(); // gfx2d3
        let icount: usize = draws.iter().map(|d| d.indices.len()).sum();
        self.ensure(0, vbytes.max(VERTEX_STRIDE) as u64)?;
        self.ensure(1, (icount.max(1) * 4) as u64)?;
        self.ensure(2, (draws.len().max(1) * PARAMS_SIZE) as u64)?;
        let mut ranges = Vec::with_capacity(draws.len());
        {
            let (vb, ib, pb) = (self.vertex.as_ref().unwrap(), self.index.as_ref().unwrap(), self.params.as_ref().unwrap());
            let (mut vo, mut io) = (0usize, 0usize);
            for (n, d) in draws.iter().enumerate() {
                let base_vertex = (vo / VERTEX_STRIDE) as u32;
                // gfx2d3: unit 0's atlas quads clamped to their cells
                let clamp = edge_clamp(d, scale);
                let rects = if clamp { crate::edges::rects(&d.vertices, &d.indices, &d.tex_matrix) } else { Vec::new() };
                unsafe {
                    for (k, v) in d.vertices.chunks_exact(24).enumerate() {
                        let at = vb.ptr.add(vo + k * VERTEX_STRIDE);
                        std::ptr::copy_nonoverlapping(v.as_ptr(), at, 24);
                        let r = rects.get(k).copied().unwrap_or(crate::edges::OPEN);
                        std::ptr::copy_nonoverlapping(r.as_ptr() as *const u8, at.add(24), 16);
                    }
                    let idx = ib.ptr.add(io * 4) as *mut u32;
                    for (k, &i) in d.indices.iter().enumerate() {
                        *idx.add(k) = i + base_vertex;
                    }
                    let p = Params {
                        world: d.world,
                        view_proj: d.view_proj,
                        tex_matrix: d.tex_matrix,
                        color: d.color,
                        stages: d.combiner.stages.map(|st| {
                            let nib = |v: [u8; 3]| v[0] as u32 | (v[1] as u32) << 4 | (v[2] as u32) << 8;
                            let srcs = nib(st.src_rgb) | nib(st.src_a) << 12;
                            let ops = nib(st.op_rgb) | nib(st.op_a) << 12;
                            let cs = st.comb_rgb as u32 | (st.comb_a as u32) << 4 | (st.scale_rgb as u32 & 3) << 8 | (st.scale_a as u32 & 3) << 10;
                            let k = st.konst.map(|c| c as f32 / 255.0);
                            [srcs as f32, ops as f32, cs as f32, 0.0, k[0], k[1], k[2], k[3]]
                        }),
                        // gfx-2d: z, the units sampled sharp (bits 0-2)
                        // mapsprites: above 1x; at 1x too where the game asked
                        // for linear (the map zoomed out: the area-aware sampling
                        // then averages the blocks a pixel covers). At 1x the
                        // nearest the game asks for at the nearest zoom stays
                        // nearest: one texel a pixel, the console's picture, where
                        // a box over a half-texel offset would blur
                        update: [d.combiner.update_rgb as f32, d.combiner.update_a as f32, d.textures.iter().enumerate().map(|(u, t)| if t.is_some_and(|t| t.1.sharp && (scale > 1.5 || t.1.linear_mag)) { 1 << u } else { 0 }).sum::<u32>() as f32, (clamp as u32 | ((clamp && scale > 1.5 && d.textures[0].is_some_and(|t| t.1.linear_mag)) as u32) << 1) as f32], // gfx2d3: w bit 0 the edge clamp, bit 1 alpha-weighted bilinear
                        buffer: d.combiner.buffer.map(|c| c as f32 / 255.0),
                        alpha: [d.alpha_test as i32 as f32, d.alpha_func as f32, d.alpha_ref as f32, d.depth_offset], // gfx-gaps: w the polygon offset
                    };
                    std::ptr::copy_nonoverlapping(
                        &p as *const Params as *const u8,
                        pb.ptr.add(n * PARAMS_SIZE),
                        PARAMS_SIZE,
                    );
                }
                ranges.push((io as u32, d.indices.len() as u32));
                vo += d.vertices.len() / 24 * VERTEX_STRIDE; // gfx2d3
                io += d.indices.len();
            }
        }

        // pipelines, textures and descriptor sets, before recording
        let mut prepared = Vec::with_capacity(draws.len());
        unsafe {
            self.device
                .reset_descriptor_pool(self.descriptor_pool, vk::DescriptorPoolResetFlags::empty())
                .map_err(vkerr("reset descriptors"))?;
        }
        let shaded_draws: Vec<&crate::scene::Shaded> = draws.iter().filter_map(|d| d.shaded.as_deref()).collect();
        let mut shaded_prepared: Vec<Option<shaded::Prepared>> = Vec::with_capacity(draws.len());
        if !shaded_draws.is_empty() {
            self.shaded_reserve(&shaded_draws)?;
        }
        // gfx-post: room for the post effects' draws, their guest textures
        let post_blocks: Vec<&crate::post::Block> = draws.iter().filter_map(|d| d.post.as_deref()).collect();
        if !post_blocks.is_empty() {
            self.post_reserve(&post_blocks, &mut |r, tex, hash| r.texture_view((tex, hash), decoded.get(&(tex, hash))).map(|_| ()))?;
        }
        let mut placed = HashMap::new();
        let mut sets: HashMap<([vk::ImageView; 3], [vk::Sampler; 3]), vk::DescriptorSet> = HashMap::new();
        for (n, d) in draws.iter().enumerate() {
            // gfx-post: drawn by post_run where it stands
            if d.post.is_some() {
                shaded_prepared.push(None);
                prepared.push((vk::Pipeline::null(), vk::DescriptorSet::null()));
                continue;
            }
            if let Some(s) = d.shaded.as_deref() {
                let _t = crate::perf::scope(crate::perf::C::RenderShaded); // gfx-perf
                shaded_prepared.push(Some(self.shaded_prepare(s, &texture_keys[n], decoded, &mut placed, samples)?)); // msaa
                prepared.push((vk::Pipeline::null(), vk::DescriptorSet::null()));
                continue;
            }
            shaded_prepared.push(None);
            let key = PipeKey {
                blend: d.blend,
                depth_test: d.depth_test,
                depth_func: d.depth_func,
                depth_write: d.depth_write,
                cull: d.cull,
                samples: samples as u8, // msaa
            };
            let pipeline = self.pipeline(key)?;
            let mut views = [vk::ImageView::null(); 3];
            let mut samplers = [vk::Sampler::null(); 3];
            for unit in 0..3 {
                let (view, sampler) = match (&d.textures[unit], texture_keys[n][unit]) {
                    (Some((tex, sampling)), Some(hash)) => {
                        let k = (*tex, hash);
                        let view = self.texture_view(k, decoded.get(&k))?;
                        // hdpacks: a pack's picture with its smaller sizes
                        match self.textures.get(&k).filter(|t| t.levels > 1 && t.width > tex.width) {
                            Some(_) => (view, self.hd_sampler(*sampling)?),
                            None => (view, self.sampler(*sampling)?),
                        }
                    }
                    _ => (self.white.view, self.sampler(Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: false, linear_min: false })?),
                };
                views[unit] = view;
                samplers[unit] = sampler;
            }
            let set = match sets.get(&(views, samplers)) {
                Some(&s) => s,
                None => {
                    let layouts = [self.set_layout];
                    let s = unsafe {
                        self.device.allocate_descriptor_sets(
                            &vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.descriptor_pool).set_layouts(&layouts),
                        )
                    }
                    .map_err(vkerr("descriptor set"))?[0];
                    let buffer_info = [vk::DescriptorBufferInfo::default()
                        .buffer(self.params.as_ref().unwrap().buffer)
                        .offset(0)
                        .range(vk::WHOLE_SIZE)];
                    let image_infos: Vec<[vk::DescriptorImageInfo; 1]> = views
                        .iter()
                        .map(|&v| [vk::DescriptorImageInfo::default().image_view(v).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)])
                        .collect();
                    let sampler_infos: Vec<[vk::DescriptorImageInfo; 1]> =
                        samplers.iter().map(|&x| [vk::DescriptorImageInfo::default().sampler(x)]).collect();
                    let mut writes = vec![vk::WriteDescriptorSet::default()
                        .dst_set(s)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&buffer_info)];
                    for unit in 0..3 {
                        writes.push(
                            vk::WriteDescriptorSet::default()
                                .dst_set(s)
                                .dst_binding(1 + unit as u32)
                                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                                .image_info(&image_infos[unit]),
                        );
                        writes.push(
                            vk::WriteDescriptorSet::default()
                                .dst_set(s)
                                .dst_binding(4 + unit as u32)
                                .descriptor_type(vk::DescriptorType::SAMPLER)
                                .image_info(&sampler_infos[unit]),
                        );
                    }
                    unsafe { self.device.update_descriptor_sets(&writes, &[]) };
                    sets.insert((views, samplers), s);
                    s
                }
            };
            prepared.push((pipeline, set));
        }
        // the emulated picture's image, drawn first
        let blit = match start {
            Start::View(view, area) => {
                let layouts = [self.blit_set_layout];
                let s = unsafe {
                    self.device.allocate_descriptor_sets(
                        &vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.descriptor_pool).set_layouts(&layouts),
                    )
                }
                .map_err(vkerr("blit set"))?[0];
                let nearest = self.sampler(Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: false, linear_min: false })?;
                let image_info = [vk::DescriptorImageInfo::default().image_view(view).image_layout(vk::ImageLayout::GENERAL)];
                let sampler_info = [vk::DescriptorImageInfo::default().sampler(nearest)];
                let writes = [
                    vk::WriteDescriptorSet::default()
                        .dst_set(s)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(s)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&sampler_info),
                ];
                unsafe { self.device.update_descriptor_sets(&writes, &[]) };
                Some((s, area))
            }
            _ => None,
        };

        // the starting picture: the emulated one under a native suffix, or
        // the game's clear colour
        let target = self.targets[disp].as_ref().unwrap();
        let (color, depth, framebuffer) = (target.color.image, target.depth.image, target.framebuffer);
        let color_view = target.color.view; // msaa
        let start_clear = match start { Start::Clear(rgba) => Some(rgba), _ => None }; // msaa
        let start_keep = matches!(start, Start::Keep); // msaa
        let staging = self.staging.as_ref().unwrap();
        let sbuf = staging.buffer;
        if let Start::Pixels(base) = start {
            unsafe { std::ptr::copy_nonoverlapping(base.as_ptr(), staging.ptr, (pixels as usize).min(base.len())) };
        }
        drop(t_prepare); // gfx-perf
        let t_record = crate::perf::scope(crate::perf::C::RenderRecord); // gfx-perf
        self.begin()?;
        // gfx-perf: this frame's new textures first
        self.record_uploads();
        self.uploads.recording = true;
        self.gpu_time_start(); // msaa: FATES_GFX_GPU_TIME (the drawing, after the uploads)
        // a frame that keeps the previous picture finds it where the last
        // frame left it, read for a copy
        let from = if matches!(start, Start::Keep) { vk::ImageLayout::TRANSFER_SRC_OPTIMAL } else { vk::ImageLayout::UNDEFINED };
        self.barrier(color, vk::ImageAspectFlags::COLOR, from, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        self.barrier(depth, vk::ImageAspectFlags::DEPTH, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let range = |aspect| vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        unsafe {
            match start {
                Start::Pixels(_) => self.device.cmd_copy_buffer_to_image(
                    self.commands,
                    sbuf,
                    color,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[copy_region(width, height, vk::ImageAspectFlags::COLOR)],
                ),
                Start::Clear(rgba) => self.device.cmd_clear_color_image(
                    self.commands,
                    color,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: rgba },
                    &[range(vk::ImageAspectFlags::COLOR)],
                ),
                Start::Keep | Start::View(..) => {}
            }
            self.device.cmd_clear_depth_stencil_image(
                self.commands,
                depth,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearDepthStencilValue { depth: depth_clear, stencil: 0 },
                &[range(vk::ImageAspectFlags::DEPTH)],
            );
        }
        self.barrier(
            color,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
        self.barrier(
            depth,
            vk::ImageAspectFlags::DEPTH,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        );
        unsafe {
            // gfx-post: a copy of the device table, so post_run can borrow self
            let device = self.device.clone();
            let d = &device;
            let c = self.commands;
            // msaa: into the samples, started as the frame starts (the
            // target keeps the start; msaa_begin draws the emulated picture)
            let mut one_layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
            if samples > 1 {
                one_layout = self.msaa_begin(samples, width, height, (color, color_view), start_clear, start_keep, depth_clear, blit)?;
            } else {
                d.cmd_begin_render_pass(
                    c,
                    &vk::RenderPassBeginInfo::default()
                        .render_pass(self.render_pass)
                        .framebuffer(framebuffer)
                        .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }),
                    vk::SubpassContents::INLINE,
                );
            }
            if let Some((set, area)) = blit.filter(|_| samples == 1) {
                d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, self.blit_pipeline);
                d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, self.blit_layout, 0, &[set], &[]);
                let bytes: Vec<u8> = area.iter().flat_map(|v| v.to_le_bytes()).collect();
                d.cmd_push_constants(c, self.blit_layout, vk::ShaderStageFlags::VERTEX, 0, &bytes);
                d.cmd_set_viewport(
                    c,
                    0,
                    &[vk::Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }],
                );
                d.cmd_set_scissor(c, 0, &[vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }]);
                d.cmd_draw(c, 3, 1, 0, 0);
            }
            d.cmd_bind_vertex_buffers(c, 0, &[self.vertex.as_ref().unwrap().buffer], &[0]);
            d.cmd_bind_index_buffer(c, self.index.as_ref().unwrap().buffer, 0, vk::IndexType::UINT32);
            for (n, dr) in draws.iter().enumerate() {
                // gfx-post: a post effect, run outside the render pass at
                // its place in the frame, then the pass resumes
                if let Some(block) = dr.post.as_deref() {
                    d.cmd_end_render_pass(c);
                    // msaa: the effect reads and draws the resolved picture,
                    // which then goes back into the samples
                    if samples > 1 {
                        self.msaa_resolve(width, height, color, one_layout);
                    }
                    let _t = crate::perf::scope(crate::perf::C::PostRun); // gfx-perf
                    self.post_run(block, disp, width, height, scale, decoded)?;
                    if samples > 1 {
                        one_layout = self.msaa_resume(samples, width, height, (color, color_view))?;
                    } else {
                        d.cmd_begin_render_pass(
                            c,
                            &vk::RenderPassBeginInfo::default()
                                .render_pass(self.render_pass)
                                .framebuffer(framebuffer)
                                .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }),
                            vk::SubpassContents::INLINE,
                        );
                    }
                    d.cmd_bind_vertex_buffers(c, 0, &[self.vertex.as_ref().unwrap().buffer], &[0]);
                    d.cmd_bind_index_buffer(c, self.index.as_ref().unwrap().buffer, 0, vk::IndexType::UINT32);
                    continue;
                }
                if let (Some(p), Some(s)) = (&shaded_prepared[n], dr.shaded.as_deref()) {
                    let g = self.shaded.as_ref().unwrap();
                    d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, p.pipeline);
                    d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, g.layout, 0, &[p.set], &[]);
                    // gfx-shadow: an offscreen target is its framebuffer's height wide
                    let fb_h = match disp { 0 => 400.0, 1 => 320.0, _ => width as f32 / scale };
                    let scale_x = width as f32 / fb_h;
                    let up = |r: [i32; 4]| -> [f32; 4] {
                        let (x, y, w, h) = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
                        [(fb_h - (y + h)) * scale_x, x * scale, h * scale_x, w * scale]
                    };
                    let vp = up(s.viewport);
                    d.cmd_set_viewport(
                        c,
                        0,
                        &[vk::Viewport { x: vp[0], y: vp[1], width: vp[2], height: vp[3], min_depth: s.depth_range.0, max_depth: s.depth_range.1 }],
                    );
                    let sc = up(scissor_mirrored(s.scissor, (height as f32 / scale).round() as i32)); // gfx-terrain
                    let x0 = (sc[0] as i32).clamp(0, width as i32);
                    let y0 = (sc[1] as i32).clamp(0, height as i32);
                    let x1 = ((sc[0] + sc[2]) as i32).clamp(x0, width as i32);
                    let y1 = ((sc[1] + sc[3]) as i32).clamp(y0, height as i32);
                    d.cmd_set_scissor(
                        c,
                        0,
                        &[vk::Rect2D { offset: vk::Offset2D { x: x0, y: y0 }, extent: vk::Extent2D { width: (x1 - x0) as u32, height: (y1 - y0) as u32 } }],
                    );
                    d.cmd_set_blend_constants(c, &s.blend_color);
                    d.cmd_bind_index_buffer(c, g.buffer.as_ref().unwrap().buffer, p.index_offset, vk::IndexType::UINT32);
                    d.cmd_draw_indexed(c, p.count, crate::drawdist::instances(s), 0, 0, 0); // drawdist: a batch of tufts
                    d.cmd_bind_index_buffer(c, self.index.as_ref().unwrap().buffer, 0, vk::IndexType::UINT32);
                    continue;
                }
                let (pipeline, set) = prepared[n];
                d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, pipeline);
                d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[set], &[]);
                d.cmd_push_constants(
                    c,
                    self.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    &(n as u32).to_le_bytes(),
                );
                // the game's rectangles are in the portrait framebuffer
                // (240 across, window y up from the bottom); on the upright
                // screen framebuffer x runs down and framebuffer y runs
                // right to left. A widescreen top screen is wider than
                // fb_h * scale: framebuffer y then stretches by k, and a
                // rectangle that isn't the whole width keeps its size in the
                // middle, as the wide agent's Pica::ScissorTest/Viewport
                // hooks squeeze it for the emulated path.
                let fb_h = match disp { 0 => 400.0, 1 => 320.0, _ => width as f32 / scale }; // gfx-shadow
                let scale_x = width as f32 / fb_h;
                let k = scale_x / scale;
                let upright = |r: [i32; 4]| -> [f32; 4] {
                    let (x, w) = (r[0] as f32, r[2] as f32);
                    let (mut y, mut h) = (r[1], r[3]);
                    if disp == 0 && k > 1.001 && !(y <= 0 && y + h >= 400) {
                        let y0 = 200.0 + (y as f32 - 200.0) / k;
                        let y1 = 200.0 + ((y + h) as f32 - 200.0) / k;
                        let (a, b) = ((y0 + 0.5).floor() as i32, (y1 + 0.5).floor() as i32);
                        y = a;
                        h = b - a;
                    }
                    let (y, h) = (y as f32, h as f32);
                    [(fb_h - (y + h)) * scale_x, x * scale, h * scale_x, w * scale]
                };
                let vp = upright(dr.viewport);
                d.cmd_set_viewport(
                    c,
                    0,
                    &[vk::Viewport { x: vp[0], y: vp[1], width: vp[2], height: vp[3], min_depth: 0.0, max_depth: 1.0 }],
                );
                // gfx-terrain: the scissor's x runs the other way from the
                // viewport's (DispRect::GetRealRect 0x00545B3C puts the
                // bottom of the upright screen at x = 0): the talk windows'
                // text, scissored to the window, was cut away
                let sc = upright(scissor_mirrored(dr.scissor, (height as f32 / scale).round() as i32));
                let x0 = (sc[0] as i32).clamp(0, width as i32);
                let y0 = (sc[1] as i32).clamp(0, height as i32);
                let x1 = ((sc[0] + sc[2]) as i32).clamp(x0, width as i32);
                let y1 = ((sc[1] + sc[3]) as i32).clamp(y0, height as i32);
                d.cmd_set_scissor(
                    c,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D { x: x0, y: y0 },
                        extent: vk::Extent2D { width: (x1 - x0) as u32, height: (y1 - y0) as u32 },
                    }],
                );
                let (first, count) = ranges[n];
                d.cmd_draw_indexed(c, count, 1, first, 0, 0);
            }
            d.cmd_end_render_pass(c);
            // msaa: the samples into the target, where every reader finds the frame
            if samples > 1 {
                self.msaa_resolve(width, height, color, one_layout);
            }
        }
        self.gpu_time_stop(disp, width, height, samples, draws.len()); // msaa: FATES_GFX_GPU_TIME (perf3: and the draws)
        self.barrier(
            color,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        drop(t_record); // gfx-perf
        self.uploads.recording = false;
        let out = match finish {
            Finish::Readback => {
                unsafe {
                    self.device.cmd_copy_image_to_buffer(
                        self.commands,
                        color,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        sbuf,
                        &[copy_region(width, height, vk::ImageAspectFlags::COLOR)],
                    );
                }
                {
                    let _t = crate::perf::scope(crate::perf::C::Wait); // gfx-perf
                    self.submit_and_wait()?;
                }
                let _t = crate::perf::scope(crate::perf::C::Readback); // gfx-perf
                let mut out = vec![0u8; pixels as usize];
                unsafe { std::ptr::copy_nonoverlapping(self.staging.as_ref().unwrap().ptr, out.as_mut_ptr(), pixels as usize) };
                Output::Pixels(out)
            }
            Finish::Present => {
                let (image, view) = self.present_image(disp, width, height)?;
                self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
                let region = vk::ImageCopy::default()
                    .src_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                    .dst_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                    .extent(vk::Extent3D { width, height, depth: 1 });
                unsafe {
                    self.device.cmd_copy_image(
                        self.commands,
                        color,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
                    );
                }
                self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::GENERAL);
                // gfx-perf: not waited for. The presenter samples the image
                // on the same queue, after the barrier above in submission
                // order; the next render waits before reusing anything.
                // FATES_GFX_PRESENT_WAIT=1 waits as before.
                if present_wait() {
                    let _t = crate::perf::scope(crate::perf::C::Wait);
                    self.submit_and_wait()?;
                } else {
                    self.submit()?;
                }
                Output::Screen(view, width, height)
            }
            Finish::Keep => {
                self.submit()?; // gfx-perf
                Output::Kept
            }
        };
        if self.frame % 120 == 0 {
            self.evict();
        }
        Ok(out)
    }
}

/// gfx2d3: whether a 2D draw's unit 0 is clamped to its atlas cells
/// (edges.rs): linearly filtered, clamped to its edge on both axes, and
/// not sampled sharp (pixel art keeps its own sampling)
fn edge_clamp(d: &Draw, scale: f32) -> bool {
    if !crate::edges::enabled() || d.shaded.is_some() || d.post.is_some() {
        return false;
    }
    let Some((_, s)) = d.textures[0] else { return false };
    (s.linear_mag || s.linear_min) && s.wrap == 0 && s.wrap_t == 0 && !(s.sharp && (scale > 1.5 || s.linear_mag))
}

/// gfx-perf: FATES_GFX_UPLOAD_WAIT=1 uploads each texture in a submission
/// of its own, waited for.
fn upload_wait() -> bool {
    static WAIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WAIT.get_or_init(|| std::env::var("FATES_GFX_UPLOAD_WAIT").is_ok_and(|v| v == "1"))
}

/// gfx-perf: FATES_GFX_PRESENT_WAIT=1 waits for each presented picture.
fn present_wait() -> bool {
    static WAIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WAIT.get_or_init(|| std::env::var("FATES_GFX_PRESENT_WAIT").is_ok_and(|v| v == "1"))
}

/// gfx-terrain: a PICA scissor rectangle (x, y, w, h) with its x mirrored
/// across the framebuffer's `across` (240 for both displays), into the
/// orientation the viewport mapping uses. Checked in shadow mode against
/// the emulated picture (Chapter 1 cutscene, f29000: the talk windows'
/// text appears and the top screen's mean difference drops 8.4 -> 7.6).
/// FATES_GFX_SCISSOR_FLIP=0 turns it off.
fn scissor_mirrored(r: [i32; 4], across: i32) -> [i32; 4] {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("FATES_GFX_SCISSOR_FLIP").is_ok_and(|v| v == "0")) {
        return r;
    }
    [across - (r[0] + r[2]), r[1], r[2], r[3]]
}

fn copy_region(width: u32, height: u32, aspect: vk::ImageAspectFlags) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .image_subresource(vk::ImageSubresourceLayers { aspect_mask: aspect, mip_level: 0, base_array_layer: 0, layer_count: 1 })
        .image_extent(vk::Extent3D { width, height, depth: 1 })
}

impl Drop for Renderer {
    fn drop(&mut self) {
        crate::cache::warm_join(); // smooth: the warm thread uses a renderer's modules
        let _ = zakuro_gpu::wait_idle(&self.device); // smooth: keeps off the present thread
        self.msaa_destroy(); // msaa
        self.gpu_time_destroy(); // msaa
        let textures: Vec<_> = self.textures.drain().map(|(_, t)| t).collect();
        for t in textures {
            self.destroy_image(&t.image);
        }
        // gfx-shadow: the offscreen target; gfx-hfr: and the pictures in
        // between's
        for t in [self.targets[2].take(), self.targets[3].take(), self.targets[4].take()].into_iter().flatten() {
            unsafe { self.device.destroy_framebuffer(t.framebuffer, None) };
            self.destroy_image(&t.color);
            self.destroy_image(&t.depth);
        }
        crate::cache::reap_flush(); // smooth: evicted textures gone first
        for disp in 0..2 {
            if let Some(t) = self.targets[disp].take() {
                unsafe { self.device.destroy_framebuffer(t.framebuffer, None) };
                self.destroy_image(&t.color);
                self.destroy_image(&t.depth);
            }
            for image in std::mem::take(&mut self.present[disp]) {
                self.destroy_image(&image);
            }
        }
        unsafe {
            self.device.destroy_pipeline(self.blit_pipeline, None);
            self.device.destroy_pipeline_layout(self.blit_layout, None);
            self.device.destroy_descriptor_set_layout(self.blit_set_layout, None);
            for m in self.blit_modules {
                self.device.destroy_shader_module(m, None);
            }
        }
        for b in [self.vertex.take(), self.index.take(), self.params.take(), self.staging.take()].into_iter().flatten() {
            self.destroy_buffer(b);
        }
        self.uploads_destroy(); // gfx-perf
        let white = std::mem::replace(
            &mut self.white,
            Image { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
        );
        self.destroy_image(&white);
        unsafe {
            for (_, p) in self.pipelines.drain() {
                self.device.destroy_pipeline(p, None);
            }
            for (_, s) in self.samplers.drain() {
                self.device.destroy_sampler(s, None);
            }
            for (_, s) in self.hd_samplers.drain() {
                self.device.destroy_sampler(s, None); // hdpacks
            }
            self.device.destroy_shader_module(self.vs, None);
            self.device.destroy_shader_module(self.fs, None);
            self.device.destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            let device = self.device.clone();
            self.pipeline_disk.destroy(&device); // gfx-perf
            // the presenter's device is the presenter's to destroy
            if self.shared.is_none() {
                self.device.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shaders_translate() {
        super::glsl_to_spirv(super::VERTEX_GLSL, naga::ShaderStage::Vertex).unwrap();
        super::glsl_to_spirv(super::FRAGMENT_GLSL, naga::ShaderStage::Fragment).unwrap();
        super::glsl_to_spirv(super::BLIT_VERTEX_GLSL, naga::ShaderStage::Vertex).unwrap();
        super::glsl_to_spirv(super::BLIT_FRAGMENT_GLSL, naga::ShaderStage::Fragment).unwrap();
    }
}

/// smooth: a target, the presented images or textures made again or let
/// go, timed into the timing log (FATES_SMOOTH_LOG, kind realloc; a = 1
/// target, 2 presented images, 3 evicted textures, msaa: 4 the multisampled
/// scratch; b = width x height).
struct Realloc(u32, u32, u32, std::time::Instant);

impl Realloc {
    fn new(what: u32, width: u32, height: u32) -> Realloc {
        Realloc(what, width, height, std::time::Instant::now())
    }
}

impl Drop for Realloc {
    fn drop(&mut self) {
        if self.3.elapsed() > std::time::Duration::from_micros(300) {
            zakuro_gpu::smooth::since("realloc", self.3, self.0 as i64, (self.1 as f64) * 10000.0 + self.2 as f64);
        }
    }
}
