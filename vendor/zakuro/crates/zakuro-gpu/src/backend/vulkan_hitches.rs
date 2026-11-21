//! hitches (issue #14): how the presenter takes a present that comes back
//! VK_SUBOPTIMAL_KHR, and a log of the swapchain being made again.
//!
//! Before, any suboptimal present marked the swapchain stale, and the next
//! frame waited for the whole device to be idle and made a new swapchain
//! (and skipped its picture). An overlay drawn into the game's presents (the
//! Steam overlay's Vulkan layer is one) or a driver can answer suboptimal to
//! every present while the window is exactly the size of the swapchain, and
//! then every frame did that: the game stuttered for as long as it lasted.
//!
//! Now a suboptimal present makes the swapchain again at once only when the
//! surface's size is not the swapchain's any more; otherwise at most once
//! every few seconds (whatever else made it suboptimal is still taken in,
//! never a burst). Out of date (VK_ERROR_OUT_OF_DATE_KHR) and a resize
//! still make it again at once, as before. Each remaking is logged, a line
//! every few seconds at most, with how long the wait and the making took.

use std::time::{Duration, Instant};

use ash::vk;

/// a suboptimal swapchain of the window's size is made again no more often.
const SUBOPTIMAL_EVERY: Duration = Duration::from_secs(3);
/// the log's lines about the swapchain, no more often.
const LOG_EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct SwapchainWatch {
    /// a present (or acquire) said suboptimal since the last frame.
    pub suboptimal: bool,
    /// why the swapchain is being made again, for the log.
    pub reason: Option<&'static str>,
    /// the last time a suboptimal swapchain of the right size was made again.
    last_suboptimal_rebuild: Option<Instant>,
    /// suboptimal presents let be since the last line.
    let_be: u32,
    /// remakings since the last line, and the time they took.
    rebuilds: u32,
    spent: Duration,
    logged: Option<Instant>,
}

impl SwapchainWatch {
    /// after a suboptimal present: whether to make the swapchain again now.
    /// `fits` says whether the surface is still the swapchain's size.
    pub fn remake_for_suboptimal(&mut self, fits: bool) -> bool {
        let now = Instant::now();
        if !fits {
            self.reason = Some("a present said suboptimal and the window's size changed");
            return true;
        }
        if self.last_suboptimal_rebuild.is_none_or(|at| now - at >= SUBOPTIMAL_EVERY) {
            self.last_suboptimal_rebuild = Some(now);
            self.reason = Some("a present said suboptimal");
            return true;
        }
        self.let_be += 1;
        false
    }

    /// after the swapchain was made again: logs it, a line every few
    /// seconds at most.
    pub fn remade(&mut self, started: Instant, extent: vk::Extent2D) {
        let now = Instant::now();
        self.rebuilds += 1;
        self.spent += now - started;
        let reason = self.reason.take().unwrap_or("out of date, or the window changed");
        if self.logged.is_some_and(|at| now - at < LOG_EVERY) {
            return;
        }
        let let_be = if self.let_be > 0 { format!("; {} suboptimal presents let be", self.let_be) } else { String::new() };
        log::info!(
            "hitches: the swapchain was made again {} time{} ({:.0} ms in all), last for {reason}, at {}x{}{let_be}",
            self.rebuilds,
            if self.rebuilds == 1 { "" } else { "s" },
            self.spent.as_secs_f64() * 1e3,
            extent.width,
            extent.height
        );
        self.logged = Some(now);
        self.rebuilds = 0;
        self.spent = Duration::ZERO;
        self.let_be = 0;
    }
}

/// whether the surface is still the size of the swapchain (extent), the
/// window being `window` where the surface takes its size from the
/// swapchain. An error says it is not, so the swapchain is made again.
pub(super) fn surface_fits(
    surface_instance: &ash::khr::surface::Instance,
    physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    window: (u32, u32),
    extent: vk::Extent2D,
) -> bool {
    // SAFETY: a query on the presenter's own surface and device
    let Ok(capabilities) = (unsafe { surface_instance.get_physical_device_surface_capabilities(physical_device, surface) }) else { return false };
    if capabilities.current_extent.width == u32::MAX {
        return (window.0, window.1) == (extent.width, extent.height);
    }
    capabilities.current_extent == extent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_of_suboptimal_presents_remakes_the_swapchain_once() {
        let mut watch = SwapchainWatch::default();
        // the first suboptimal present of the window's size: made again
        assert!(watch.remake_for_suboptimal(true));
        watch.remade(Instant::now(), vk::Extent2D { width: 1, height: 1 });
        // every frame after it, for a while: let be
        for _ in 0..500 {
            assert!(!watch.remake_for_suboptimal(true));
        }
        // the window's size changed: at once
        assert!(watch.remake_for_suboptimal(false));
    }
}
