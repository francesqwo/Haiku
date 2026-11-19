//! pulse2: Divine Pulse's effects, drawn by the presenter over the top
//! screen in place of its plain picture while one is on (see
//! [`super::PulseEffect`]):
//!
//! - the time-stopped grade: the picture desaturated toward the rewind
//!   screen's green, with darkened edges;
//! - the activation wave: a ring of green light with a clock face's twelve
//!   marks spreading from the middle, bending the picture where it passes,
//!   the grade following it;
//! - the rewind's cross-fade: the picture shown before a moment was loaded,
//!   kept, fading out over the new one, smeared sideways along the way time
//!   went.
//!
//! The old picture is kept by drawing the top screen (cross-fade included,
//! grade and wave left out) into one of two images the size of the window
//! after each present's own pass; when the serial changes the two swap, so
//! the old one is the last picture shown, even halfway through a fade.
//! Both shaders are GLSL compiled with naga when the presenter starts; if
//! that fails the effect is simply left out (the rewind screen's own tint
//! still shows).

use ash::vk;

use super::vulkan::{find_memory_type, vk_fail};
use super::{PresentError, PulseEffect, Viewport};

const VERTEX: &str = r#"#version 450
layout(location = 0) out vec2 uv;
layout(push_constant) uniform Fx {
    ivec4 head;
    vec4 area;
    vec4 bounds;
    vec4 vp;
    vec4 mixes;
    vec4 ring;
    vec4 more;
} fx;
void main() {
    vec2 position = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    vec2 at = position;
    if (fx.head.x == 90) {
        at = vec2(position.y, 1.0 - position.x);
    } else if (fx.head.x == 180) {
        at = vec2(1.0 - position.x, 1.0 - position.y);
    } else if (fx.head.x == 270) {
        at = vec2(1.0 - position.y, position.x);
    }
    uv = at;
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"#version 450
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 colour;
layout(set = 0, binding = 0) uniform texture2D live_t;
layout(set = 0, binding = 1) uniform sampler samp;
layout(set = 0, binding = 2) uniform texture2D held_t;
// head: orientation, filter mode, the old picture is there, the pass (0
// shown, 1 kept: no grade, no wave). area and bounds: where the top screen
// lies in its image. vp: the top screen in the window, in pixels.
// mixes: cross-fade, streak, grade, the grade inside the wave only.
// ring: radius (1 the corners), width, strength, flash.
// more: time, vignette, the echo's strength, unused.
layout(push_constant) uniform Fx {
    ivec4 head;
    vec4 area;
    vec4 bounds;
    vec4 vp;
    vec4 mixes;
    vec4 ring;
    vec4 more;
} fx;

vec3 live_at(vec2 at_uv) {
    vec2 at = fx.area.xy + at_uv * fx.area.zw;
    vec2 size = vec2(textureSize(sampler2D(live_t, samp), 0));
    vec2 covered = fwidth(at) * size;
    if (max(covered.x, covered.y) <= 1.001) {
        vec2 texel = at * size;
        if (fx.head.y == 1) {
            at = (floor(texel) + 0.5) / size;
        } else if (fx.head.y == 2) {
            vec2 scale = 1.0 / max(covered, vec2(0.000001));
            vec2 centred = texel - 0.5;
            vec2 blend = clamp((fract(centred) - 0.5) * scale + 0.5, 0.0, 1.0);
            at = (floor(centred) + 0.5 + blend) / size;
        }
        return texture(sampler2D(live_t, samp), clamp(at, fx.bounds.xy, fx.bounds.zw)).rgb;
    }
    int tx = int(clamp(ceil(covered.x), 1.0, 4.0));
    int ty = int(clamp(ceil(covered.y), 1.0, 4.0));
    vec3 sum = vec3(0.0);
    for (int y = 0; y < ty; y++) {
        for (int x = 0; x < tx; x++) {
            vec2 offset = ((vec2(float(x), float(y)) + 0.5) / vec2(float(tx), float(ty)) - 0.5) * covered / size;
            sum += texture(sampler2D(live_t, samp), clamp(at + offset, fx.bounds.xy, fx.bounds.zw)).rgb;
        }
    }
    return sum / float(tx * ty);
}

vec3 held_at(vec2 px) {
    vec2 size = vec2(textureSize(sampler2D(held_t, samp), 0));
    vec2 p = clamp(px, fx.vp.xy + 0.5, fx.vp.xy + fx.vp.zw - 0.5);
    return texture(sampler2D(held_t, samp), p / size).rgb;
}

void main() {
    vec2 px = gl_FragCoord.xy;
    vec2 p = (px - fx.vp.xy) / fx.vp.zw;
    float aspect = fx.vp.z / fx.vp.w;
    vec2 d = (p - 0.5) * vec2(aspect, 1.0);
    float r = length(d);
    vec2 dir = vec2(0.0);
    if (r > 0.0001) {
        dir = d / r;
    }
    bool kept = fx.head.w == 1;
    // the wave, in screen heights: 1 reaches the corners
    float reach = length(vec2(aspect, 1.0)) * 0.5;
    float w = max(fx.ring.y, 0.001);
    float radius = fx.ring.x * (reach + w);
    float strength = fx.ring.z;
    float band = (r - radius) / w;
    float ring = strength * exp(-band * band);
    float band2 = (r - radius * 0.7) / (w * 0.8);
    float echo = strength * fx.more.z * exp(-band2 * band2);
    if (kept) {
        ring = 0.0;
        echo = 0.0;
    }
    // the picture bends at the wave front, outward ahead of it, inward behind
    float bend = -(ring * band + 0.6 * echo * band2);
    vec2 shift = dir * bend * 0.012;
    vec3 col = live_at(uv - shift / vec2(aspect, 1.0));
    // the old picture fading out, smeared along the way time went
    float cf = fx.head.z != 0 ? fx.mixes.x : 0.0;
    if (cf > 0.001) {
        vec3 old = vec3(0.0);
        float total = 0.0;
        for (int i = 0; i < 8; i++) {
            float k = float(i) / 7.0;
            float weight = 1.0 - 0.6 * k;
            vec2 q = px - shift * fx.vp.w + vec2(fx.mixes.y * k * fx.vp.z, 0.0);
            old += held_at(q) * weight;
            total += weight;
        }
        col = mix(col, old / total, cf);
    }
    if (!kept) {
        // time stopped: desaturated toward Divine Pulse's green
        float g = fx.mixes.z;
        if (fx.mixes.w > 0.5) {
            g *= smoothstep(radius + w, radius - 0.5 * w, r);
        }
        float lum = dot(col, vec3(0.299, 0.587, 0.114));
        vec3 green = vec3(lum) * vec3(0.80, 1.06, 0.97) + vec3(0.0, 0.05, 0.04);
        col = mix(col, green, g * 0.72) * (1.0 - 0.06 * g);
        float edge = smoothstep(0.35, 0.95, length((p - 0.5) * vec2(1.0, 1.0)) * 1.25);
        col *= 1.0 - fx.more.y * edge * 0.32;
        // the wave's light, with a clock face's twelve marks turning slowly
        float angle = atan(d.y, d.x) + fx.more.x * 0.5;
        float marks = pow(max(cos(angle * 12.0), 0.0), 28.0);
        vec3 glow = vec3(0.52, 1.0, 0.74);
        col += glow * (ring * (0.26 + 0.95 * marks) + 0.3 * echo);
        col += glow * fx.ring.w * 0.32;
    }
    colour = vec4(col, 1.0);
}
"#;

fn compile(source: &str, stage: naga::ShaderStage, what: &str) -> Result<Vec<u32>, String> {
    use naga::back::spv;
    use naga::front::glsl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let run = || -> Result<Vec<u32>, String> {
        let module = glsl::Frontend::default()
            .parse(&glsl::Options::from(stage), source)
            .map_err(|errors| errors.emit_to_string_with_path(source, what))?;
        let info = Validator::new(ValidationFlags::all(), Capabilities::IMMEDIATES)
            .validate(&module)
            .map_err(|error| error.emit_to_string_with_path(source, what))?;
        let options = spv::Options { lang_version: (1, 0), flags: spv::WriterFlags::empty(), force_loop_bounding: false, ..spv::Options::default() };
        let pipeline = spv::PipelineOptions { shader_stage: stage, entry_point: "main".into() };
        spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|error| error.to_string())
    };
    std::panic::catch_unwind(run).unwrap_or_else(|_| Err("naga panicked".to_owned()))
}

