//! bottom: reads a presented frame back from the swapchain and writes it as
//! a PNG, so the window's composition can be checked without capturing the
//! desktop (which may be locked, covered or black for a Vulkan window).

use std::path::PathBuf;

use ash::vk;

use super::vulkan::{find_memory_type, vk_fail};
use super::PresentError;

/// a frame being read back: the buffer it is copied into.
pub(super) struct Capture {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    width: u32,
    height: u32,
    pub(super) path: PathBuf,
    /// the swapchain image it reads.
    pub(super) image: vk::Image,
}

impl Capture {
    pub(super) fn new(
        device: &ash::Device,
        memory: &vk::PhysicalDeviceMemoryProperties,
        extent: vk::Extent2D,
        path: PathBuf,
    ) -> Result<Capture, PresentError> {
        let size = extent.width as u64 * extent.height as u64 * 4;
        // SAFETY: plain object creation on a live device
        unsafe {
            let buffer = device
                .create_buffer(&vk::BufferCreateInfo::default().size(size).usage(vk::BufferUsageFlags::TRANSFER_DST), None)
                .map_err(vk_fail("creating the capture buffer"))?;
            let requirements = device.get_buffer_memory_requirements(buffer);
            let Some(type_index) = find_memory_type(
                memory,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            ) else {
                device.destroy_buffer(buffer, None);
                return Err(PresentError::Backend("no host memory for the capture".into()));
            };
            let memory = device
                .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(type_index), None)
                .map_err(vk_fail("allocating the capture buffer"))?;
            device.bind_buffer_memory(buffer, memory, 0).map_err(vk_fail("binding the capture buffer"))?;
            Ok(Capture { buffer, memory, width: extent.width, height: extent.height, path, image: vk::Image::null() })
        }
    }

    /// copies the swapchain image, drawn and ready to present, into the
    /// buffer, leaving it ready to present.
    pub(super) fn record(&self, device: &ash::Device, command_buffer: vk::CommandBuffer) {
        let image = self.image;
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |from, to, src, dst| {
            vk::ImageMemoryBarrier::default()
                .old_layout(from)
                .new_layout(to)
                .src_access_mask(src)
                .dst_access_mask(dst)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)
        };
        // SAFETY: recording outside a render pass, the image having left it
        // in the present layout
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(vk::ImageLayout::PRESENT_SRC_KHR, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_READ)],
            );
            device.cmd_copy_image_to_buffer(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.buffer,
                &[vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                    .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 })],
            );
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier(vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::PRESENT_SRC_KHR, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::empty())],
            );
        }
    }

    /// once the GPU is done: writes the PNG and frees the buffer.
    pub(super) fn finish(self, device: &ash::Device, format: vk::Format) {
        // SAFETY: the caller waited for the device, the memory is host
        // visible and as large as the frame
        let pixels = unsafe {
            let mapped = device.map_memory(self.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty());
            let pixels = mapped.ok().map(|mapped| std::slice::from_raw_parts(mapped as *const u8, (self.width * self.height * 4) as usize).to_vec());
            device.unmap_memory(self.memory);
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
            pixels
        };
        let Some(mut pixels) = pixels else { return };
        let bgr = matches!(format, vk::Format::B8G8R8A8_SRGB | vk::Format::B8G8R8A8_UNORM);
        for pixel in pixels.chunks_exact_mut(4) {
            if bgr {
                pixel.swap(0, 2);
            }
            pixel[3] = 255;
        }
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let file = std::io::BufWriter::new(std::fs::File::create(&self.path)?);
            let mut encoder = png::Encoder::new(file, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&pixels)?;
            Ok(())
        };
        match write() {
            Ok(()) => log::info!("captured the window, {}x{}, to {}", self.width, self.height, self.path.display()),
            Err(error) => log::warn!("could not write the capture {}: {error}", self.path.display()),
        }
    }
}
