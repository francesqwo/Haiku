//! msaa: the options' Anti-Aliasing (`pc.msaa`: Off, 2x, 4x, 8x) in the
//! native renderer.
//!
//! A display's frame is drawn into multisampled colour and depth images
//! (the scratch, one pair per renderer, as large as the largest frame it
//! drew) and resolved into the display's single-sample target, which stays
//! what every reader takes: the presented images, read-backs, the next
//! frame drawn without a clear (`Start::Keep`), pictures in between and
//! the post effects' captures. So nothing outside `Renderer::render` sees
//! the samples.
//!
//! - **Start.** A clear clears the samples (and the depth samples to the
//!   frame's depth clear). Any other start (the previous picture, pixels,
//!   the emulated picture under a hybrid) is drawn into every sample from
//!   the single-sample target (`load`, an exact texel fetch) or from the
//!   emulated view (the hybrid's blit at the sample count).
//! - **Post effects** (bloom) read and draw the single-sample target: at a
//!   post draw the samples are resolved into it, the effect runs as it did,
//!   and its result is loaded back into every sample before the frame goes
//!   on. The depth samples are kept across it.
//! - **Alpha-tested 3D** (foliage, hair, fences) is shaded per sample
//!   (sample-rate shading), so the alpha test cuts each sample apart and
//!   its edges are smoothed too, not only the triangles' edges: all
//!   samples up to 2x render scale, 4 up to 4x, 2 above (shaded_samples).
//!   FATES_GFX_MSAA_SHADING=f sets the fraction of samples shaded (0 off).
//! - Offscreen targets (shadow maps, unit-icon captures) stay single
//!   sample: their texels are read back as values, not looked at.
//! - With Off (1 sample) none of this runs: the frame is drawn exactly as
//!   before.
//!
//! The sample count is read each frame (`zakuro_core::native_gfx::msaa`,
//! which the frontend sets from the menu), so a change takes at once.
//! FATES_GFX_MSAA=n overrides it (headless tests).

use std::collections::HashMap;

use ash::vk;

use super::{glsl_to_spirv, vkerr, Image, Renderer};

const LOAD_VERTEX_GLSL: &str = r#"#version 450
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// the single-sample target's texel under the pixel, into each sample
const LOAD_FRAGMENT_GLSL: &str = r#"#version 450
layout(set = 0, binding = 0) uniform texture2D base;
layout(set = 0, binding = 1) uniform sampler samp;
layout(location = 0) out vec4 o_color;
void main() {
    o_color = texelFetch(sampler2D(base, samp), ivec2(gl_FragCoord.xy), 0);
}
"#;

struct Scratch {
    samples: u32,
    width: u32,
    height: u32,
    color: Image,
    depth: Image,
    framebuffer: vk::Framebuffer,
    /// the single-sample target (and size) its samples were last resolved
    /// into: a frame that keeps that target's picture keeps its samples
    holds: Option<(vk::Image, u32, u32)>,
}

pub(super) struct MsaaGpu {
    /// a render pass per sample count (pipelines are made for one)
    passes: HashMap<u32, vk::RenderPass>,
    /// per sample count: the load from the single-sample target, the
    /// hybrid's blit
    loads: HashMap<u32, vk::Pipeline>,
    blits: HashMap<u32, vk::Pipeline>,
    load_modules: [vk::ShaderModule; 2],
    /// shaded pipelines by key, sample count and per-sample shading
    shaded: HashMap<(crate::scene::Key, u32, u32), vk::Pipeline>,
    scratch: Option<Scratch>,
    /// the counts the device draws colour and depth with (bits)
    supported: u32,
    /// sample-rate shading is enabled on the device
    sample_shading: bool,
    /// the samples a pixel alpha-tested draws are shaded at this frame (0, 1: once)
    shade: u32,
}

/// FATES_GFX_MSAA=n: the sample count, over the setting (dev, headless).
fn forced() -> Option<u32> {
    static FORCED: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| crate::perf::var("FATES_GFX_MSAA").ok().and_then(|v| v.trim().parse().ok()))
}

/// FATES_GFX_MSAA_SHADING=f: the fraction of an alpha-tested draw's
/// samples shaded apart, over shaded_samples' choice (0: none, 1: all).
fn shading_forced() -> Option<f32> {
    static F: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *F.get_or_init(|| crate::perf::var("FATES_GFX_MSAA_SHADING").ok().and_then(|v| v.trim().parse::<f32>().ok()).map(|f| f.clamp(0.0, 1.0)))
}

