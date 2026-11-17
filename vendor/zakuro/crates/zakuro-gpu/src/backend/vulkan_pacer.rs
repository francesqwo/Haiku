//! smooth: presents handed to the display on a thread of their own, each at
//! its time on the display's refresh grid.
//!
//! The frontend emulates the game and draws on one thread. Emulating one of
//! the game's refreshes takes up to ~10 ms, longer than a 175 Hz refresh
//! (5.7 ms), and on Windows a window's presents are composed: two presents
//! between the same two refreshes show only the newer, and a refresh with
//! none repeats the picture. Presenting from the emulating thread therefore
//! left refreshes without a new picture during every emulated refresh and
//! bunched the presents after it (measured: ~125 distinct pictures a second
//! at 175 Hz, 1% lows near 50 fps).
//!
//! Here the frontend records and submits a few pictures ahead, each for a
//! slot on the refresh grid, and this thread calls vkQueuePresentKHR for
//! each at its slot, while the frontend emulates. The pictures' GPU work is
//! all submitted by the frontend's thread on its queue, in order, as
//! before; only the present moves, onto a second queue of the same family
//! (so the frontend's queue needs no lock). What still needs one:
//!
//! - the swapchain, which acquire and present both use: `swapchain_lock`.
//!   A present waits for the display (about a refresh, windowed), so the
//!   thread also acquires the images the frontend draws into, right after
//!   each present, and keeps a few ready (`take_image`); the frontend only
//!   acquires itself when none is ready;
//! - vkDeviceWaitIdle, which needs every queue of the device kept off:
//!   `crate::wait_idle` takes the device-wide lock this thread takes around
//!   each present.
//!
//! The refresh grid comes from DWM (`DwmGetCompositionTimingInfo`: the last
//! vertical blank and the refresh period), else from the monitor's rate.
//! Each present goes out at `PHASE` of the way through its refresh, far from
//! both blanks, so jitter of a millisecond either way still lands it in the
//! right one.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ash::vk;

/// how far through its refresh a present goes out.
fn phase() -> f64 {
    static PHASE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PHASE.get_or_init(|| std::env::var("FATES_PRESENT_PHASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5f64).clamp(0.0, 0.95))
}

/// a picture recorded and submitted, waiting for its slot.
pub(super) struct Pending {
    pub swapchain: vk::SwapchainKHR,
    pub image: u32,
    /// signalled by the picture's submit, waited by the present.
    pub semaphore: vk::Semaphore,
    pub id: u64,
    /// when it goes out, none for at once.
    pub at: Option<Instant>,
}

/// semaphores the thread acquires images with, used in turn; more than the
/// images waiting can hold plus the frontend's frames in flight, so one is
/// used again only after the submit that waited for it finished.
const SEMAPHORES: usize = 24;
/// images the thread keeps acquired for the frontend.
const STOCK: usize = 2;

struct Queue {
    pending: VecDeque<Pending>,
    /// images acquired for the frontend: swapchain, index, semaphore.
    stock: VecDeque<(vk::SwapchainKHR, u32, vk::Semaphore)>,
    /// the swapchain to acquire from, null while it changes.
    swapchain: vk::SwapchainKHR,
    /// the last acquire found no image: wait for the next present.
    starved: bool,
    /// the present being made now.
    busy: bool,
    /// present everything at once (the swapchain is about to change).
    flush: bool,
    quit: bool,
    /// slots of the presents handed over and not yet made.
    last_at: Option<Instant>,
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    /// a present came back out of date or suboptimal.
    stale: AtomicBool,
}

/// the refresh grid: a vertical blank and the period.
#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub blank: Instant,
    pub period: Duration,
}

impl Grid {
    /// the slot (a present's time) of the refresh at or after `t`.
    pub fn slot_at_or_after(&self, t: Instant) -> Instant {
        let period = self.period.as_secs_f64();
        let offset = phase() * period;
        let since = if t >= self.blank { (t - self.blank).as_secs_f64() } else { -(self.blank - t).as_secs_f64() };
        let k = ((since - offset) / period).ceil();
        let at = k * period + offset;
        if at >= 0.0 {
            self.blank + Duration::from_secs_f64(at)
        } else {
            self.blank - Duration::from_secs_f64(-at)
        }
    }
}

pub(super) struct PresentThread {
    shared: Arc<Shared>,
    device: ash::Device,
    semaphores: Arc<Vec<vk::Semaphore>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// held by acquire and present.
    pub swapchain_lock: Arc<Mutex<()>>,
    grid: Mutex<(Grid, Instant)>,
    fallback_period: Mutex<Duration>,
    /// smooth2: with VSync off, slots on a grid of this period of their own
    /// (the frame rate asked for), not the display's refreshes.
    free: Mutex<Option<Grid>>,
}

#[cfg(windows)]
mod win {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        pub fn DwmGetCompositionTimingInfo(hwnd: isize, info: *mut u8) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn QueryPerformanceCounter(count: *mut i64) -> i32;
        pub fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }
    /// sizeof(DWM_TIMING_INFO), which dwmapi.h packs to 1.
    pub const SIZE: usize = 292;
}

