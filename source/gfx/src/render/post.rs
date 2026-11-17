//! gfx-post: the renderer side of native post effects: the surfaces
//! (surfaces.rs) as Vulkan images, the actions that keep them up to date,
//! and post draws (post.rs) with tex3.shbin's vertex program and the PICA
//! combiner, into surfaces or the display target, in the order the game
//! issued them.
//!
//! Surfaces are portrait images in memory-row order (image row 0 is memory
//! row 0, the top of a PICA framebuffer), at the renderer's scale, kept in
//! the GENERAL layout; every action is separated by a full memory barrier.
//! The display target stays upright, as the rest of the frame is drawn; a
//! capture turns rows of it into a portrait surface.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ash::vk;

use super::{glsl_to_spirv, vkerr, Buffer, Decoded, Image, Renderer};
use crate::post::{Block, Op, PostDraw, Target, Unit};
use crate::prim::{Sampling, TexRef};
use crate::surfaces::{Action, Desc, Key};

const STRIDE: usize = 27;
const PARAMS: usize = STRIDE * 16;
const ALIGN: u64 = 256;

const POST_VS: &str = r#"#version 450
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec2 a_uv0;
layout(location = 2) in vec2 a_uv1;
layout(location = 3) in vec2 a_uv2;
layout(location = 4) in vec4 a_color;
layout(std430, set = 0, binding = 0) readonly buffer DrawParams { vec4 pv[]; };
layout(push_constant) uniform Push { uint index; } pc;
layout(location = 0) out vec4 v_color;
layout(location = 1) out vec2 v_uv0;
layout(location = 2) out vec2 v_uv1;
layout(location = 3) out vec2 v_uv2;
void main() {
    // tex3.shbin: pos.w = 1, o0 = uViewProj * pos, colour = c77 (1/255) *
    // the byte colour, three texture coordinates passed on
    vec4 pos = vec4(a_pos, 1.0);
    vec4 clip = vec4(dot(pv[0], pos), dot(pv[1], pos), dot(pv[2], pos), dot(pv[3], pos));
    v_color = a_color;
    v_uv0 = a_uv0;
    v_uv1 = a_uv1;
    v_uv2 = a_uv2;
    if (pv[4].x > 0.5) {
        // the display target: the portrait framebuffer turned upright, as
        // the 2D layer does it
        gl_Position = vec4(-clip.y, clip.x, -clip.z, clip.w);
    } else {
        // a surface in memory-row order: window y runs up from the last
        // row. (naga's SPIR-V output negates gl_Position.y, so this puts
        // clip y = -1 at the bottom of the viewport.)
        gl_Position = vec4(clip.x, clip.y, -clip.z, clip.w);
    }
}
"#;

const POST_FS: &str = r#"#version 450
layout(std430, set = 0, binding = 0) readonly buffer DrawParams { vec4 pv[]; };
layout(set = 0, binding = 1) uniform texture2D tex0;
layout(set = 0, binding = 2) uniform texture2D tex1;
layout(set = 0, binding = 3) uniform texture2D tex2;
layout(set = 0, binding = 4) uniform sampler samp0;
layout(set = 0, binding = 5) uniform sampler samp1;
layout(set = 0, binding = 6) uniform sampler samp2;
layout(location = 0) in vec4 v_color;
layout(location = 1) in vec2 v_uv0;
layout(location = 2) in vec2 v_uv1;
layout(location = 3) in vec2 v_uv2;
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
vec3 combine3(int f, vec3 a, vec3 b, vec3 c) {
    if (f == 0) return a;
    if (f == 1) return a * b;
    if (f == 2) return a + b;
    if (f == 3) return a + b - vec3(0.5);
    if (f == 4) return a * c + b * (vec3(1.0) - c);
    if (f == 5) return a - b;
    if (f == 6 || f == 7) return vec3(4.0 * dot(a - vec3(0.5), b - vec3(0.5)));
    if (f == 8) return a * b + c;
    if (f == 9) return (a + b) * c;
    return a;
}
float combine1(int f, float a, float b, float c) {
    if (f == 0) return a;
    if (f == 1) return a * b;
    if (f == 2) return a + b;
    if (f == 3) return a + b - 0.5;
    if (f == 4) return a * c + b * (1.0 - c);
    if (f == 5) return a - b;
    if (f == 8) return a * b + c;
    if (f == 9) return (a + b) * c;
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
// a texture coordinate of the game's texture (s, t) to the image's: wrap
// and clamp in the texture's own range, then the texture's place in its
// surface
vec2 place(int u, vec2 st) {
    float wrap = pv[11][u];
    if (wrap == 2.0) st = fract(st);
    vec4 c = pv[8 + u];
    st = clamp(st, c.xz, c.yw);
    vec4 t = pv[5 + u];
    return vec2(t.x * st.x + t.y, t.z * st.y + t.w);
}
void main() {
    vec4 prim = clamp(v_color, 0.0, 1.0);
    vec4 t0 = texture(sampler2D(tex0, samp0), place(0, v_uv0));
    vec4 t1 = texture(sampler2D(tex1, samp1), place(1, v_uv1));
    vec4 t2 = texture(sampler2D(tex2, samp2), place(2, pv[4].y > 0.5 ? v_uv1 : v_uv2));
    ivec4 upd = ivec4(pv[24]);
    vec4 prev = prim;
    vec4 buf = vec4(0.0);
    vec4 next_buf = pv[25];
    for (int i = 0; i < 6; i++) {
        ivec4 st = ivec4(pv[12 + 2 * i]);
        vec4 k = pv[13 + 2 * i];
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
            if (((upd.x >> i) & 1) != 0) next_buf = vec4(prev.rgb, next_buf.a);
            if (((upd.y >> i) & 1) != 0) next_buf = vec4(next_buf.rgb, prev.a);
        }
    }
    ivec4 at = ivec4(pv[26]);
    if (at.x != 0) {
        float a8 = floor(prev.a * 255.0 + 0.5);
        if (!compare(at.y, a8, float(at.z))) discard;
    }
    o_color = prev;
}
"#;