/// how many of a pixel's samples an alpha-tested draw is shaded at, at
/// render scale `scale`: shading per sample costs its fragments that many
/// times over, and a high render scale already has many pixels to a 3DS
/// pixel, so fewer are needed (and affordable) there. Measured on the
/// prologue battle's trees and grass (dev/reports/msaa.md): at 8x, 2
/// shaded samples look the same as 8 on the monitor for a quarter of the
/// cost (+1.0 ms a top frame against +5.1 ms).
/// - up to 2x: every sample;
/// - up to 4x: 4;
/// - above: 2.
fn shaded_samples(samples: u32, scale: f32) -> u32 {
    if let Some(f) = shading_forced() {
        return (f * samples as f32).ceil() as u32;
    }
    let cap = if scale <= 2.5 { 8 } else if scale <= 4.5 { 4 } else { 2 };
    samples.min(cap)
}

/// perf3: the most samples a picture in between is drawn with, against the
/// setting's `want`. FATES_GFX_MSAA_BETWEEN=n sets it (dev).
fn between_samples(want: u32) -> u32 {
    static CAP: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    CAP.get_or_init(|| crate::perf::var("FATES_GFX_MSAA_BETWEEN").ok().and_then(|v| v.trim().parse().ok())).unwrap_or(want)
}

/// whether a shaded draw has the alpha test on (raster.frag's Draw block:
/// misc.z, word 50, is GPUREG_FRAGOP_ALPHA_TEST; a test of "always" cuts
/// nothing)
pub(super) fn alpha_tested(s: &crate::scene::Shaded) -> bool {
    s.draw.get(50).is_some_and(|&v| v & 1 != 0 && (v >> 4) & 7 != 1)
}

impl Renderer {
    /// the sample count display `disp`'s frame is drawn with now: the
    /// setting, at most what the device has; 1 for offscreen targets.
    /// (`scale`: the frame's render scale, for how many samples alpha-tested
    /// draws are shaded at)
    pub(super) fn msaa_samples(&mut self, disp: usize, scale: f32) -> u32 {
        if disp > 1 {
            return 1;
        }
        let want = forced().unwrap_or_else(zakuro_core::native_gfx::msaa);
        if want <= 1 {
            // turned off in the menu: the samples' memory goes back
            if self.msaa.as_ref().is_some_and(|m| m.scratch.is_some()) {
                self.msaa_drop_scratch();
                eprintln!("gfx-msaa: {} draws with one sample", self.name);
            }
            return 1;
        }
        if self.msaa.is_none() && self.msaa_init().map_err(|e| eprintln!("gfx-msaa: {e}")).is_err() {
            return 1;
        }
        // perf3: pictures in between at most between_samples()
        // (sharp: and the frames, while they are drawn at the pictures in
        // between's smaller scale, so every picture has the same samples)
        let want = if self.between || crate::hfr::frames_reduced() { want.min(between_samples(want)).min(zakuro_core::native_gfx::between_samples().unwrap_or(want)) } else { want };
        let m = self.msaa.as_mut().unwrap();
        let samples = [8u32, 4, 2].into_iter().find(|&n| n <= want && m.supported & n != 0).unwrap_or(1);
        m.shade = if m.sample_shading { shaded_samples(samples, scale) } else { 0 };
        samples
    }