/// DWM's last vertical blank and its period, as Instants.
#[cfg(windows)]
fn dwm_grid() -> Option<Grid> {
    let mut info = [0u8; win::SIZE];
    info[..4].copy_from_slice(&(win::SIZE as u32).to_le_bytes());
    let (mut now, mut frequency) = (0i64, 0i64);
    // SAFETY: a buffer of the struct's size, its size in its first field
    let (ok, instant) = unsafe {
        let ok = win::DwmGetCompositionTimingInfo(0, info.as_mut_ptr()) == 0;
        win::QueryPerformanceCounter(&mut now);
        win::QueryPerformanceFrequency(&mut frequency);
        (ok, Instant::now())
    };
    if !ok || frequency <= 0 {
        return None;
    }
    let u64_at = |offset: usize| u64::from_le_bytes(info[offset..offset + 8].try_into().unwrap());
    let (period, blank) = (u64_at(12), u64_at(28));
    if period == 0 || blank == 0 {
        return None;
    }
    let period = Duration::from_secs_f64(period as f64 / frequency as f64);
    if !(Duration::from_micros(2000)..Duration::from_millis(60)).contains(&period) {
        return None;
    }
    let ago = Duration::from_secs_f64((now - blank as i64).max(0) as f64 / frequency as f64);
    Some(Grid { blank: instant.checked_sub(ago)?, period })
}

#[cfg(not(windows))]
fn dwm_grid() -> Option<Grid> {
    None
}

