//! smooth: when presented images reach the display, from VK_KHR_present_id
//! and VK_KHR_present_wait.
//!
//! Each present carries an id. A thread of its own waits for each id in
//! turn (`vkWaitForPresentKHR`) and notes the time it came back: the moment
//! the image was shown, as the driver tells it. That gives
//!
//! - the timing log's `shown` events (FATES_SMOOTH_LOG), the intervals at
//!   which pictures really reach the display;
//! - the newest shown id and its time, which the frontend's pacing reads to
//!   keep presents from queueing up ahead of the display.
//!
//! The waiter never touches the queue. It holds `lock` during each wait,
//! which is short (WAIT_SLICE), and the presenter holds it while it makes
//! or destroys a swapchain, so a wait never sees a retired or destroyed
//! swapchain (a wait on a retired one is invalid usage).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ash::vk;

/// how long one wait lasts before the waiter looks again whether its
/// swapchain is still the one presented to.
const WAIT_SLICE: Duration = Duration::from_millis(4);
/// a present not shown after this long is given up on (a hidden window, a
/// swapchain that went), so the waiter moves on to the next.
const GIVE_UP: Duration = Duration::from_millis(500);

/// whether a device can do it: both extensions and both features.
pub(super) fn supported(instance: &ash::Instance, physical: vk::PhysicalDevice) -> bool {
    if std::env::var("FATES_PRESENT_WAIT").is_ok_and(|v| v == "0") {
        return false;
    }
    // SAFETY: queries on a physical device of this instance
    let available = unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
    let has = |name: &std::ffi::CStr| available.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
    if !has(ash::khr::present_id::NAME) || !has(ash::khr::present_wait::NAME) {
        return false;
    }
    let mut id = vk::PhysicalDevicePresentIdFeaturesKHR::default();
    let mut wait = vk::PhysicalDevicePresentWaitFeaturesKHR::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut id).push_next(&mut wait);
    // SAFETY: as above, the chain lives as long as the call
    unsafe { instance.get_physical_device_features2(physical, &mut features) };
    id.present_id == vk::TRUE && wait.present_wait == vk::TRUE
}

/// the extensions to ask the device for.
pub(super) fn extensions() -> [*const std::ffi::c_char; 2] {
    [ash::khr::present_id::NAME.as_ptr(), ash::khr::present_wait::NAME.as_ptr()]
}

struct Queue {
    /// presents to wait for: the swapchain, its generation and the id.
    pending: VecDeque<(vk::SwapchainKHR, u64, u64)>,
    /// the swapchain presented to now; pending items of another go.
    generation: u64,
    /// the presenter is changing the swapchain: no wait starts.
    changing: bool,
    /// the waiter is in a wait.
    in_wait: bool,
    quit: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    /// for every change of the queue's state.
    changed: Condvar,
    /// the newest id shown and when (ns since `base`).
    shown_id: AtomicU64,
    shown_at: AtomicU64,
    base: Instant,
}

pub(super) struct PresentTiming {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    next_id: u64,
}

/// while it lives, the waiter keeps off the swapchains, see
/// PresentTiming::changing.
pub(super) struct Changing(Arc<Shared>);

impl Drop for Changing {
    fn drop(&mut self) {
        self.0.queue.lock().unwrap().changing = false;
        self.0.changed.notify_all();
    }
}

impl PresentTiming {
    pub(super) fn new(instance: &ash::Instance, device: &ash::Device) -> PresentTiming {
        let wait = ash::khr::present_wait::Device::new(instance, device);
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue { pending: VecDeque::new(), generation: 0, changing: false, in_wait: false, quit: false }),
            changed: Condvar::new(),
            shown_id: AtomicU64::new(0),
            shown_at: AtomicU64::new(0),
            base: Instant::now(),
        });
        let theirs = shared.clone();
        let thread = std::thread::Builder::new().name("present wait".to_owned()).spawn(move || waiter(theirs, wait)).ok();
        log::info!("smooth: presents are timed with VK_KHR_present_wait");
        PresentTiming { shared, thread, next_id: 0 }
    }

    /// the id for the next present, to chain into its VkPresentIdKHR.
    pub(super) fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// after a present with that id went to the queue.
    pub(super) fn presented(&self, swapchain: vk::SwapchainKHR, id: u64) {
        presented(&self.shared, swapchain, id);
    }

    /// `presented`, for the present thread to call.
    pub(super) fn notifier(&self) -> Arc<dyn Fn(vk::SwapchainKHR, u64) + Send + Sync> {
        let shared = self.shared.clone();
        Arc::new(move |swapchain, id| presented(&shared, swapchain, id))
    }

    /// keeps the waiter off the swapchains while they change: keep what it
    /// gives while a swapchain is made (the old one retires) or destroyed.
    /// Presents waited for so far are forgotten.
    pub(super) fn changing(&self) -> Changing {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.changing = true;
        queue.generation += 1;
        queue.pending.clear();
        while queue.in_wait {
            queue = self.shared.changed.wait(queue).unwrap();
        }
        Changing(self.shared.clone())
    }

    /// the newest present shown, and when.
    pub(super) fn shown(&self) -> Option<(u64, Instant)> {
        let id = self.shared.shown_id.load(Ordering::Acquire);
        (id != 0).then(|| (id, self.shared.base + Duration::from_nanos(self.shared.shown_at.load(Ordering::Relaxed))))
    }

    /// the id the last present carried.
    pub(super) fn last_id(&self) -> u64 {
        self.next_id
    }
}

impl Drop for PresentTiming {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn presented(shared: &Shared, swapchain: vk::SwapchainKHR, id: u64) {
    let mut queue = shared.queue.lock().unwrap();
    let generation = queue.generation;
    queue.pending.push_back((swapchain, generation, id));
    drop(queue);
    shared.changed.notify_all();
}

fn waiter(shared: Arc<Shared>, wait: ash::khr::present_wait::Device) {
    let mut current: Option<(vk::SwapchainKHR, u64, u64, Instant)> = None;
    loop {
        // the next wait to make, once nothing keeps it off
        let (swapchain, id, started) = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if queue.quit {
                    return;
                }
                if current.is_some_and(|(_, generation, _, _)| generation != queue.generation) {
                    current = None;
                }
                if current.is_none() {
                    current = queue.pending.pop_front().map(|(swapchain, generation, id)| (swapchain, generation, id, Instant::now()));
                }
                if let (Some((swapchain, _, id, started)), false) = (current, queue.changing) {
                    queue.in_wait = true;
                    break (swapchain, id, started);
                }
                queue = shared.changed.wait(queue).unwrap();
            }
        };
        // SAFETY: the swapchain is the one presented to (its generation is
        // current), and the presenter does not change it while in_wait
        let result = unsafe { wait.wait_for_present(swapchain, id, WAIT_SLICE.as_nanos() as u64) };
        let now = Instant::now();
        shared.queue.lock().unwrap().in_wait = false;
        shared.changed.notify_all();
        match result {
            Ok(()) => {
                shared.shown_at.store((now - shared.base).as_nanos() as u64, Ordering::Relaxed);
                shared.shown_id.store(id, Ordering::Release);
                crate::smooth::point("shown", now, id as i64, 0.0);
                current = None;
            }
            Err(vk::Result::TIMEOUT) if started.elapsed() < GIVE_UP => {}
            Err(vk::Result::TIMEOUT) => {
                crate::smooth::point("lost", now, id as i64, 0.0);
                current = None;
            }
            Err(error) => {
                log::debug!("present wait: {error}");
                current = None;
            }
        }
    }
}
