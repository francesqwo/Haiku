//! one Vulkan device for the renderer and the presenter, when they would
//! pick the same GPU, so the screens the renderer draws are shown straight
//! from its images rather than copied out to the host and up again.

use ash::vk;

/// a device, the instance it came from and its one queue, gone once the
/// last of the renderer and the presenter lets go of it. both record and
/// submit on the thread the window runs on, so the queue needs no lock.
pub struct SharedDevice {
    pub(crate) _entry: ash::Entry,
    pub(crate) instance: ash::Instance,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) family: u32,
    pub(crate) device: ash::Device,
    pub(crate) queue: vk::Queue,
    /// perf3: a queue of the same family for small work the game's thread
    /// waits for (shadow maps, icon captures, read back at once), so that
    /// wait is not behind the frames and pictures in between on `queue`
    pub(crate) side: Option<vk::Queue>,
    /// whether the device does logic ops, and counts pipeline statistics.
    pub(crate) logic_ops: bool,
    pub(crate) statistics: bool,
    /// the blend state draws set as they go.
    pub(crate) dynamic: Dynamic,
}

impl SharedDevice {
    /// a device on the GPU the renderer would pick, with no presenter, to
    /// time showing the screens straight from the GPU without a window.
    pub fn new() -> Result<SharedDevice, String> {
        own_device()
    }
}

/// gfx: the parts a native renderer drawing on the same device uses. It
/// records and submits on the same thread as the renderer and presenter.
impl SharedDevice {
    pub fn instance(&self) -> &ash::Instance {
        &self.instance
    }
    pub fn physical(&self) -> vk::PhysicalDevice {
        self.physical
    }
    pub fn device(&self) -> &ash::Device {
        &self.device
    }
    pub fn queue(&self) -> vk::Queue {
        self.queue
    }
    pub fn family(&self) -> u32 {
        self.family
    }
    /// perf3: the side queue (see the field), when the family has one more
    pub fn side_queue(&self) -> Option<vk::Queue> {
        self.side
    }
}

impl std::fmt::Debug for SharedDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedDevice").field("family", &self.family).finish_non_exhaustive()
    }
}