impl PresentThread {
    pub(super) fn new(device: &ash::Device, swapchain_device: ash::khr::swapchain::Device, queue: vk::Queue, timing: Option<Arc<dyn Fn(vk::SwapchainKHR, u64) + Send + Sync>>) -> PresentThread {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                pending: VecDeque::new(),
                stock: VecDeque::new(),
                swapchain: vk::SwapchainKHR::null(),
                starved: false,
                busy: false,
                flush: false,
                quit: false,
                last_at: None,
            }),
            changed: Condvar::new(),
            stale: AtomicBool::new(false),
        });
        let swapchain_lock = Arc::new(Mutex::new(()));
        // SAFETY: plain semaphores of the device, destroyed in drop
        let semaphores: Vec<vk::Semaphore> =
            (0..SEMAPHORES).filter_map(|_| unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }.ok()).collect();
        let semaphores = Arc::new(semaphores);
        let (theirs, lock, ring, dev) = (shared.clone(), swapchain_lock.clone(), semaphores.clone(), device.clone());
        let thread = std::thread::Builder::new()
            .name("present".to_owned())
            .spawn(move || presenter(theirs, lock, swapchain_device, queue, timing, dev, ring))
            .ok();
        let start = Instant::now();
        let grid = dwm_grid().unwrap_or(Grid { blank: start, period: Duration::from_secs_f64(1.0 / 60.0) });
        log::info!("smooth: presents go out on a thread of their own, at {:.0}% of each {:.3} ms refresh", phase() * 100.0, grid.period.as_secs_f64() * 1e3);
        PresentThread {
            shared,
            device: device.clone(),
            semaphores,
            thread,
            swapchain_lock,
            grid: Mutex::new((grid, start)),
            fallback_period: Mutex::new(grid.period),
            free: Mutex::new(None),
        }
    }

    /// the swapchain the thread acquires images from, once made.
    pub(super) fn set_swapchain(&self, swapchain: vk::SwapchainKHR) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.swapchain = swapchain;
        queue.starved = false;
        drop(queue);
        self.shared.changed.notify_all();
    }

    /// an image the thread acquired, and the semaphore to wait for.
    pub(super) fn take_image(&self, swapchain: vk::SwapchainKHR) -> Option<(u32, vk::Semaphore)> {
        let mut queue = self.shared.queue.lock().unwrap();
        let taken = match queue.stock.front() {
            Some(&(from, image, semaphore)) if from == swapchain => {
                queue.stock.pop_front();
                Some((image, semaphore))
            }
            _ => None,
        };
        drop(queue);
        self.shared.changed.notify_all();
        taken
    }

    /// smooth2: VSync off: slots every `period` (none: the display's
    /// refresh grid again).
    pub(super) fn set_free_period(&self, period: Option<Duration>) {
        let mut free = self.free.lock().unwrap();
        *free = period.map(|period| Grid { blank: free.map_or_else(Instant::now, |grid| grid.blank), period });
    }

    /// the refresh grid, from DWM at most every 250 ms (smooth2: or the
    /// free grid with VSync off).
    pub(super) fn grid(&self) -> Grid {
        let mut kept = self.grid.lock().unwrap();
        if kept.1.elapsed() >= Duration::from_millis(250) {
            kept.1 = Instant::now();
            match dwm_grid() {
                Some(grid) => {
                    kept.0 = grid;
                    // mapjit: dev, the display's refresh grid in the timing log
                    crate::smooth::point("dwm", grid.blank, 0, grid.period.as_secs_f64() * 1e3);
                }
                None => kept.0.period = *self.fallback_period.lock().unwrap(),
            }
        }
        if let Some(free) = *self.free.lock().unwrap() {
            return free;
        }
        // mapjit: DWM's timing is the desktop's; with two monitors at
        // different rates it can be the other monitor's. Slots then keep to
        // the window's monitor's period (FATES_VSYNC_OFF_GRID=0: DWM's always)
        let monitor = *self.fallback_period.lock().unwrap();
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let on = *ON.get_or_init(|| std::env::var("FATES_VSYNC_OFF_GRID").map_or(true, |v| v != "0"));
        if on && (kept.0.period.as_secs_f64() / monitor.as_secs_f64().max(1e-6) - 1.0).abs() >= 0.01 {
            return Grid { blank: kept.0.blank, period: monitor };
        }
        kept.0
    }

    /// mapjit: the display's refresh period (DWM's, else the monitor's
    /// rate), whatever grid presents keep to.
    pub(super) fn display_period(&self) -> Duration {
        self.grid.lock().unwrap().0.period
    }

    /// mapjit: the refresh period of the monitor the window is on, as the
    /// frontend gave it (set_fallback_rate). DWM's timing is the desktop's,
    /// which with two monitors at different rates can be the other one's.
    pub(super) fn monitor_period(&self) -> Duration {
        *self.fallback_period.lock().unwrap()
    }

    /// the monitor's refresh rate, where DWM says nothing.
    pub(super) fn set_fallback_rate(&self, hz: f64) {
        if hz > 1.0 {
            *self.fallback_period.lock().unwrap() = Duration::from_secs_f64(1.0 / hz);
        }
    }

    /// presents handed over and not yet made, and the slot of the last.
    pub(super) fn pending(&self) -> (usize, Option<Instant>) {
        let queue = self.shared.queue.lock().unwrap();
        (queue.pending.len() + queue.busy as usize, queue.last_at)
    }

    /// the slot for the next picture: the refresh after the last one handed
    /// over, or the first one far enough ahead to record a picture for.
    pub(super) fn next_slot(&self) -> Instant {
        let grid = self.grid();
        let (_, last) = self.pending();
        let soonest = Instant::now() + Duration::from_micros(700);
        match last {
            Some(last) if last + grid.period / 2 > soonest => grid.slot_at_or_after(last + grid.period / 2),
            _ => grid.slot_at_or_after(soonest),
        }
    }

    pub(super) fn submit(&self, pending: Pending) {
        let mut queue = self.shared.queue.lock().unwrap();
        if pending.at.is_some() {
            queue.last_at = pending.at;
        }
        queue.pending.push_back(pending);
        drop(queue);
        self.shared.changed.notify_all();
    }

    /// presents everything handed over at once and waits until it is made,
    /// and lets the images kept acquired go, before the swapchain changes
    /// or goes. The thread acquires again after set_swapchain.
    pub(super) fn flush(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.flush = true;
        queue.swapchain = vk::SwapchainKHR::null();
        self.shared.changed.notify_all();
        while !queue.pending.is_empty() || queue.busy || !queue.stock.is_empty() {
            queue = self.shared.changed.wait(queue).unwrap();
        }
        queue.flush = false;
        queue.last_at = None;
    }

    /// whether a present said the swapchain no longer fits, once.
    pub(super) fn take_stale(&self) -> bool {
        self.shared.stale.swap(false, Ordering::AcqRel)
    }
}