    fn msaa_init(&mut self) -> Result<(), String> {
        let props = unsafe { self.instance.get_physical_device_properties(self.physical) };
        let features = unsafe { self.instance.get_physical_device_features(self.physical) };
        let supported = (props.limits.framebuffer_color_sample_counts & props.limits.framebuffer_depth_sample_counts).as_raw();
        let vs = glsl_to_spirv(LOAD_VERTEX_GLSL, naga::ShaderStage::Vertex)?;
        let fs = glsl_to_spirv(LOAD_FRAGMENT_GLSL, naga::ShaderStage::Fragment)?;
        let load_modules = unsafe {
            [
                self.device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vs), None).map_err(vkerr("msaa load vs"))?,
                self.device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&fs), None).map_err(vkerr("msaa load fs"))?,
            ]
        };
        eprintln!("gfx-msaa: {} can draw with {:?} samples, per-sample shading {}", self.name, [2u32, 4, 8, 16].iter().filter(|&&n| supported & n != 0).collect::<Vec<_>>(), features.sample_rate_shading == vk::TRUE);
        self.msaa = Some(MsaaGpu {
            passes: HashMap::new(),
            loads: HashMap::new(),
            blits: HashMap::new(),
            load_modules,
            shaded: HashMap::new(),
            scratch: None,
            supported,
            sample_shading: features.sample_rate_shading == vk::TRUE,
            shade: 0,
        });
        Ok(())
    }

    /// the render pass for `samples`: as the renderer's own, multisampled.
    pub(super) fn msaa_pass(&mut self, samples: u32) -> Result<vk::RenderPass, String> {
        if let Some(&p) = self.msaa.as_ref().unwrap().passes.get(&samples) {
            return Ok(p);
        }
        let count = vk::SampleCountFlags::from_raw(samples);
        let attachments = [
            vk::AttachmentDescription::default()
                .format(vk::Format::R8G8B8A8_UNORM)
                .samples(count)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(vk::Format::D32_SFLOAT)
                .samples(count)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        ];
        let color_ref = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_ref = vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpass = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref)
            .depth_stencil_attachment(&depth_ref)];
        let pass = unsafe { self.device.create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpass), None) }
            .map_err(vkerr("msaa render pass"))?;
        self.msaa.as_mut().unwrap().passes.insert(samples, pass);
        Ok(pass)
    }

    fn msaa_image(&self, width: u32, height: u32, samples: u32, format: vk::Format, usage: vk::ImageUsageFlags, aspect: vk::ImageAspectFlags) -> Result<Image, String> {
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
                        .samples(vk::SampleCountFlags::from_raw(samples))
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(usage)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(vkerr("msaa image"))?;
            let req = self.device.get_image_memory_requirements(image);
            let ty = self.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
            let memory = match self.device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) {
                Ok(m) => m,
                Err(e) => {
                    self.device.destroy_image(image, None);
                    return Err(format!("msaa image memory: {e:?}"));
                }
            };
            self.device.bind_image_memory(image, memory, 0).map_err(vkerr("bind msaa image"))?;
            let view = self
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: aspect,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                    None,
                )
                .map_err(vkerr("msaa image view"))?;
            Ok(Image { image, memory, view })
        }
    }

    fn msaa_drop_scratch(&mut self) {
        if let Some(s) = self.msaa.as_mut().and_then(|m| m.scratch.take()) {
            let _ = zakuro_gpu::wait_idle(&self.device); // as ensure_target: keeps off the present thread
            unsafe { self.device.destroy_framebuffer(s.framebuffer, None) };
            self.destroy_image(&s.color);
            self.destroy_image(&s.depth);
        }
    }

    /// the scratch at `samples`, at least `width` x `height` (it grows to
    /// the largest frame drawn, so the two displays and the pictures in
    /// between at a smaller scale share it).
    pub(super) fn msaa_ensure(&mut self, width: u32, height: u32, samples: u32) -> Result<(), String> {
        let (mut w, mut h) = (width, height);
        if let Some(s) = self.msaa.as_ref().unwrap().scratch.as_ref() {
            if s.samples == samples && s.width >= width && s.height >= height {
                return Ok(());
            }
            if s.samples == samples {
                (w, h) = (w.max(s.width), h.max(s.height));
            }
        }
        let _smooth = super::Realloc::new(4, w, h); // smooth: in the timing log (4: msaa scratch)
        self.msaa_drop_scratch();
        let pass = self.msaa_pass(samples)?;
        let color = self.msaa_image(w, h, samples, vk::Format::R8G8B8A8_UNORM, vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST, vk::ImageAspectFlags::COLOR)?;
        let depth = match self.msaa_image(w, h, samples, vk::Format::D32_SFLOAT, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST, vk::ImageAspectFlags::DEPTH) {
            Ok(d) => d,
            Err(e) => {
                self.destroy_image(&color);
                return Err(e);
            }
        };
        let views = [color.view, depth.view];
        let framebuffer = unsafe { self.device.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(pass).attachments(&views).width(w).height(h).layers(1), None) }
            .map_err(vkerr("msaa framebuffer"))?;
        eprintln!("gfx-msaa: {} draws at {samples} samples, {w}x{h} ({} MB)", self.name, (w as u64 * h as u64 * samples as u64 * 8) >> 20);
        self.msaa.as_mut().unwrap().scratch = Some(Scratch { samples, width: w, height: h, color, depth, framebuffer, holds: None });
        Ok(())
    }

    /// a full-screen pipeline into `samples`: the load (`load`) or the
    /// hybrid's blit of the emulated picture.
    fn msaa_fill_pipeline(&mut self, samples: u32, load: bool) -> Result<vk::Pipeline, String> {
        let m = self.msaa.as_ref().unwrap();
        if let Some(&p) = if load { m.loads.get(&samples) } else { m.blits.get(&samples) } {
            return Ok(p);
        }
        let modules = if load { m.load_modules } else { self.blit_modules };
        let pass = self.msaa_pass(samples)?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(modules[0]).name(c"main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(modules[1]).name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::from_raw(samples));
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
            .layout(self.blit_layout)
            .render_pass(pass)];
        let p = unsafe { self.device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) }.map_err(|(_, e)| format!("msaa fill pipeline: {e:?}"))?[0];
        let m = self.msaa.as_mut().unwrap();
        if load {
            m.loads.insert(samples, p);
        } else {
            m.blits.insert(samples, p);
        }
        Ok(p)
    }

    /// a shaded draw's pipeline at `samples`, alpha-tested draws shaded
    /// per sample.
    pub(super) fn msaa_shaded_pipeline(&mut self, s: &crate::scene::Shaded, samples: u32) -> Result<vk::Pipeline, String> {
        let m = self.msaa.as_ref().unwrap();
        // drawdist: far grass in batches is not shaded per sample
        let shade = if alpha_tested(s) && m.shade > 1 && !crate::drawdist::far_batch(s) { m.shade.min(samples) } else { 0 };
        // drawdist: a batch of tufts has its own shade.vert, so its own pipelines
        let batch = crate::drawdist::is_batch(s);
        let slot = shade | (batch as u32) << 16;
        if let Some(&p) = m.shaded.get(&(s.key, samples, slot)) {
            return Ok(p);
        }
        let _t = crate::perf::scope(crate::perf::C::Pipeline);
        let pass = self.msaa_pass(samples)?;
        let p = self.shaded_make_at(s.key, pass, samples, shade as f32 / samples as f32, batch)?;
        self.msaa.as_mut().unwrap().shaded.insert((s.key, samples, slot), p);
        Ok(p)
    }

    /// starts drawing a frame into the samples (outside a render pass, the
    /// single-sample target `one` in the colour-attachment layout holding
    /// the frame's start unless it starts from `view`): the depth samples
    /// cleared to `depth_clear`; the colour samples cleared to `clear`, or
    /// drawn from the target, or from the emulated picture's `view`, or, for
    /// a frame that keeps the previous picture (`keep`) of the target the
    /// samples were last resolved into, kept as they are. Ends inside the
    /// multisampled render pass. Returns the target's layout.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn msaa_begin(&mut self, samples: u32, width: u32, height: u32, one: (vk::Image, vk::ImageView), clear: Option<[f32; 4]>, keep: bool, depth_clear: f32, view: Option<(vk::DescriptorSet, [f32; 4])>) -> Result<vk::ImageLayout, String> {
        let (color, depth, kept) = {
            let s = self.msaa.as_mut().unwrap().scratch.as_mut().unwrap();
            let kept = keep && clear.is_none() && view.is_none() && s.holds == Some((one.0, width, height));
            s.holds = None;
            (s.color.image, s.depth.image, kept)
        };
        let range = |aspect| vk::ImageSubresourceRange { aspect_mask: aspect, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        self.barrier(depth, vk::ImageAspectFlags::DEPTH, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        unsafe {
            self.device.cmd_clear_depth_stencil_image(self.commands, depth, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearDepthStencilValue { depth: depth_clear, stencil: 0 }, &[range(vk::ImageAspectFlags::DEPTH)]);
        }
        self.barrier(depth, vk::ImageAspectFlags::DEPTH, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        if let Some(rgba) = clear {
            self.barrier(color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            unsafe {
                self.device.cmd_clear_color_image(self.commands, color, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: rgba }, &[range(vk::ImageAspectFlags::COLOR)]);
            }
            self.barrier(color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            self.msaa_pass_begin(width, height)?;
            return Ok(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        }
        if kept {
            // the samples the last resolve made the kept picture from
            self.msaa_pass_begin(width, height)?;
            return Ok(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        }
        self.barrier(color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        match view {
            Some((set, area)) => {
                let pipeline = self.msaa_fill_pipeline(samples, false)?;
                self.msaa_pass_begin(width, height)?;
                let bytes: Vec<u8> = area.iter().flat_map(|v| v.to_le_bytes()).collect();
                unsafe {
                    let (d, c) = (&self.device, self.commands);
                    d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, self.blit_layout, 0, &[set], &[]);
                    d.cmd_push_constants(c, self.blit_layout, vk::ShaderStageFlags::VERTEX, 0, &bytes);
                    d.cmd_draw(c, 3, 1, 0, 0);
                }
                Ok(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            }
            None => {
                self.msaa_load(samples, width, height, one)?;
                Ok(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            }
        }
    }

    /// begins the multisampled render pass over `width` x `height`, with
    /// the viewport and scissor over it.
    fn msaa_pass_begin(&mut self, width: u32, height: u32) -> Result<(), String> {
        let (pass, framebuffer) = {
            let m = self.msaa.as_ref().unwrap();
            let s = m.scratch.as_ref().unwrap();
            (m.passes[&s.samples], s.framebuffer)
        };
        let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } };
        unsafe {
            let (d, c) = (&self.device, self.commands);
            d.cmd_begin_render_pass(c, &vk::RenderPassBeginInfo::default().render_pass(pass).framebuffer(framebuffer).render_area(area), vk::SubpassContents::INLINE);
            d.cmd_set_viewport(c, 0, &[vk::Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }]);
            d.cmd_set_scissor(c, 0, &[area]);
        }
        Ok(())
    }

    /// the single-sample target (colour-attachment layout, left in the
    /// shader-read layout) drawn into every sample, inside the
    /// multisampled pass it begins.
    fn msaa_load(&mut self, samples: u32, width: u32, height: u32, one: (vk::Image, vk::ImageView)) -> Result<(), String> {
        let pipeline = self.msaa_fill_pipeline(samples, true)?;
        let nearest = self.sampler(crate::prim::Sampling { wrap: 0, wrap_t: 0, sharp: false, linear_mag: false, linear_min: false })?;
        let layouts = [self.blit_set_layout];
        let set = unsafe { self.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.descriptor_pool).set_layouts(&layouts)) }
            .map_err(vkerr("msaa load set"))?[0];
        let image_info = [vk::DescriptorImageInfo::default().image_view(one.1).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(nearest)];
        let writes = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&image_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler_info),
        ];
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        self.barrier(one.0, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        self.msaa_pass_begin(width, height)?;
        unsafe {
            let (d, c) = (&self.device, self.commands);
            d.cmd_bind_pipeline(c, vk::PipelineBindPoint::GRAPHICS, pipeline);
            d.cmd_bind_descriptor_sets(c, vk::PipelineBindPoint::GRAPHICS, self.blit_layout, 0, &[set], &[]);
            d.cmd_draw(c, 3, 1, 0, 0);
        }
        Ok(())
    }

    /// after a post effect changed the single-sample target (left in the
    /// colour-attachment layout): its picture into every sample, the frame
    /// going on in the multisampled pass. Returns the target's layout.
    pub(super) fn msaa_resume(&mut self, samples: u32, width: u32, height: u32, one: (vk::Image, vk::ImageView)) -> Result<vk::ImageLayout, String> {
        self.msaa_load(samples, width, height, one)?;
        Ok(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
    }

    /// the samples resolved into the single-sample target (outside the
    /// render pass; the target from `layout`, left in the colour-attachment
    /// layout, as the renderer's own pass leaves it).
    pub(super) fn msaa_resolve(&mut self, width: u32, height: u32, one: vk::Image, layout: vk::ImageLayout) {
        let color = {
            let s = self.msaa.as_mut().unwrap().scratch.as_mut().unwrap();
            s.holds = Some((one, width, height));
            s.color.image
        };
        self.barrier(color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        self.barrier(one, vk::ImageAspectFlags::COLOR, layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let layers = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
        let region = vk::ImageResolve::default().src_subresource(layers).dst_subresource(layers).extent(vk::Extent3D { width, height, depth: 1 });
        unsafe {
            self.device.cmd_resolve_image(self.commands, color, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, one, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
        }
        self.barrier(color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        self.barrier(one, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    }

    pub(super) fn msaa_destroy(&mut self) {
        self.msaa_drop_scratch();
        if let Some(m) = self.msaa.take() {
            unsafe {
                for p in m.loads.values().chain(m.blits.values()).chain(m.shaded.values()) {
                    self.device.destroy_pipeline(*p, None);
                }
                for p in m.passes.values() {
                    self.device.destroy_render_pass(*p, None);
                }
                for module in m.load_modules {
                    self.device.destroy_shader_module(module, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shaders_translate() {
        super::glsl_to_spirv(super::LOAD_VERTEX_GLSL, naga::ShaderStage::Vertex).unwrap();
        super::glsl_to_spirv(super::LOAD_FRAGMENT_GLSL, naga::ShaderStage::Fragment).unwrap();
    }
}

/// msaa: FATES_GFX_GPU_TIME=1 times each display frame's commands on the
/// GPU (timestamps around the drawing and the resolve, not the texture
/// uploads or the copy to the presented image), printed per display, size
/// and sample count every 120 frames as `gfx-gpu-time`, over all of that
/// key's frames so far (the last line is the run's). Dev, for the
/// performance table.
pub(super) struct GpuTimer {
    pool: vk::QueryPool,
    /// nanoseconds a tick
    period: f64,
    pending: Option<(usize, u32, u32, u32)>,
    /// perf3: the pending frame's draw count
    draws: usize,
    times: HashMap<(usize, u32, u32, u32), Vec<f64>>,
}

fn gpu_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_GPU_TIME").is_ok_and(|v| v == "1"))
}

impl Renderer {
    /// at the start of a frame's commands (after `begin`): the last
    /// frame's time taken in, the first timestamp written.
    pub(super) fn gpu_time_start(&mut self) {
        if !gpu_timing() {
            return;
        }
        if self.gpu_timer.is_none() {
            let period = unsafe { self.instance.get_physical_device_properties(self.physical) }.limits.timestamp_period as f64;
            let Ok(pool) = (unsafe { self.device.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2), None) }) else { return };
            self.gpu_timer = Some(GpuTimer { pool, period, pending: None, draws: 0, times: HashMap::new() });
        }
        let t = self.gpu_timer.as_mut().unwrap();
        if let Some(key) = t.pending.take() {
            let mut ticks = [0u64; 2];
            if unsafe { self.device.get_query_pool_results(t.pool, 0, &mut ticks, vk::QueryResultFlags::TYPE_64) }.is_ok() {
                let us = ticks[1].wrapping_sub(ticks[0]) as f64 * t.period / 1000.0;
                // perf3: each frame's GPU time in the smooth log too ("gpu":
                // a = display << 48 | width << 24 | draws, b = us), so it can
                // be put beside the game's frames (dev/perf3/gpu.py)
                zakuro_gpu::smooth::point("gpu", std::time::Instant::now(), ((key.0 as i64) << 48) | ((key.1 as i64) << 24) | (t.draws as i64 & 0xFF_FFFF), us);
                let list = t.times.entry(key).or_default();
                list.push(us);
                // every 120 frames, all of this key's so far
                if list.len() % 120 == 0 {
                    let mut v = list.clone();
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let mean = v.iter().sum::<f64>() / v.len() as f64;
                    eprintln!(
                        "gfx-gpu-time: {} disp {} {}x{} samples {}: mean {:.0} us, p50 {:.0}, p90 {:.0}, max {:.0} over {}",
                        self.name, key.0, key.1, key.2, key.3, mean, v[v.len() / 2], v[v.len() * 9 / 10], v[v.len() - 1], v.len()
                    );
                }
            }
        }
        let t = self.gpu_timer.as_ref().unwrap();
        unsafe {
            self.device.cmd_reset_query_pool(self.commands, t.pool, 0, 2);
            self.device.cmd_write_timestamp(self.commands, vk::PipelineStageFlags::TOP_OF_PIPE, t.pool, 0);
        }
    }

    /// after the frame's drawing and resolve.
    pub(super) fn gpu_time_stop(&mut self, disp: usize, width: u32, height: u32, samples: u32, draws: usize) {
        let Some(t) = self.gpu_timer.as_mut().filter(|_| gpu_timing()) else { return };
        unsafe { self.device.cmd_write_timestamp(self.commands, vk::PipelineStageFlags::BOTTOM_OF_PIPE, t.pool, 1) };
        t.pending = Some((disp, width, height, samples));
        t.draws = draws;
    }

    pub(super) fn gpu_time_destroy(&mut self) {
        if let Some(t) = self.gpu_timer.take() {
            unsafe { self.device.destroy_query_pool(t.pool, None) };
        }
    }
}
