//! gfx-perf: texture uploads recorded into the frame's own command buffer.
//!
//! Before, each new texture was its own submission, waited for (about
//! 0.7 ms each; a shadow map is a new texture every frame). Now the texels
//! go into a host-visible upload buffer and the copies are recorded at the
//! start of the next frame's commands (or right away when the frame is
//! already recording). The buffer is reused once the GPU is done with it.

use ash::vk;

use super::{copy_region, Buffer, Image, Renderer};

#[derive(Default)]
pub(crate) struct Uploads {
    buffer: Option<Buffer>,
    used: u64,
    /// copies not recorded yet: image, buffer, offset, size, sizes
    pending: Vec<(vk::Image, vk::Buffer, u64, u32, u32, u32)>,
    /// buffers replaced by a larger one while copies from them were
    /// pending or in flight
    retired: Vec<Buffer>,
    /// the frame's commands are being recorded
    pub(crate) recording: bool,
    /// images to destroy once the submission in flight is done
    retired_images: Vec<Image>,
}

impl Renderer {
    /// an RGBA8 sampled image with the given texels, its copy recorded with
    /// the frame (gfx-perf).
    pub(super) fn upload_deferred(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Image, String> {
        let image = self.image(
            width,
            height,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        self.stage_upload(image, width, height, 1, rgba)
    }

    /// the same for a texture with its smaller sizes (gfx-terrain's mip
    /// chains, render/shaded.rs `upload_levels`): `rgba` holds them one
    /// after the other, the largest first.
    pub(super) fn upload_deferred_levels(&mut self, width: u32, height: u32, levels: u32, rgba: &[u8]) -> Result<Image, String> {
        let format = vk::Format::R8G8B8A8_UNORM;
        let range = level_range(levels);
        let image = unsafe {
            let image = self
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D { width, height, depth: 1 })
                        .mip_levels(levels)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("image: {e:?}"))?;
            let req = self.device.get_image_memory_requirements(image);
            let ty = self.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
            let memory = self
                .device
                .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None)
                .map_err(|e| format!("image memory: {e:?}"))?;
            self.device.bind_image_memory(image, memory, 0).map_err(|e| format!("bind image: {e:?}"))?;
            let info = vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(range);
            let view = self.device.create_image_view(&info, None).map_err(|e| format!("image view: {e:?}"))?;
            Image { image, memory, view }
        };
        self.stage_upload(image, width, height, levels, rgba)
    }

    fn stage_upload(&mut self, image: Image, width: u32, height: u32, levels: u32, rgba: &[u8]) -> Result<Image, String> {
        let need = rgba.len() as u64;
        let at = self.uploads.used.div_ceil(16) * 16;
        let room = self.uploads.buffer.as_ref().map_or(0, |b| b.size);
        if at + need > room {
            // a larger buffer; the old one stays until its copies are done
            let size = (room * 2).max(need * 2).max(4 << 20);
            let new = self.buffer(size, vk::BufferUsageFlags::TRANSFER_SRC)?;
            if let Some(old) = self.uploads.buffer.replace(new) {
                self.uploads.retired.push(old);
            }
            self.uploads.used = 0;
        }
        let at = self.uploads.used.div_ceil(16) * 16;
        let b = self.uploads.buffer.as_ref().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), b.ptr.add(at as usize), rgba.len()) };
        self.uploads.pending.push((image.image, b.buffer, at, width, height, levels));
        self.uploads.used = at + need;
        if self.uploads.recording {
            self.record_uploads();
        }
        Ok(image)
    }

    /// records the pending copies into the command buffer being recorded
    /// (outside a render pass).
    pub(super) fn record_uploads(&mut self) {
        let pending = std::mem::take(&mut self.uploads.pending);
        for (image, buffer, offset, width, height, levels) in pending {
            if levels <= 1 {
                self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
                unsafe {
                    self.device.cmd_copy_buffer_to_image(
                        self.commands,
                        buffer,
                        image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy_region(width, height, vk::ImageAspectFlags::COLOR).buffer_offset(offset)],
                    );
                }
                self.barrier(image, vk::ImageAspectFlags::COLOR, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
                continue;
            }
            // every size, one after the other in the buffer
            let barrier = |from: vk::ImageLayout, to: vk::ImageLayout| {
                vk::ImageMemoryBarrier::default()
                    .image(image)
                    .old_layout(from)
                    .new_layout(to)
                    .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .subresource_range(level_range(levels))
            };
            let mut regions = Vec::new();
            let mut at = offset;
            for level in 0..levels {
                let (w, h) = ((width >> level).max(1), (height >> level).max(1));
                regions.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(at)
                        .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: level, base_array_layer: 0, layer_count: 1 })
                        .image_extent(vk::Extent3D { width: w, height: h, depth: 1 }),
                );
                at += (w * h * 4) as u64;
            }
            unsafe {
                let all = vk::PipelineStageFlags::ALL_COMMANDS;
                let to_dst = [barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL)];
                self.device.cmd_pipeline_barrier(self.commands, all, all, vk::DependencyFlags::empty(), &[], &[], &to_dst);
                self.device.cmd_copy_buffer_to_image(self.commands, buffer, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &regions);
                let to_read = [barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                self.device.cmd_pipeline_barrier(self.commands, all, all, vk::DependencyFlags::empty(), &[], &[], &to_read);
            }
        }
    }

    /// pending copies done now, in a submission of their own (outside a
    /// frame: the renderer's first textures).
    pub(super) fn upload_now(&mut self) -> Result<(), String> {
        if self.uploads.pending.is_empty() || self.uploads.recording {
            return Ok(());
        }
        self.settle();
        self.begin()?;
        self.record_uploads();
        self.submit_and_wait()?;
        self.uploads_free();
        Ok(())
    }

    /// the GPU is done with everything submitted: the upload buffer can be
    /// written from its start again.
    pub(super) fn uploads_free(&mut self) {
        if !self.uploads.pending.is_empty() || self.in_flight {
            return;
        }
        self.uploads.used = 0;
        for b in std::mem::take(&mut self.uploads.retired) {
            self.destroy_buffer(b);
        }
    }

    /// an image no frame draws with any more: destroyed now, or once the
    /// submission in flight is done.
    pub(super) fn retire_image(&mut self, image: Image) {
        if self.in_flight || self.uploads.recording {
            self.uploads.retired_images.push(image);
        } else {
            self.destroy_image(&image);
        }
    }

    pub(super) fn images_free(&mut self) {
        for image in std::mem::take(&mut self.uploads.retired_images) {
            self.destroy_image(&image);
        }
    }

    /// a texture forgotten before its copy was recorded.
    pub(super) fn upload_cancel(&mut self, image: vk::Image) {
        self.uploads.pending.retain(|p| p.0 != image);
    }

    pub(super) fn uploads_destroy(&mut self) {
        self.images_free();
        for b in std::mem::take(&mut self.uploads.retired).into_iter().chain(self.uploads.buffer.take()) {
            self.destroy_buffer(b);
        }
    }
}

fn level_range(levels: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: levels, base_array_layer: 0, layer_count: 1 }
}