impl Drop for PresentThread {
    fn drop(&mut self) {
        self.flush();
        self.shared.queue.lock().unwrap().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // the semaphores once nothing waits for them
        let _ = crate::wait_idle(&self.device);
        for &semaphore in self.semaphores.iter() {
            // SAFETY: the device is idle and the thread gone
            unsafe { self.device.destroy_semaphore(semaphore, None) };
        }
    }
}

fn presenter(
    shared: Arc<Shared>,
    swapchain_lock: Arc<Mutex<()>>,
    swapchain_device: ash::khr::swapchain::Device,
    queue: vk::Queue,
    timing: Option<Arc<dyn Fn(vk::SwapchainKHR, u64) + Send + Sync>>,
    device: ash::Device,
    semaphores: Arc<Vec<vk::Semaphore>>,
) {
    raise_priority();
    let mut turn = 0usize;
    let mut kept_off = 0usize; // rtpace
    loop {
        // rtpace: off the game's core once the frontend names it
        let avoid = KEEP_OFF.load(std::sync::atomic::Ordering::Relaxed);
        if avoid != kept_off {
            kept_off = avoid;
            keep_off_now(avoid);
        }
        // the next picture, once its slot comes (or at once to flush)
        let next = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if q.quit && q.pending.is_empty() {
                    return;
                }
                // images kept acquired for the frontend
                if q.flush && q.pending.is_empty() && !q.busy && !q.stock.is_empty() {
                    // their semaphores were signalled by the acquire and
                    // nothing waits for them: an empty submit does
                    let waits: Vec<vk::Semaphore> = q.stock.drain(..).map(|(_, _, semaphore)| semaphore).collect();
                    let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
                    let submit = vk::SubmitInfo::default().wait_semaphores(&waits).wait_dst_stage_mask(&stages);
                    let _device = crate::device::queue_lock();
                    // SAFETY: the queue is this thread's; the semaphores were
                    // signalled by acquires
                    let _ = unsafe { device.queue_submit(queue, &[submit], vk::Fence::null()) };
                    shared.changed.notify_all();
                    continue;
                }
                if !q.flush && !q.starved && q.swapchain != vk::SwapchainKHR::null() && q.stock.len() < STOCK && !semaphores.is_empty() {
                    let (swapchain, semaphore) = (q.swapchain, semaphores[turn % semaphores.len()]);
                    drop(q);
                    let acquired = {
                        let _swapchain = swapchain_lock.lock().unwrap();
                        // SAFETY: the swapchain is current (the frontend
                        // flushes before changing it), the semaphore unused:
                        // see SEMAPHORES
                        unsafe { swapchain_device.acquire_next_image(swapchain, 0, semaphore, vk::Fence::null()) }
                    };
                    q = shared.queue.lock().unwrap();
                    match acquired {
                        Ok((image, _)) if q.swapchain == swapchain => {
                            turn += 1;
                            q.stock.push_back((swapchain, image, semaphore));
                            shared.changed.notify_all();
                        }
                        Ok((image, _)) => {
                            // the swapchain changed meanwhile: given back at
                            // the flush
                            turn += 1;
                            q.stock.push_back((swapchain, image, semaphore));
                            shared.changed.notify_all();
                        }
                        Err(vk::Result::NOT_READY) | Err(vk::Result::TIMEOUT) => q.starved = true,
                        Err(_) => {
                            q.starved = true;
                            shared.stale.store(true, Ordering::Release);
                        }
                    }
                    continue;
                }
                if let Some(front) = q.pending.front() {
                    let due = front.at.is_none_or(|at| at <= Instant::now()) || q.flush;
                    if due {
                        q.busy = true;
                        break q.pending.pop_front().unwrap();
                    }
                    let left = front.at.unwrap().saturating_duration_since(Instant::now());
                    // the last stretch spun, the OS's wait overshoots
                    if left > Duration::from_micros(1500) {
                        q = shared.changed.wait_timeout(q, left - Duration::from_micros(1200)).unwrap().0;
                    } else {
                        drop(q);
                        let until = Instant::now() + left;
                        while Instant::now() < until {
                            std::hint::spin_loop();
                        }
                        q = shared.queue.lock().unwrap();
                    }
                    continue;
                }
                q = shared.changed.wait(q).unwrap();
            }
        };
        let started = Instant::now();
        let swapchains = [next.swapchain];
        let images = [next.image];
        let waits = [next.semaphore];
        let ids = [next.id];
        let mut present_id = vk::PresentIdKHR::default().present_ids(&ids);
        let mut info = vk::PresentInfoKHR::default().wait_semaphores(&waits).swapchains(&swapchains).image_indices(&images);
        if timing.is_some() {
            info = info.push_next(&mut present_id);
        }
        let result = {
            let _swapchain = swapchain_lock.lock().unwrap();
            let _device = crate::device::queue_lock();
            // SAFETY: the picture's submit signals the semaphore and was made
            // before it was handed over; the queue is this thread's alone,
            // the swapchain and the device kept off by the locks
            unsafe { swapchain_device.queue_present(queue, &info) }
        };
        crate::smooth::since("queue", started, next.id as i64, next.at.map_or(-1.0, |at| (started.saturating_duration_since(at)).as_secs_f64() * 1e3));
        match result {
            Ok(false) => {
                if let Some(timing) = &timing {
                    timing(next.swapchain, next.id);
                }
            }
            Ok(true) => {
                shared.stale.store(true, Ordering::Release);
                if let Some(timing) = &timing {
                    timing(next.swapchain, next.id);
                }
            }
            Err(error) => {
                if error != vk::Result::ERROR_OUT_OF_DATE_KHR {
                    log::warn!("present: {error}");
                }
                shared.stale.store(true, Ordering::Release);
            }
        }
        let mut q = shared.queue.lock().unwrap();
        q.busy = false;
        // an image may be free again
        q.starved = false;
        if q.pending.is_empty() && q.last_at.is_some_and(|at| at <= Instant::now()) {
            q.last_at = None;
        }
        drop(q);
        shared.changed.notify_all();
    }
}