/// the copy pass: a rectangle of a destination filled from a source
/// image through an affine map of its own (0..1) coordinates
const COPY_VS: &str = r#"#version 450
layout(location = 0) out vec2 p;
void main() {
    vec2 q = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    p = q;
    // y written negated: naga's SPIR-V output negates it again, so p.y = 0
    // is the top row
    gl_Position = vec4(q.x * 2.0 - 1.0, 1.0 - q.y * 2.0, 0.0, 1.0);
}
"#;

const COPY_FS: &str = r#"#version 450
layout(set = 0, binding = 0) uniform texture2D src;
layout(set = 0, binding = 1) uniform sampler samp;
layout(push_constant) uniform Push { vec4 row0; vec4 row1; } pc;
layout(location = 0) in vec2 p;
layout(location = 0) out vec4 o_color;
void main() {
    vec3 q = vec3(p, 1.0);
    vec2 uv = vec2(dot(pc.row0.xyz, q), dot(pc.row1.xyz, q));
    // gfx-2d: row0.w > 1: an n x n box of linear taps over the pixel's
    // footprint, for a copy into a surface drawn at a lower scale
    int n = int(pc.row0.w + 0.5);
    if (n <= 1) {
        o_color = texture(sampler2D(src, samp), uv);
    } else {
        vec2 dx = dFdx(uv);
        vec2 dy = dFdy(uv);
        vec4 sum = vec4(0.0);
        for (int j = 0; j < n; j++) {
            for (int i = 0; i < n; i++) {
                vec2 f = (vec2(float(i), float(j)) + 0.5) / float(n) - 0.5;
                sum += texture(sampler2D(src, samp), uv + dx * f.x + dy * f.y);
            }
        }
        o_color = sum / float(n * n);
    }
}
"#;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    blend: u32,
    mask: u32,
}

struct Surf {
    color: Image,
    depth: Image,
    framebuffer: vk::Framebuffer,
    /// image size in pixels, and the memory rows it holds
    width: u32,
    height: u32,
    rows: u32,
    scale: u32,
    used: u64,
}

pub(super) struct PostGpu {
    surfaces: HashMap<Key, Surf>,
    /// images replaced this frame, destroyed at the next
    retired: Vec<Surf>,
    render_pass: vk::RenderPass,
    vs: vk::ShaderModule,
    fs: vk::ShaderModule,
    pipelines: HashMap<PipeKey, vk::Pipeline>,
    copy_set_layout: vk::DescriptorSetLayout,
    copy_layout: vk::PipelineLayout,
    copy_pipeline: vk::Pipeline,
    zero: Image,
    buffer: Option<Buffer>,
    used: u64,
    frame: u64,
    /// FATES_GFX_POST_SURFDUMP=dir: surfaces read back after a frame
    /// (name, width, height, buffer), written at the next
    dumps: Vec<(String, u32, u32, Buffer)>,
    dumped: u32,
    /// gfx-2d: surfaces a display-transfer downscale writes (a bloom's
    /// shrunk copies): drawn at the console's own scale, see `scale_of`
    low: HashSet<Key>,
}