impl Drop for SharedDevice {
    fn drop(&mut self) {
        // SAFETY: whoever made something from the device destroyed it before
        // letting go, the surface of the instance went with the presenter
        unsafe {
            let _ = wait_idle(&self.device); // smooth
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// smooth: the lock that keeps a device's queues apart where Vulkan wants
/// every queue of the device kept off (vkDeviceWaitIdle) while the
/// presenter's thread presents on a queue of its own (backend
/// vulkan_pacer). One for the process: there is one device in practice.
pub fn queue_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// rtpace: the lock around every vkQueueSubmit on the frontend's queue
/// (`SharedDevice::queue`): pictures in between are drawn on a thread of
/// their own (fates-gfx rtpace) while the frontend's thread lends its time
/// to the game, so the queue has more than one thread submitting to it.
/// Held only for the call itself, never across a wait.
pub fn submit_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// smooth: vkDeviceWaitIdle under `queue_lock`; use it instead of the
/// device's own wherever the presenter may share the device.
pub fn wait_idle(device: &ash::Device) -> ash::prelude::VkResult<()> {
    let started = std::time::Instant::now();
    let _queues = queue_lock();
    let _submits = submit_lock(); // rtpace
    // SAFETY: every queue of the device is kept off: the frontend's thread
    // is the caller, the present thread takes the lock to present
    let result = unsafe { device.device_wait_idle() };
    crate::smooth::since("idle", started, 0, 0.0);
    result
}
// -- native-final: making the device, moved here from the emulated hardware
// rasterizer (raster/hardware.rs), which the pure build leaves out: the
// presenter and the native renderer make their device with these.

const DEPTH_FORMAT: vk::Format = vk::Format::D24_UNORM_S8_UINT;
const FLOAT_DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT_S8_UINT;

fn vk_error(what: &'static str) -> impl Fn(vk::Result) -> String {
    move |error| format!("could not {what}, {error}")
}

/// the state draws set as they go rather than their pipelines hold, when
/// the device can, so that draws differing only in it share a pipeline.
/// the generic pipelines are then few enough to make them all at the start,
/// rather than each on the draw that first needs it, which stalled the game
/// for as long as the driver took, up to a third of a second.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Dynamic {
    /// blending on or off, its equation, and the color write mask.
    pub(crate) blend: bool,
    /// the logic op on or off, and which.
    pub(crate) logic_op: bool,
}

/// whether the renderer can draw on a GPU, Vulkan 1.3, push descriptors,
/// depth clamping and a depth format, see depth_format.
pub(crate) fn renders_on(instance: &ash::Instance, device: vk::PhysicalDevice) -> bool {
    // SAFETY: queries on a GPU of the instance
    let (properties, features, extensions) = unsafe {
        (
            instance.get_physical_device_properties(device),
            instance.get_physical_device_features(device),
            instance.enumerate_device_extension_properties(device).unwrap_or_default(),
        )
    };
    let push = extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(ash::khr::push_descriptor::NAME));
    properties.api_version >= vk::API_VERSION_1_3
        && features.depth_clamp == vk::TRUE
        && depth_format(instance, device).is_some()
        && push
}

/// the format a GPU draws depth buffers into, DEPTH_FORMAT, the console's
/// 24 bits, or where it can't, FLOAT_DEPTH_FORMAT. Vulkan has every GPU do
/// one of the two, AMD's and Apple's only the floats. ZAKURO_FLOAT_DEPTH set has the
/// floats wherever the GPU does them, to tell the two apart.
pub(crate) fn depth_format(instance: &ash::Instance, device: vk::PhysicalDevice) -> Option<vk::Format> {
    let draws_into = |format| {
        // SAFETY: a query on a GPU of the instance
        let features = unsafe { instance.get_physical_device_format_properties(device, format) }.optimal_tiling_features;
        features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    };
    let mut formats = [DEPTH_FORMAT, FLOAT_DEPTH_FORMAT];
    if std::env::var_os("ZAKURO_FLOAT_DEPTH").is_some() {
        formats.reverse();
    }
    formats.into_iter().find(|&format| draws_into(format))
}

/// whether the renderer can draw on a GPU through a family of its queues,
/// which has to do compute as well, for display transfers.
pub(crate) fn can_render(instance: &ash::Instance, device: vk::PhysicalDevice, family: u32) -> bool {
    // SAFETY: as above
    let families = unsafe { instance.get_physical_device_queue_family_properties(device) };
    let both = vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE;
    families.get(family as usize).is_some_and(|f| f.queue_flags.contains(both)) && renders_on(instance, device)
}

/// a device the renderer draws with, on a GPU can_render or pick chose,
/// with the extensions given besides its own. it owns the instance from then
/// on, destroying it when it goes, which stays the caller's when it fails.
pub(crate) fn render_device(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    family: u32,
    extra: &[*const std::ffi::c_char],
) -> Result<SharedDevice, String> {
    // smooth: a presenter's device (it asks for the swapchain) gets a second
    // queue of the family where there is one, which its present thread
    // presents on (backend vulkan_pacer)
    let presents = extra.iter().any(|&e| unsafe { std::ffi::CStr::from_ptr(e) } == ash::khr::swapchain::NAME);
    // SAFETY: a query on the physical device of this instance
    let family_queues = unsafe { instance.get_physical_device_queue_family_properties(physical) }.get(family as usize).map_or(1, |f| f.queue_count);
    let priorities: &[f32] = if presents && family_queues >= 2 { &[1.0, 1.0] } else { &[1.0] };
    // perf3: and one more for the side queue where the family has it
    // (FATES_GFX_SIDE_QUEUE=0: none, everything on the first)
    let side_index = priorities.len() as u32;
    let side_wanted = family_queues > side_index && !std::env::var("FATES_GFX_SIDE_QUEUE").is_ok_and(|v| v == "0");
    let priorities: &[f32] = match (side_wanted, priorities.len()) {
        (true, 2) => &[1.0, 1.0, 1.0],
        (true, _) => &[1.0, 1.0],
        _ => priorities,
    };
    let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(priorities)];
    let mut extensions = vec![ash::khr::push_descriptor::NAME.as_ptr()];
    extensions.extend_from_slice(extra);
    extensions.extend(crate::instance::portability_subset(instance, physical).map(std::ffi::CStr::as_ptr));
    // SAFETY: the physical device came from this instance
    let supported = unsafe { instance.get_physical_device_features(physical) };
    let logic_ops = supported.logic_op == vk::TRUE;
    let dynamic = dynamic_state(instance, physical, logic_ops);
    if dynamic.blend {
        extensions.push(ash::ext::extended_dynamic_state3::NAME.as_ptr());
    }
    if dynamic.logic_op {
        extensions.push(ash::ext::extended_dynamic_state2::NAME.as_ptr());
    }
    let mut blend_state = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default()
        .extended_dynamic_state3_color_blend_enable(true)
        .extended_dynamic_state3_color_blend_equation(true)
        .extended_dynamic_state3_color_write_mask(true)
        .extended_dynamic_state3_logic_op_enable(dynamic.logic_op);
    let mut logic_op_state = vk::PhysicalDeviceExtendedDynamicState2FeaturesEXT::default().extended_dynamic_state2_logic_op(true);
    // what the GPU shades counted along with its times
    let statistics = std::env::var_os("ZAKURO_GPU_TIMES").is_some() && supported.pipeline_statistics_query == vk::TRUE;
    let features = vk::PhysicalDeviceFeatures::default()
        .depth_clamp(true)
        .logic_op(logic_ops)
        // gfx2d3: the native renderer filters 3D textures anisotropically
        .sampler_anisotropy(supported.sampler_anisotropy == vk::TRUE)
        // msaa: the native renderer shades alpha-tested draws (foliage) per
        // sample when anti-aliasing is on
        .sample_rate_shading(supported.sample_rate_shading == vk::TRUE)
        .pipeline_statistics_query(statistics);
    // the cache control, to make a pipeline only when the cache on disk
    // has it, every Vulkan 1.3 device does
    let mut features13 = vk::PhysicalDeviceVulkan13Features::default()
        .dynamic_rendering(true)
        .synchronization2(true)
        .pipeline_creation_cache_control(true);
    let mut info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&extensions)
        .enabled_features(&features)
        .push_next(&mut features13);
    if dynamic.blend {
        info = info.push_next(&mut blend_state);
    }
    if dynamic.logic_op {
        info = info.push_next(&mut logic_op_state);
    }
    // smooth: the presenter asks for present ids and waits, which take
    // their features as well as their extensions
    let wants = |name: &std::ffi::CStr| extra.iter().any(|&e| unsafe { std::ffi::CStr::from_ptr(e) } == name);
    let mut present_id = vk::PhysicalDevicePresentIdFeaturesKHR::default().present_id(true);
    let mut present_wait = vk::PhysicalDevicePresentWaitFeaturesKHR::default().present_wait(true);
    if wants(ash::khr::present_id::NAME) && wants(ash::khr::present_wait::NAME) {
        info = info.push_next(&mut present_id).push_next(&mut present_wait);
    }
    // SAFETY: the caller checked the device has all of this
    let device = unsafe { instance.create_device(physical, &info, None) }.map_err(vk_error("create a device"))?;
    // SAFETY: a queue the device was made with
    let queue = unsafe { device.get_device_queue(family, 0) };
    // SAFETY: as above (perf3)
    let side = side_wanted.then(|| unsafe { device.get_device_queue(family, side_index) });
    let (_entry, instance) = (entry.clone(), instance.clone());
    Ok(SharedDevice { _entry, instance, physical, family, device, queue, side, logic_ops, statistics, dynamic })
}