/// rtpace: logical processors the present thread keeps off (the core the
/// game's threads are pinned to: spinning out the last of each slot on its
/// other logical processor took the game's thread's time), see keep_off.
static KEEP_OFF: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// rtpace: the present thread keeps off these logical processors from its
/// next present on (0: anywhere).
pub fn keep_off(mask: usize) {
    KEEP_OFF.store(mask, std::sync::atomic::Ordering::Relaxed);
}

fn keep_off_now(mask: usize) {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn GetCurrentProcess() -> isize;
            fn GetProcessAffinityMask(process: isize, process_mask: *mut usize, system_mask: *mut usize) -> i32;
            fn SetThreadAffinityMask(thread: isize, mask: usize) -> usize;
        }
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: plain queries and a mask within the process's own
        unsafe {
            if GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) != 0 && process & !mask != 0 {
                SetThreadAffinityMask(GetCurrentThread(), process & !mask);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = mask;
}

/// the present thread runs ahead of every other of the program's: a present
/// late by a refresh is a picture shown twice.
fn raise_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        // THREAD_PRIORITY_HIGHEST; it sleeps nearly all the time
        // SAFETY: a plain call on this thread's pseudo-handle
        unsafe { SetThreadPriority(GetCurrentThread(), 2) };
    }
}