fn module(device: &ash::Device, words: &[u32]) -> Result<vk::ShaderModule, PresentError> {
    unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None) }.map_err(vk_fail("creating a pulse shader"))
}

/// the push constants: 7 x 16 bytes, both stages.
const CONSTANTS: u32 = 112;

struct Held {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    framebuffer: vk::Framebuffer,
}

pub(super) struct PulseFx {
    device: ash::Device,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    format: vk::Format,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// one per frame in flight
    sets: Vec<vk::DescriptorSet>,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    held: Vec<Held>,
    extent: vk::Extent2D,
    /// the image the effect reads the old picture from; the other is drawn
    cur: usize,
    /// each image holds a picture, and has been cleared once
    filled: [bool; 2],
    cleared: bool,
    /// this present clears them first
    clear_now: bool,
    /// whether the old image holds a picture, for this present
    old_filled: bool,
    serial: u32,
    /// this present's effect, when there is one
    pub(super) effect: Option<PulseEffect>,
}

impl PulseFx {
    pub(super) fn new(
        device: &ash::Device,
        memory_properties: vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        frames: usize,
    ) -> Result<PulseFx, PresentError> {
        let vertex = compile(VERTEX, naga::ShaderStage::Vertex, "pulse.vert").map_err(PresentError::Backend)?;
        let fragment = compile(FRAGMENT, naga::ShaderStage::Fragment, "pulse.frag").map_err(PresentError::Backend)?;
        unsafe {
            let attachments = [vk::AttachmentDescription::default()
                .format(format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let color = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let subpasses = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&color)];
            let dependencies = [
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
                vk::SubpassDependency::default()
                    .src_subpass(0)
                    .dst_subpass(vk::SUBPASS_EXTERNAL)
                    .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ),
            ];
            let render_pass = device
                .create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpasses).dependencies(&dependencies), None)
                .map_err(vk_fail("creating the pulse render pass"))?;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let set_layout = device
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)
                .map_err(vk_fail("creating the pulse set layout"))?;
            let sizes = [
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(2 * frames as u32),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLER).descriptor_count(frames as u32),
            ];
            let pool = device
                .create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(frames as u32).pool_sizes(&sizes), None)
                .map_err(vk_fail("creating the pulse descriptor pool"))?;
            let layouts = vec![set_layout; frames];
            let sets = device
                .allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))
                .map_err(vk_fail("allocating the pulse sets"))?;
            let set_layouts = [set_layout];
            let ranges = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT).offset(0).size(CONSTANTS)];
            let pipeline_layout = device
                .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&ranges), None)
                .map_err(vk_fail("creating the pulse pipeline layout"))?;
            let vertex_module = module(device, &vertex)?;
            let fragment_module = module(device, &fragment)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vertex_module).name(c"main"),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fragment_module).name(c"main"),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport_state = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA).blend_enable(false)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterization)
                .multisample_state(&multisample)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0);
            let pipelines = device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None);
            device.destroy_shader_module(vertex_module, None);
            device.destroy_shader_module(fragment_module, None);
            let pipeline = pipelines.map_err(|(_, error)| vk_fail("creating the pulse pipeline")(error))?[0];
            Ok(PulseFx {
                device: device.clone(),
                memory_properties,
                format,
                render_pass,
                set_layout,
                pool,
                sets,
                pipeline_layout,
                pipeline,
                held: Vec::new(),
                extent: vk::Extent2D::default(),
                cur: 0,
                filled: [false; 2],
                cleared: false,
                clear_now: false,
                old_filled: false,
                serial: 0,
                effect: None,
            })
        }
    }

    fn destroy_held(&mut self) {
        unsafe {
            for held in self.held.drain(..) {
                self.device.destroy_framebuffer(held.framebuffer, None);
                self.device.destroy_image_view(held.view, None);
                self.device.destroy_image(held.image, None);
                self.device.free_memory(held.memory, None);
            }
        }
        self.filled = [false; 2];
        self.cleared = false;
    }

    fn make_held(&mut self, extent: vk::Extent2D) -> Result<(), PresentError> {
        let device = &self.device;
        for _ in 0..2 {
            unsafe {
                let image = device
                    .create_image(
                        &vk::ImageCreateInfo::default()
                            .image_type(vk::ImageType::TYPE_2D)
                            .format(self.format)
                            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
                            .mip_levels(1)
                            .array_layers(1)
                            .samples(vk::SampleCountFlags::TYPE_1)
                            .tiling(vk::ImageTiling::OPTIMAL)
                            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE)
                            .initial_layout(vk::ImageLayout::UNDEFINED),
                        None,
                    )
                    .map_err(vk_fail("creating a pulse image"))?;
                let requirements = device.get_image_memory_requirements(image);
                let Some(type_index) = find_memory_type(&self.memory_properties, requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL) else {
                    device.destroy_image(image, None);
                    return Err(PresentError::Backend("no device memory for the pulse images".into()));
                };
                let memory = device
                    .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(type_index), None)
                    .map_err(vk_fail("allocating a pulse image"))?;
                device.bind_image_memory(image, memory, 0).map_err(vk_fail("binding a pulse image"))?;
                let view = device
                    .create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(self.format)
                            .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)),
                        None,
                    )
                    .map_err(vk_fail("creating a pulse image view"))?;
                let attachments = [view];
                let framebuffer = device
                    .create_framebuffer(
                        &vk::FramebufferCreateInfo::default().render_pass(self.render_pass).attachments(&attachments).width(extent.width).height(extent.height).layers(1),
                        None,
                    )
                    .map_err(vk_fail("creating a pulse framebuffer"))?;
                self.held.push(Held { image, memory, view, framebuffer });
            }
        }
        self.extent = extent;
        Ok(())
    }

    /// before recording a present: the effect for it, the window's size,
    /// the images swapped when the picture is about to change, and this
    /// frame's set pointed at the top screen's picture and the old one.
    /// false: nothing to draw.
    pub(super) fn prepare(
        &mut self,
        effect: Option<PulseEffect>,
        extent: vk::Extent2D,
        frame: usize,
        live: Option<(vk::ImageView, vk::ImageLayout)>,
        sampler: vk::Sampler,
    ) -> bool {
        self.effect = None;
        let (Some(effect), Some((view, layout))) = (effect, live) else {
            return false;
        };
        if extent.width == 0 || extent.height == 0 {
            return false;
        }
        if self.extent != extent || self.held.len() != 2 {
            // the old images may still be read by frames in flight
            let _ = crate::wait_idle(&self.device);
            self.destroy_held();
            if let Err(error) = self.make_held(extent) {
                log::warn!("pulse2: no rewind effect, {error}");
                self.destroy_held();
                return false;
            }
        }
        if effect.serial != self.serial {
            self.serial = effect.serial;
            // the picture shown last is the old one now
            self.cur ^= 1;
        }
        let set = self.sets[frame % self.sets.len()];
        let images = [
            [vk::DescriptorImageInfo::default().image_view(view).image_layout(layout)],
            [vk::DescriptorImageInfo::default().image_view(self.held[self.cur].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)],
        ];
        let samplers = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let writes = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&images[0]),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&samplers),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&images[1]),
        ];
        // SAFETY: the frame's fence was waited on, so no command using the
        // set is in flight
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        self.clear_now = !self.cleared;
        self.cleared = true;
        if self.clear_now {
            self.filled = [false; 2];
        }
        self.old_filled = self.filled[self.cur];
        // keep() draws the other one this present
        self.filled[self.cur ^ 1] = true;
        self.effect = Some(effect);
        true
    }

    /// before the present's render pass: the images cleared once, so both
    /// are in the layout the shader reads them in.
    pub(super) fn before_pass(&self, command_buffer: vk::CommandBuffer) {
        if self.effect.is_none() || !self.clear_now {
            return;
        }
        let device = &self.device;
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        for held in &self.held {
            unsafe {
                let to_clear = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(held.image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(command_buffer, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[to_clear]);
                device.cmd_clear_color_image(command_buffer, held.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] }, &[range]);
                let to_read = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(held.image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(command_buffer, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[to_read]);
            }
        }
    }

    fn constants(&self, effect: &PulseEffect, viewport: Viewport, area: [f32; 8], angle: i32, mode: i32, kept: bool) -> Vec<u8> {
        let filled = self.old_filled as i32;
        let mut words: Vec<u8> = Vec::with_capacity(CONSTANTS as usize);
        for word in [angle, mode, filled, kept as i32] {
            words.extend_from_slice(&word.to_le_bytes());
        }
        let floats = [
            area[0],
            area[1],
            area[2],
            area[3],
            area[4],
            area[5],
            area[6],
            area[7],
            viewport.x,
            viewport.y,
            viewport.width,
            viewport.height,
            effect.crossfade,
            effect.streak,
            effect.grade,
            if effect.wave_grades { 1.0 } else { 0.0 },
            effect.ring[0],
            effect.ring[1],
            effect.ring[2],
            effect.flash,
            effect.time,
            effect.vignette,
            effect.echo,
            0.0,
        ];
        for value in floats {
            words.extend_from_slice(&value.to_le_bytes());
        }
        words
    }

    /// in the present's render pass, right after the top screen: the top
    /// screen again, with the effect, over it. Its viewport is set.
    pub(super) fn draw(&self, command_buffer: vk::CommandBuffer, frame: usize, viewport: Viewport, area: [f32; 8], angle: i32, mode: i32) {
        let Some(effect) = &self.effect else { return };
        let constants = self.constants(effect, viewport, area, angle, mode, false);
        unsafe {
            self.device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            self.device.cmd_bind_descriptor_sets(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline_layout, 0, &[self.sets[frame % self.sets.len()]], &[]);
            self.device.cmd_push_constants(command_buffer, self.pipeline_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, &constants);
            self.device.cmd_draw(command_buffer, 3, 1, 0, 0);
        }
    }

    /// after the present's render pass: the top screen as shown (without
    /// the grade and the wave) kept in the other image, for the next
    /// change of picture.
    pub(super) fn keep(&self, command_buffer: vk::CommandBuffer, frame: usize, viewport: Viewport, area: [f32; 8], angle: i32, mode: i32) {
        let Some(effect) = self.effect else { return };
        let constants = self.constants(&effect, viewport, area, angle, mode, true);
        let target = &self.held[self.cur ^ 1];
        let device = &self.device;
        let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] } }];
        unsafe {
            device.cmd_begin_render_pass(
                command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(target.framebuffer)
                    .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: self.extent })
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            device.cmd_set_viewport(command_buffer, 0, &[vk::Viewport { x: viewport.x, y: viewport.y, width: viewport.width, height: viewport.height, min_depth: 0.0, max_depth: 1.0 }]);
            let x = viewport.x.max(0.0) as i32;
            let y = viewport.y.max(0.0) as i32;
            let width = (viewport.width as u32).min(self.extent.width.saturating_sub(x as u32));
            let height = (viewport.height as u32).min(self.extent.height.saturating_sub(y as u32));
            device.cmd_set_scissor(command_buffer, 0, &[vk::Rect2D { offset: vk::Offset2D { x, y }, extent: vk::Extent2D { width, height } }]);
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline_layout, 0, &[self.sets[frame % self.sets.len()]], &[]);
            device.cmd_push_constants(command_buffer, self.pipeline_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, &constants);
            device.cmd_draw(command_buffer, 3, 1, 0, 0);
            device.cmd_end_render_pass(command_buffer);
        }
    }
}

impl Drop for PulseFx {
    fn drop(&mut self) {
        let _ = crate::wait_idle(&self.device);
        self.destroy_held();
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}