impl Renderer {
    fn post_init(&mut self) -> Result<(), String> {
        if self.post.is_some() {
            return Ok(());
        }
        unsafe {
            let d = &self.device;
            let attachments = [
                vk::AttachmentDescription::default()
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(vk::ImageLayout::GENERAL)
                    .final_layout(vk::ImageLayout::GENERAL),
                vk::AttachmentDescription::default()
                    .format(vk::Format::D32_SFLOAT)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::GENERAL)
                    .final_layout(vk::ImageLayout::GENERAL),
            ];
            let color_ref = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::GENERAL)];
            let depth_ref = vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::GENERAL);
            let subpass = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_ref)
                .depth_stencil_attachment(&depth_ref)];
            let render_pass = d
                .create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpass), None)
                .map_err(vkerr("post render pass"))?;
            let module = |src: &str, stage| -> Result<vk::ShaderModule, String> {
                let code = glsl_to_spirv(src, stage)?;
                d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None).map_err(vkerr("post module"))
            };
            let vs = module(POST_VS, naga::ShaderStage::Vertex)?;
            let fs = module(POST_FS, naga::ShaderStage::Fragment)?;
            let cvs = module(COPY_VS, naga::ShaderStage::Vertex)?;
            let cfs = module(COPY_FS, naga::ShaderStage::Fragment)?;
            let binding = [
                vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let copy_set_layout = d
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding), None)
                .map_err(vkerr("copy set layout"))?;
            let push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::FRAGMENT).size(32)];
            let layouts = [copy_set_layout];
            let copy_layout = d
                .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)
                .map_err(vkerr("copy layout"))?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(cvs).name(c"main"),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(cfs).name(c"main"),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
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
                .layout(copy_layout)
                .render_pass(render_pass)];
            let copy_pipeline = d.create_graphics_pipelines(vk::PipelineCache::null(), &info, None).map_err(|(_, e)| format!("copy pipeline: {e:?}"))?[0];
            let zero = self.upload_image(1, 1, &[0, 0, 0, 0])?;
            self.post = Some(PostGpu {
                surfaces: HashMap::new(),
                retired: Vec::new(),
                render_pass,
                vs,
                fs,
                pipelines: HashMap::new(),
                copy_set_layout,
                copy_layout,
                copy_pipeline,
                zero,
                buffer: None,
                used: 0,
                frame: 0,
                dumps: Vec::new(),
                dumped: 0,
                low: HashSet::new(),
            });
        }
        Ok(())
    }

    /// before a frame's commands are recorded: room for its post draws,
    /// guest textures uploaded, old images dropped
    pub(super) fn post_reserve(&mut self, blocks: &[&Block], texture: &mut dyn FnMut(&mut Renderer, TexRef, u64) -> Result<(), String>) -> Result<(), String> {
        self.post_init()?;
        let al = |n: u64| n.div_ceil(ALIGN) * ALIGN;
        let mut size = 0u64;
        for b in blocks {
            for op in &b.ops {
                if let Op::Draw(d) = op {
                    size += al(d.vertices.len() as u64) + al(d.indices.len() as u64 * 4) + al(PARAMS as u64);
                    for u in d.units {
                        if let Unit::Guest { tex, key: Some(k), .. } = u {
                            texture(self, tex, k)?;
                        }
                    }
                }
            }
        }
        let have = self.post.as_ref().unwrap().buffer.as_ref().map_or(0, |b| b.size);
        if have < size.max(ALIGN) {
            if let Some(old) = self.post.as_mut().unwrap().buffer.take() {
                zakuro_gpu::wait_idle(&self.device).ok(); // smooth: keeps off the present thread
                self.destroy_buffer(old);
            }
            let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER;
            let b = self.buffer((size * 2).max(1 << 18), usage)?;
            self.post.as_mut().unwrap().buffer = Some(b);
        }
        // the surfaces read back last frame, as pictures
        let dumps = std::mem::take(&mut self.post.as_mut().unwrap().dumps);
        for (name, w, h, b) in dumps {
            if let Some(dir) = std::env::var_os("FATES_GFX_POST_SURFDUMP") {
                let mut px = vec![0u8; (w * h * 4) as usize];
                unsafe { std::ptr::copy_nonoverlapping(b.ptr, px.as_mut_ptr(), px.len()) };
                for p in px.chunks_exact_mut(4) {
                    p[3] = 255;
                }
                let _ = std::fs::create_dir_all(&dir);
                let _ = crate::write_png(&std::path::Path::new(&dir).join(name), &px, w, h);
            }
            self.destroy_buffer(b);
        }
        let g = self.post.as_mut().unwrap();
        g.used = 0;
        g.frame += 1;
        let retired = std::mem::take(&mut g.retired);
        // surfaces no frame used for a long time
        let frame = g.frame;
        let old: Vec<Key> = g.surfaces.iter().filter(|(_, s)| frame - s.used > 1200).map(|(k, _)| *k).collect();
        let mut drop: Vec<Surf> = retired;
        for k in old {
            drop.extend(g.surfaces.remove(&k));
        }
        if !drop.is_empty() {
            zakuro_gpu::wait_idle(&self.device).ok(); // smooth: keeps off the present thread
            for s in drop {
                self.destroy_surf(s);
            }
        }
        Ok(())
    }

    fn destroy_surf(&self, s: Surf) {
        unsafe { self.device.destroy_framebuffer(s.framebuffer, None) };
        self.destroy_image(&s.color);
        self.destroy_image(&s.depth);
    }

    fn post_put(&mut self, bytes: &[u8]) -> u64 {
        let g = self.post.as_mut().unwrap();
        let b = g.buffer.as_ref().unwrap();
        let at = g.used;
        assert!(at + bytes.len() as u64 <= b.size, "post buffer reserved too small");
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.ptr.add(at as usize), bytes.len()) };
        g.used = (at + bytes.len() as u64).div_ceil(ALIGN) * ALIGN;
        at
    }

    fn post_barrier(&self) {
        let b = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)];
        unsafe {
            self.device.cmd_pipeline_barrier(self.commands, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &b, &[], &[]);
        }
    }

    /// the surface's image, made (cleared to zero) or grown to `desc.rows`
    /// rows at `scale`, its old rows kept
    /// gfx-2d: the scale a surface is drawn at. A surface a display-transfer
    /// downscale writes holds a shrunk copy of the picture (BloomContext's
    /// 128x200, 64x104 and 32x64) that the effect then blurs with taps a
    /// texel or so apart and lays back over the picture, magnified. At the
    /// renderer's scale those copies kept the full picture's detail, so the
    /// taps laid sharp copies side by side: ghost outlines around the avatar
    /// and anything else bloom lights, plainer the higher the scale. They
    /// are drawn at the console's own size instead, as the PICA does, and
    /// magnified smoothly where the picture samples them.
    /// FATES_GFX_POST_LOWRES=0 keeps every surface at the renderer's scale.
    fn scale_of(&self, key: Key, sc: u32) -> u32 {
        let on = crate::perf::var("FATES_GFX_POST_LOWRES").map_or(true, |v| v != "0");
        if on && self.post.as_ref().is_some_and(|g| g.low.contains(&key)) {
            1
        } else {
            sc
        }
    }

    fn surf(&mut self, desc: Desc, scale: u32) -> Result<(), String> {
        let frame = self.post.as_ref().unwrap().frame;
        let have = self.post.as_ref().unwrap().surfaces.get(&desc.key).map(|s| (s.rows, s.scale));
        if let Some((rows, s)) = have {
            if rows >= desc.rows && s == scale {
                self.post.as_mut().unwrap().surfaces.get_mut(&desc.key).unwrap().used = frame;
                return Ok(());
            }
        }
        let rows = desc.rows.max(have.map_or(0, |h| h.0)).max(8);
        let (w, h) = (desc.key.width.max(8) * scale, rows * scale);
        let color = self.image(
            w,
            h,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        let depth = self.image(w, h, vk::Format::D32_SFLOAT, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, vk::ImageAspectFlags::DEPTH)?;
        let views = [color.view, depth.view];
        let render_pass = self.post.as_ref().unwrap().render_pass;
        let framebuffer = unsafe {
            self.device.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(render_pass).attachments(&views).width(w).height(h).layers(1), None)
        }
        .map_err(vkerr("surface framebuffer"))?;
        self.barrier(color.image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL);
        self.barrier(depth.image, vk::ImageAspectFlags::DEPTH, vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL);
        let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        unsafe {
            self.device.cmd_clear_color_image(self.commands, color.image, vk::ImageLayout::GENERAL, &vk::ClearColorValue { float32: [0.0; 4] }, &[range]);
        }
        let new_image = color.image;
        let new = Surf { color, depth, framebuffer, width: w, height: h, rows, scale, used: frame };
        let old = self.post.as_mut().unwrap().surfaces.insert(desc.key, new);
        if let Some(old) = old {
            if old.scale == scale {
                let layers = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
                let region = vk::ImageCopy::default().src_subresource(layers).dst_subresource(layers).extent(vk::Extent3D { width: old.width, height: old.height, depth: 1 });
                self.post_barrier();
                unsafe { self.device.cmd_copy_image(self.commands, old.color.image, vk::ImageLayout::GENERAL, new_image, vk::ImageLayout::GENERAL, &[region]) };
            }
            self.post.as_mut().unwrap().retired.push(old);
        }
        self.post_barrier();
        Ok(())
    }

    fn begin_surface_pass(&self, key: Key) {
        let g = self.post.as_ref().unwrap();
        let s = &g.surfaces[&key];
        unsafe {
            self.device.cmd_begin_render_pass(
                self.commands,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(g.render_pass)
                    .framebuffer(s.framebuffer)
                    .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: s.width, height: s.height } }),
                vk::SubpassContents::INLINE,
            );
        }
    }

    fn rect2d(r: [u32; 4], scale: u32, w: u32, h: u32) -> vk::Rect2D {
        let x0 = (r[0] * scale).min(w);
        let y0 = (r[1] * scale).min(h);
        let x1 = (r[2] * scale).clamp(x0, w);
        let y1 = (r[3] * scale).clamp(y0, h);
        vk::Rect2D { offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 }, extent: vk::Extent2D { width: x1 - x0, height: y1 - y0 } }
    }

    /// the copy pass into a rectangle of `dst`, sampling `view`
    fn copy_pass(&mut self, dst: Key, rect: vk::Rect2D, view: vk::ImageView, layout: vk::ImageLayout, linear: bool, mut rows: [[f32; 4]; 2], taps: u32) -> Result<(), String> {
        rows[0][3] = taps as f32; // gfx-2d: the box (COPY_FS)
        if rect.extent.width == 0 || rect.extent.height == 0 {
            return Ok(());
        }
        let sampler = self.sampler(Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: linear, linear_min: linear })?;
        let g = self.post.as_ref().unwrap();
        let layouts = [g.copy_set_layout];
        let set = unsafe { self.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.descriptor_pool).set_layouts(&layouts)) }
            .map_err(vkerr("copy set"))?[0];
        let info = [vk::DescriptorImageInfo::default().image_view(view).image_layout(layout)];
        let sinfo = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let write = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sinfo),
        ];
        let (pipeline, playout) = (g.copy_pipeline, g.copy_layout);
        unsafe { self.device.update_descriptor_sets(&write, &[]) };
        self.begin_surface_pass(dst);
        let push: Vec<u8> = rows.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        unsafe {
            let (d, c) = (&self.device, self.commands);
            d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, pipeline);
            d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, playout, 0, &[set], &[]);
            d.cmd_push_constants(c, playout, vk::ShaderStageFlags::FRAGMENT, 0, &push);
            d.cmd_set_viewport(c, 0, &[vk::Viewport { x: rect.offset.x as f32, y: rect.offset.y as f32, width: rect.extent.width as f32, height: rect.extent.height as f32, min_depth: 0.0, max_depth: 1.0 }]);
            d.cmd_set_scissor(c, 0, &[rect]);
            d.cmd_draw(c, 3, 1, 0, 0);
            d.cmd_end_render_pass(c);
        }
        Ok(())
    }

    fn post_pipeline(&mut self, key: PipeKey) -> Result<vk::Pipeline, String> {
        if let Some(&p) = self.post.as_ref().unwrap().pipelines.get(&key) {
            return Ok(p);
        }
        let g = self.post.as_ref().unwrap();
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(g.vs).name(c"main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(g.fs).name(c"main"),
        ];
        let binding = [vk::VertexInputBindingDescription::default().binding(0).stride(40).input_rate(vk::VertexInputRate::VERTEX)];
        let attr = |l: u32, f: vk::Format, o: u32| vk::VertexInputAttributeDescription::default().location(l).binding(0).format(f).offset(o);
        let attrs = [
            attr(0, vk::Format::R32G32B32_SFLOAT, 0),
            attr(1, vk::Format::R32G32_SFLOAT, 12),
            attr(2, vk::Format::R32G32_SFLOAT, 20),
            attr(3, vk::Format::R32G32_SFLOAT, 28),
            attr(4, vk::Format::R8G8B8A8_UNORM, 36),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&binding).vertex_attribute_descriptions(&attrs);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default();
        // the PICA numbers its blend factors and equations as Vulkan does
        let config = key.blend;
        let factor = |raw: u32| if raw & 0xF == 15 { vk::BlendFactor::ONE } else { vk::BlendFactor::from_raw((raw & 0xF) as i32) };
        let equation = |raw: u32| if raw & 7 > 4 { vk::BlendOp::ADD } else { vk::BlendOp::from_raw((raw & 7) as i32) };
        let attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .color_blend_op(equation(config))
            .alpha_blend_op(equation(config >> 8))
            .src_color_blend_factor(factor(config >> 16))
            .dst_color_blend_factor(factor(config >> 20))
            .src_alpha_blend_factor(factor(config >> 24))
            .dst_alpha_blend_factor(factor(config >> 28))
            .color_write_mask(vk::ColorComponentFlags::from_raw(key.mask))];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR, vk::DynamicState::BLEND_CONSTANTS];
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
            .render_pass(self.render_pass)];
        // smooth: through the logged maker (kind 2: post)
        let p = crate::cache::make_pipeline(&self.device, self.pipeline_disk.cache, info[0], 2.0).map_err(|e| format!("post pipeline: {e:?}"))?; // gfx-perf: kept on disk
        self.pipeline_disk.created(&self.device);
        self.post.as_mut().unwrap().pipelines.insert(key, p);
        Ok(p)
    }

    /// runs a recorded post effect, with the display target `disp`
    /// (`width` x `height`, `scale` framebuffer units per pixel) holding
    /// the frame so far, outside a render pass
    #[allow(clippy::too_many_arguments)]
    pub(super) fn post_run(&mut self, block: &Block, disp: usize, width: u32, height: u32, scale: f32, decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> Result<(), String> {
        // FATES_GFX_POST_SKIP=1 (dev): recorded, not drawn, to see what the
        // effect itself adds
        if std::env::var_os("FATES_GFX_POST_SKIP").is_some() {
            return Ok(());
        }
        let sc = scale.round().max(1.0) as u32;
        // gfx-2d: the downscales' destinations are drawn at scale 1 (scale_of)
        for op in &block.ops {
            if let Op::Act(Action::Filter { src, dst, .. }) = op {
                if src.key != dst.key {
                    self.post.as_mut().unwrap().low.insert(dst.key);
                }
            }
        }
        let (display_image, display_view, display_fb) = {
            let t = self.targets[disp].as_ref().unwrap();
            (t.color.image, t.color.view, t.framebuffer)
        };
        let (dw, fb_h) = {
            let (w, h) = crate::prim::display_size(disp as u32);
            (w as f32, h as f32)
        };
        self.dump_display("display-before.png", display_image, width, height)?;
        // gfx-gaps: dev, FATES_GFX_POST_DROP=i,j: ops of each block left out
        let drop: Vec<usize> = crate::perf::var("FATES_GFX_POST_DROP").map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect()).unwrap_or_default();
        for (op_index, op) in block.ops.iter().enumerate() {
            if drop.contains(&op_index) {
                continue;
            }
            // gfx-2d: dev, FATES_GFX_POST_OPS=n: the ops of the first n blocks
            if crate::perf::var("FATES_GFX_POST_OPS").ok().and_then(|v| v.parse::<u32>().ok()).is_some_and(|n| self.post.as_ref().unwrap().dumped < n) {
                match op {
                    Op::Draw(d) => {
                        let units: Vec<String> = d.units.iter().map(|u| match u {
                            Unit::Surface { desc, row, width, height, sampling } => format!("surf {:08X}/{} row {row} {width}x{height} {sampling:?}", desc.key.addr, desc.key.width),
                            other => format!("{other:?}").chars().take(60).collect(),
                        }).collect();
                        let target = match &d.target {
                            Target::Surface { desc, row, rows } => format!("surf {:08X}/{} row {row} rows {rows}", desc.key.addr, desc.key.width),
                            _ => "display".to_string(),
                        };
                        eprintln!("gfx-post-op {op_index}: draw -> {target} vp {:?} n {} units {units:?}", d.viewport, d.indices.len());
                    }
                    other => eprintln!("gfx-post-op {op_index}: {:?}", other),
                }
                if op_index + 1 == block.ops.len() {
                    self.post.as_mut().unwrap().dumped += 1;
                }
            }
            self.post_barrier();
            match op {
                Op::Callback(_) => {}
                Op::Act(Action::Capture { dst, rect, row_offset, fb_rows, .. }) => {
                    let dsc = self.scale_of(dst.key, sc);
                    self.surf(*dst, dsc)?;
                    let s = &self.post.as_ref().unwrap().surfaces[&dst.key];
                    let (sw, sh) = (s.width, s.height);
                    // columns past the display's width hold nothing
                    let x1 = (rect[2] as f32).min(dw) as u32;
                    if rect[2] > x1 {
                        self.clear_rect(dst.key, [x1.max(rect[0]), rect[1], rect[2], rect[3]], 0, dsc)?;
                    }
                    if x1 > rect[0] {
                        self.barrier(display_image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
                        let (y0, y1) = (rect[1] as f32, rect[3] as f32);
                        let rows = [
                            // the colour buffer's memory row M is window
                            // y = rows - M, upright column (fb_h - y); its
                            // column X is upright row (240 - X): the 2D
                            // layer's mapping (render.rs, whose SPIR-V has y
                            // negated by naga), checked against the display
                            // target with FATES_GFX_POST_SURFDUMP
                            [0.0, (y1 - y0) / fb_h, (fb_h - *fb_rows as f32 + *row_offset as f32 + y0) / fb_h, 0.0],
                            [-((x1 - rect[0]) as f32) / dw, 0.0, 1.0 - rect[0] as f32 / dw, 0.0],
                        ];
                        let r = Self::rect2d([rect[0], rect[1], x1, rect[3]], dsc, sw, sh);
                        // gfx-2d: into a smaller surface, averaged
                        let taps = (sc / dsc).div_ceil(2);
                        self.copy_pass(dst.key, r, display_view, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, dsc < sc, rows, taps)?;
                        self.barrier(display_image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
                    }
                }
                Op::Act(Action::Clear { dst, rect, value }) => {
                    let dsc = self.scale_of(dst.key, sc);
                    self.surf(*dst, dsc)?;
                    self.clear_rect(dst.key, *rect, *value, dsc)?;
                }
                Op::Act(Action::Copy { src, dst, rects }) => {
                    let (ssc, dsc) = (self.scale_of(src.key, sc), self.scale_of(dst.key, sc));
                    self.surf(*src, ssc)?;
                    self.surf(*dst, dsc)?;
                    if ssc != dsc {
                        // gfx-2d: surfaces at two scales: each rectangle drawn across
                        let g = self.post.as_ref().unwrap();
                        let s = &g.surfaces[&src.key];
                        let (sview, sw_mem, srows) = (s.color.view, src.key.width.max(8) as f32, s.rows as f32);
                        let (dwid, dhei) = (g.surfaces[&dst.key].width, g.surfaces[&dst.key].height);
                        for (r, at) in rects {
                            let sr = r.map(|v| v as f32);
                            let rows = [[(sr[2] - sr[0]) / sw_mem, 0.0, sr[0] / sw_mem, 0.0], [0.0, (sr[3] - sr[1]) / srows, sr[1] / srows, 0.0]];
                            let dr = Self::rect2d([at[0], at[1], at[0] + r[2] - r[0], at[1] + r[3] - r[1]], dsc, dwid, dhei);
                            self.copy_pass(dst.key, dr, sview, vk::ImageLayout::GENERAL, true, rows, (ssc / dsc).div_ceil(2))?;
                            self.post_barrier();
                        }
                        continue;
                    }
                    let sc = ssc;
                    let g = self.post.as_ref().unwrap();
                    let (si, di) = (g.surfaces[&src.key].color.image, g.surfaces[&dst.key].color.image);
                    let (sw, sh) = (g.surfaces[&src.key].width, g.surfaces[&src.key].height);
                    let (dw2, dh2) = (g.surfaces[&dst.key].width, g.surfaces[&dst.key].height);
                    let layers = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
                    let regions: Vec<vk::ImageCopy> = rects
                        .iter()
                        .filter_map(|(r, at)| {
                            let (w, h) = ((r[2] - r[0]) * sc, (r[3] - r[1]) * sc);
                            let (sx, sy, dx, dy) = (r[0] * sc, r[1] * sc, at[0] * sc, at[1] * sc);
                            (sx + w <= sw && sy + h <= sh && dx + w <= dw2 && dy + h <= dh2 && w > 0 && h > 0).then(|| {
                                vk::ImageCopy::default()
                                    .src_subresource(layers)
                                    .dst_subresource(layers)
                                    .src_offset(vk::Offset3D { x: sx as i32, y: sy as i32, z: 0 })
                                    .dst_offset(vk::Offset3D { x: dx as i32, y: dy as i32, z: 0 })
                                    .extent(vk::Extent3D { width: w, height: h, depth: 1 })
                            })
                        })
                        .collect();
                    if !regions.is_empty() {
                        unsafe { self.device.cmd_copy_image(self.commands, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &regions) };
                    }
                }
                Op::Act(Action::Filter { src, src_rect, dst, dst_rect }) => {
                    if src.key == dst.key {
                        continue;
                    }
                    let (ssc, dsc) = (self.scale_of(src.key, sc), self.scale_of(dst.key, sc));
                    self.surf(*src, ssc)?;
                    self.surf(*dst, dsc)?;
                    let g = self.post.as_ref().unwrap();
                    let s = &g.surfaces[&src.key];
                    let (sview, sw_mem, srows) = (s.color.view, src.key.width.max(8) as f32, s.rows as f32);
                    let d = &g.surfaces[&dst.key];
                    let r = Self::rect2d(*dst_rect, dsc, d.width, d.height);
                    let sr = src_rect.map(|v| v as f32);
                    let rows = [[(sr[2] - sr[0]) / sw_mem, 0.0, sr[0] / sw_mem, 0.0], [0.0, (sr[3] - sr[1]) / srows, sr[1] / srows, 0.0]];
                    // gfx-2d: the source pixels behind each destination pixel,
                    // two to a linear tap (one tap for 2x2 at one scale)
                    let ratio = ((sr[2] - sr[0]) * ssc as f32 / (r.extent.width.max(1) as f32)).max(1.0);
                    let taps = (ratio / 2.0).ceil() as u32;
                    self.copy_pass(dst.key, r, sview, vk::ImageLayout::GENERAL, true, rows, taps)?;
                }
                Op::Draw(d) => self.post_draw(d, disp, width, height, scale, sc, display_fb, decoded)?,
            }
        }
        self.post_barrier();
        // FATES_GFX_POST_SURFDUMP=dir (FATES_GFX_POST_SURFDUMP_AT=n: the
        // nth block, default 30): every surface as it is after the block
        let at = std::env::var("FATES_GFX_POST_SURFDUMP_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(30u32);
        if std::env::var_os("FATES_GFX_POST_SURFDUMP").is_some() {
            let n = self.post.as_ref().unwrap().dumped;
            self.post.as_mut().unwrap().dumped += 1;
            if n == at {
                self.dump_display("display-after.png", display_image, width, height)?;
                let list: Vec<(Key, vk::Image, u32, u32)> = self.post.as_ref().unwrap().surfaces.iter().map(|(k, s)| (*k, s.color.image, s.width, s.height)).collect();
                for (k, image, w, h) in list {
                    let b = self.buffer((w * h * 4) as u64, vk::BufferUsageFlags::TRANSFER_DST)?;
                    unsafe { self.device.cmd_copy_image_to_buffer(self.commands, image, vk::ImageLayout::GENERAL, b.buffer, &[super::copy_region(w, h, vk::ImageAspectFlags::COLOR)]) };
                    let name = format!("surface-{:08X}-{}-f{}.png", k.addr, k.width, k.format);
                    self.post.as_mut().unwrap().dumps.push((name, w, h, b));
                }
            }
        }
        Ok(())
    }

    /// FATES_GFX_POST_SURFDUMP: the display target read back (at the block
    /// FATES_GFX_POST_SURFDUMP_AT, before it runs)
    fn dump_display(&mut self, name: &str, image: vk::Image, w: u32, h: u32) -> Result<(), String> {
        if std::env::var_os("FATES_GFX_POST_SURFDUMP").is_none() {
            return Ok(());
        }
        let at = std::env::var("FATES_GFX_POST_SURFDUMP_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(30u32);
        let n = self.post.as_ref().unwrap().dumped;
        if n != at || (name.contains("before") && self.post.as_ref().unwrap().dumps.iter().any(|d| d.0 == name)) {
            return Ok(());
        }
        let b = self.buffer((w * h * 4) as u64, vk::BufferUsageFlags::TRANSFER_DST)?;
        self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        unsafe { self.device.cmd_copy_image_to_buffer(self.commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, b.buffer, &[super::copy_region(w, h, vk::ImageAspectFlags::COLOR)]) };
        self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        self.post.as_mut().unwrap().dumps.push((name.to_owned(), w, h, b));
        Ok(())
    }

    fn clear_rect(&mut self, key: Key, rect: [u32; 4], value: u32, sc: u32) -> Result<(), String> {
        let s = &self.post.as_ref().unwrap().surfaces[&key];
        let r = Self::rect2d(rect, sc, s.width, s.height);
        if r.extent.width == 0 || r.extent.height == 0 {
            return Ok(());
        }
        // RGBA8 memory holds a pixel as the word 0xRRGGBBAA
        let c = [(value >> 24) & 0xFF, (value >> 16) & 0xFF, (value >> 8) & 0xFF, value & 0xFF].map(|v| v as f32 / 255.0);
        self.begin_surface_pass(key);
        unsafe {
            self.device.cmd_clear_attachments(
                self.commands,
                &[vk::ClearAttachment { aspect_mask: vk::ImageAspectFlags::COLOR, color_attachment: 0, clear_value: vk::ClearValue { color: vk::ClearColorValue { float32: c } } }],
                &[vk::ClearRect { rect: r, base_array_layer: 0, layer_count: 1 }],
            );
            self.device.cmd_end_render_pass(self.commands);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn post_draw(&mut self, dr: &PostDraw, disp: usize, width: u32, height: u32, scale: f32, sc: u32, display_fb: vk::Framebuffer, decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> Result<(), String> {
        if dr.indices.is_empty() {
            return Ok(());
        }
        if let Target::Surface { desc, .. } = dr.target {
            let tsc = self.scale_of(desc.key, sc); // gfx-2d
            self.surf(desc, tsc)?;
        }
        // the parameters
        let mut p = [[0f32; 4]; STRIDE];
        for r in 0..4 {
            p[r].copy_from_slice(&dr.matrix[r * 4..r * 4 + 4]);
        }
        p[4] = [matches!(dr.target, Target::Display) as u32 as f32, dr.unit2_uv1 as u32 as f32, 0.0, 0.0];
        let mut views = [self.post.as_ref().unwrap().zero.view; 3];
        let mut layouts = [vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL; 3];
        let mut samplers = [vk::Sampler::null(); 3];
        for (u, unit) in dr.units.iter().enumerate() {
            let (transform, clamp, wrap, sampling) = match *unit {
                Unit::Off => ([1.0, 0.0, 1.0, 0.0], [-1e9, 1e9, -1e9, 1e9], 0.0, Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: false, linear_min: false }),
                Unit::Guest { tex, key, sampling } => {
                    if let Some(k) = key {
                        views[u] = self.texture_view((tex, k), decoded.get(&(tex, k)))?;
                    }
                    ([1.0, 0.0, 1.0, 0.0], [-1e9, 1e9, -1e9, 1e9], 0.0, sampling)
                }
                Unit::Surface { desc, row, width: tw, height: th, sampling } => {
                    let sc = self.scale_of(desc.key, sc); // gfx-2d
                    self.surf(desc, sc)?;
                    let s = &self.post.as_ref().unwrap().surfaces[&desc.key];
                    views[u] = s.color.view;
                    layouts[u] = vk::ImageLayout::GENERAL;
                    let (iw, ir) = (desc.key.width.max(8) as f32, s.rows as f32);
                    let (tw, th) = (tw as f32, th as f32);
                    let (hs, ht) = (0.5 / (tw * sc as f32), 0.5 / (th * sc as f32));
                    (
                        [tw / iw, 0.0, -th / ir, (row as f32 + th) / ir],
                        [hs, 1.0 - hs, ht, 1.0 - ht],
                        if sampling.wrap == 2 { 2.0 } else { 0.0 },
                        Sampling { wrap: 0, wrap_t: 0, sharp: false, ..sampling },
                    )
                }
            };
            p[5 + u] = transform;
            p[8 + u] = clamp;
            p[11][u] = wrap;
            samplers[u] = self.sampler(sampling)?;
        }
        for (i, st) in dr.combiner.stages.iter().enumerate() {
            let nib = |v: [u8; 3]| v[0] as u32 | (v[1] as u32) << 4 | (v[2] as u32) << 8;
            let srcs = nib(st.src_rgb) | nib(st.src_a) << 12;
            let ops = nib(st.op_rgb) | nib(st.op_a) << 12;
            let cs = st.comb_rgb as u32 | (st.comb_a as u32) << 4 | (st.scale_rgb as u32 & 3) << 8 | (st.scale_a as u32 & 3) << 10;
            p[12 + 2 * i] = [srcs as f32, ops as f32, cs as f32, 0.0];
            p[13 + 2 * i] = st.konst.map(|c| c as f32 / 255.0);
        }
        p[24] = [dr.combiner.update_rgb as f32, dr.combiner.update_a as f32, 0.0, 0.0];
        p[25] = dr.combiner.buffer.map(|c| c as f32 / 255.0);
        p[26] = [dr.alpha_test as i32 as f32, dr.alpha_func as f32, dr.alpha_ref as f32, 0.0];
        let pbytes: Vec<u8> = p.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let poff = self.post_put(&pbytes);
        let voff = self.post_put(&dr.vertices);
        let ibytes: Vec<u8> = dr.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let ioff = self.post_put(&ibytes);
        let pipeline = self.post_pipeline(PipeKey { blend: dr.blend, mask: dr.mask & 0xF })?;
        let buffer = self.post.as_ref().unwrap().buffer.as_ref().unwrap().buffer;
        let set_layouts = [self.set_layout];
        let set = unsafe { self.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.descriptor_pool).set_layouts(&set_layouts)) }
            .map_err(vkerr("post set"))?[0];
        let binfo = [vk::DescriptorBufferInfo::default().buffer(buffer).offset(poff).range(PARAMS as u64)];
        let iinfo: Vec<[vk::DescriptorImageInfo; 1]> = (0..3).map(|u| [vk::DescriptorImageInfo::default().image_view(views[u]).image_layout(layouts[u])]).collect();
        let sinfo: Vec<[vk::DescriptorImageInfo; 1]> = (0..3).map(|u| [vk::DescriptorImageInfo::default().sampler(samplers[u])]).collect();
        let mut writes = vec![vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&binfo)];
        for u in 0..3 {
            writes.push(vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1 + u as u32).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&iinfo[u]));
            writes.push(vk::WriteDescriptorSet::default().dst_set(set).dst_binding(4 + u as u32).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sinfo[u]));
        }
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        // the pass, the viewport and the scissor
        let (viewport, scissor) = match dr.target {
            Target::Display => {
                unsafe {
                    self.device.cmd_begin_render_pass(
                        self.commands,
                        &vk::RenderPassBeginInfo::default()
                            .render_pass(self.render_pass)
                            .framebuffer(display_fb)
                            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }),
                        vk::SubpassContents::INLINE,
                    );
                }
                let (dw, fb_h) = (crate::prim::display_size(disp as u32).0 as f32, crate::prim::display_size(disp as u32).1 as f32);
                let sx = width as f32 / fb_h;
                let upright = |r: [i32; 4]| -> [f32; 4] {
                    let (x, y, w, h) = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
                    // framebuffer x runs up the upright target
                    [(fb_h - (y + h)) * sx, (dw - x - w) * scale, h * sx, w * scale]
                };
                (upright(dr.viewport), upright(dr.scissor.unwrap_or(dr.viewport)))
            }
            Target::Surface { desc, row, rows } => {
                self.begin_surface_pass(desc.key);
                let s = self.scale_of(desc.key, sc) as f32; // gfx-2d
                let mem = |r: [i32; 4]| -> [f32; 4] {
                    let (x, y, w, h) = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
                    [x * s, (row as f32 + rows as f32 - y - h) * s, w * s, h * s]
                };
                (mem(dr.viewport), mem(dr.scissor.unwrap_or([0, 0, desc.key.width as i32, rows as i32])))
            }
        };
        let (tw, th) = match dr.target {
            Target::Display => (width, height),
            Target::Surface { desc, .. } => {
                let s = &self.post.as_ref().unwrap().surfaces[&desc.key];
                (s.width, s.height)
            }
        };
        let x0 = (scissor[0] as i32).clamp(0, tw as i32);
        let y0 = (scissor[1] as i32).clamp(0, th as i32);
        let x1 = ((scissor[0] + scissor[2]) as i32).clamp(x0, tw as i32);
        let y1 = ((scissor[1] + scissor[3]) as i32).clamp(y0, th as i32);
        unsafe {
            let (d, c) = (&self.device, self.commands);
            d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, pipeline);
            d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[set], &[]);
            d.cmd_push_constants(c, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, &0u32.to_le_bytes());
            d.cmd_set_viewport(c, 0, &[vk::Viewport { x: viewport[0], y: viewport[1], width: viewport[2], height: viewport[3], min_depth: 0.0, max_depth: 1.0 }]);
            d.cmd_set_scissor(c, 0, &[vk::Rect2D { offset: vk::Offset2D { x: x0, y: y0 }, extent: vk::Extent2D { width: (x1 - x0) as u32, height: (y1 - y0) as u32 } }]);
            d.cmd_set_blend_constants(c, &dr.blend_color.map(|v| v as f32 / 255.0));
            d.cmd_bind_vertex_buffers(c, 0, &[buffer], &[voff]);
            d.cmd_bind_index_buffer(c, buffer, ioff, vk::IndexType::UINT32);
            d.cmd_draw_indexed(c, dr.indices.len() as u32, 1, 0, 0, 0);
            d.cmd_end_render_pass(c);
        }
        Ok(())
    }
}