/// which of the blend state the device lets draws set as they go, none
/// with ZAKURO_STATIC_BLEND set, to compare the two.
pub(crate) fn dynamic_state(instance: &ash::Instance, physical: vk::PhysicalDevice, logic_ops: bool) -> Dynamic {
    if std::env::var_os("ZAKURO_STATIC_BLEND").is_some() {
        return Dynamic::default();
    }
    // SAFETY: the physical device came from this instance
    let available = unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
    let has = |name: &std::ffi::CStr| available.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
    let (three, two) = (has(ash::ext::extended_dynamic_state3::NAME), has(ash::ext::extended_dynamic_state2::NAME));
    let mut supported3 = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default();
    let mut supported2 = vk::PhysicalDeviceExtendedDynamicState2FeaturesEXT::default();
    {
        let mut features = vk::PhysicalDeviceFeatures2::default();
        if three {
            features = features.push_next(&mut supported3);
        }
        if two {
            features = features.push_next(&mut supported2);
        }
        // SAFETY: as above, with a chain of structures that live as long
        unsafe { instance.get_physical_device_features2(physical, &mut features) };
    }
    let blend = three
        && supported3.extended_dynamic_state3_color_blend_enable == vk::TRUE
        && supported3.extended_dynamic_state3_color_blend_equation == vk::TRUE
        && supported3.extended_dynamic_state3_color_write_mask == vk::TRUE;
    let logic_op = blend
        && logic_ops
        && two
        && supported2.extended_dynamic_state2_logic_op == vk::TRUE
        && supported3.extended_dynamic_state3_logic_op_enable == vk::TRUE;
    Dynamic { blend, logic_op }
}

/// a device of the renderer's own, on the GPU pick chooses.
pub(crate) fn own_device() -> Result<SharedDevice, String> {
    let entry = crate::instance::entry().map_err(|e| format!("no Vulkan loader, {e}"))?;
    let app = vk::ApplicationInfo::default().application_name(c"zakuro").api_version(vk::API_VERSION_1_3);
    let instance = crate::instance::new(&entry, &app, &[]).map_err(vk_error("create an instance"))?;
    let made = pick(&instance).and_then(|(physical, family)| render_device(&entry, &instance, physical, family, &[]));
    if made.is_err() {
        // SAFETY: nothing made from the instance is left
        unsafe { instance.destroy_instance(None) };
    }
    made
}

/// the device to draw with and its graphics queue family, a discrete GPU
/// when there is one.
pub(crate) fn pick(instance: &ash::Instance) -> Result<(vk::PhysicalDevice, u32), String> {
    // SAFETY: queries on an instance we own
    let devices = unsafe { instance.enumerate_physical_devices() }.map_err(vk_error("list the GPUs"))?;
    let mut best: Option<(u32, vk::PhysicalDevice, u32)> = None;
    for device in devices {
        // SAFETY: as above
        let (properties, families) =
            unsafe { (instance.get_physical_device_properties(device), instance.get_physical_device_queue_family_properties(device)) };
        let Some(family) = families.iter().position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)) else { continue };
        if !renders_on(instance, device) {
            continue;
        }
        let rank = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 0,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
            _ => 2,
        };
        if best.is_none_or(|(best, ..)| rank < best) {
            best = Some((rank, device, family as u32));
        }
    }
    best.map(|(_, device, family)| (device, family))
        .ok_or_else(|| "no GPU with Vulkan 1.3, push descriptors and a D24S8 depth format".to_owned())
}

