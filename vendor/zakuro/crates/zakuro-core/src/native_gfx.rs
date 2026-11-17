//! gfx: the seam a native renderer (the `fates-gfx` crate) plugs into.
//!
//! The native renderer records the game's draws at the engine level (from
//! game-function overrides), renders them itself, and puts its pictures on
//! the screens here: `transferred` tells it a display transfer into a
//! display buffer ran, so the frame it recorded for that buffer is complete,
//! and `compose` hands it the emulated picture of a screen to draw over or
//! replace. Nothing here knows about PICA200 registers; with no renderer
//! installed every function is a no-op.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// wide2: the headless run's frame number (as ZAKURO_DUMP_FRAMES counts
/// them), for development traces keyed to dumped frames.
pub static HEADLESS_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::sync::Mutex;

use crate::{Screen, System};

/// what a native renderer implements.
pub trait NativeScreens: Send {
    /// a display transfer into the display buffer at `output` (a virtual
    /// address, as the game names it) ran on the emulated GPU.
    fn transferred(&mut self, system: &mut System, output: u32);
    /// the picture for screen `index` (0 top, 1 bottom), given the emulated
    /// picture of the display buffer at `address` (virtual). None keeps the
    /// emulated picture.
    fn compose(&mut self, system: &mut System, index: usize, address: u32, emulated: &Screen) -> Option<Screen>;
    /// the same on the GPU, for a presenter sharing the renderer's device:
    /// `emulated` is the emulated picture's image (none when the emulated
    /// path has none on the GPU), `size` its width and height.
    /// gfx-hfr: `pick` says which picture of the frame's time the
    /// present shows (see Pick); `emulated` is the emulated picture at it.
    #[allow(clippy::too_many_arguments)]
    fn compose_gpu(
        &mut self,
        system: &mut System,
        index: usize,
        address: u32,
        emulated: Option<zakuro_gpu::GpuScreen>,
        size: (u32, u32),
        pick: Pick,
    ) -> Option<zakuro_gpu::GpuScreen>;
    /// gfx-hfr: the native frame for the display buffer at `address` of
    /// screen `index`, by an id that changes with each new frame, and how
    /// its draws matched the frame before's (whether pictures in between
    /// it look right). None: the emulated path alone draws that picture.
    fn newest(&mut self, _index: usize, _address: u32) -> Option<(u64, Option<zakuro_gpu::Matches>)> {
        None
    }
    /// gfx-hfr: the CPU path's picture at `pick`, given the emulated
    /// picture at it (hybrid frames draw over it). None: as compose.
    fn compose_at(&mut self, _system: &mut System, _index: usize, _address: u32, _pick: Pick, _emulated: &Screen) -> Option<Screen> {
        None
    }
    /// rtpace: starts drawing the picture in between compose_gpu will be
    /// asked for at `at` (same arguments) on a thread of the renderer's own,
    /// so the frontend's thread can lend the time to the game's threads
    /// meanwhile (BETWEEN_BUSY says when it is drawn; compose_gpu then
    /// finds it). False when nothing was started (compose_gpu draws it).
    fn begin_between(&mut self, _system: &mut System, _index: usize, _address: u32, _emulated: Option<zakuro_gpu::GpuScreen>, _size: (u32, u32), _at: f32) -> bool {
        false
    }
}

/// rtpace: a picture in between being drawn off the frontend's thread, see
/// NativeScreens::begin_between.
pub static BETWEEN_BUSY: AtomicBool = AtomicBool::new(false);

/// rtpace: real time only: starts drawing off the frontend's thread the
/// screens' pictures in between the next present will show (the pick
/// show_at made), see NativeScreens::begin_between. False when there is
/// none to draw (the present then draws what it needs, as before).
pub fn begin_between(system: &mut System) -> bool {
    if !active() || !crate::pcos::rt::realtime() || BETWEEN_BUSY.load(Ordering::Acquire) {
        return false;
    }
    let mut begun = false;
    for (index, screen) in [zakuro_common::Screen::Top, zakuro_common::Screen::Bottom].into_iter().enumerate() {
        let Some((emulated, address, Pick::Between(at))) = system.native_gfx_emulated(screen) else { continue };
        // (a negative place: in the frame before's time, see Pick)
        if !(at > -1.0 && at < 1.0) || at == 0.0 {
            continue;
        }
        // as gpu_screen sizes them
        let scale = system.gpu.scale();
        let width = match screen {
            zakuro_common::Screen::Top => (screen.width() as f32 * scale as f32 * zakuro_gpu::wide::factor()).round() as u32,
            _ => screen.width() * scale,
        };
        let size = (width, screen.height() * scale);
        let mut hook = HOOK.lock().unwrap();
        begun |= hook.as_mut().is_some_and(|hook| hook.begin_between(system, index, address, emulated, size, at));
    }
    begun
}

/// rtpace: whether a picture begin_between started is still being drawn.
pub fn between_busy() -> bool {
    BETWEEN_BUSY.load(Ordering::Acquire)
}

/// gfx-hfr: which picture of a frame's time a present shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    /// the newest frame's own
    Newest,
    /// the frame before's, the start of the newest one's time
    Previous,
    /// in between the frame before (0) and the newest (1)
    Between(f32),
}

static HOOK: Mutex<Option<Box<dyn NativeScreens>>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SHOWN: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// smooth2: whether each display's newest native frame reads what earlier
/// frames' post effects left (fates-gfx hfr carries): such a frame has no
/// picture in between, and the frontend cross-fades it from the one before.
static NEWEST_CARRIED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// smooth2: the fraction of the frames' scale pictures in between are drawn
/// at, as the frontend's pacing asks (f32 bits; 0: the renderer's own).
static BETWEEN_FRACTION: AtomicU32 = AtomicU32::new(0);

/// smooth2: see BETWEEN_FRACTION.
pub fn set_between_fraction(fraction: f32) {
    BETWEEN_FRACTION.store(fraction.to_bits(), Ordering::Relaxed);
}

/// smooth2: see BETWEEN_FRACTION; none when the frontend asks nothing.
pub fn between_fraction() -> Option<f32> {
    let bits = BETWEEN_FRACTION.load(Ordering::Relaxed);
    (bits != 0).then(|| f32::from_bits(bits))
}

/// perf3: the most samples pictures in between are drawn with while the
/// frontend's pacing has stepped below the lowest scale it may draw them at
/// (0: no cap). See zakuro hfr.rs.
static BETWEEN_SAMPLES: AtomicU32 = AtomicU32::new(0);

/// perf3: see BETWEEN_SAMPLES.
pub fn set_between_samples(samples: u32) {
    BETWEEN_SAMPLES.store(samples, Ordering::Relaxed);
}

/// perf3: see BETWEEN_SAMPLES.
pub fn between_samples() -> Option<u32> {
    let n = BETWEEN_SAMPLES.load(Ordering::Relaxed);
    (n != 0).then_some(n)
}

/// msaa: the samples a pixel the native renderer draws the displays with
/// (pc.msaa, the options' Anti-Aliasing: 1 off, 2, 4, 8), set by the
/// frontend at start and each frame, so a change in the menu takes at once.
static MSAA: AtomicU32 = AtomicU32::new(1);

/// msaa: see MSAA.
pub fn set_msaa(samples: u32) {
    MSAA.store(samples.max(1), Ordering::Relaxed);
}

/// msaa: see MSAA.
pub fn msaa() -> u32 {
    MSAA.load(Ordering::Relaxed)
}

/// smooth2: see NEWEST_CARRIED; set by the native renderer.
pub fn set_newest_carried(index: usize, carried: bool) {
    if let Some(flag) = NEWEST_CARRIED.get(index) {
        flag.store(carried, Ordering::Relaxed);
    }
}

/// smooth2: see NEWEST_CARRIED.
pub fn newest_carried(index: usize) -> bool {
    active() && NEWEST_CARRIED.get(index).is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// installs the native renderer. From then on the presenter takes the
/// screens through `compose` (the CPU path) instead of straight from the
/// emulated GPU's images.
pub fn install(hook: Box<dyn NativeScreens>) {
    *HOOK.lock().unwrap() = Some(hook);
    ACTIVE.store(true, Ordering::Release);
}

/// whether a native renderer is installed.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// qol: at a game speed above 1x the game makes more pictures than the
/// display shows; the renderer then draws only those a shown picture needs
/// (from the newest frame that starts with a clear), not every one.
static LATEST_ONLY: AtomicBool = AtomicBool::new(false);

pub fn set_latest_only(on: bool) {
    LATEST_ONLY.store(on, Ordering::Relaxed);
}

pub fn latest_only() -> bool {
    LATEST_ONLY.load(Ordering::Relaxed)
}

/// gfx-perf: whether a headless run should take both screens every frame
/// (to compare or dump each one), rather than only those it writes out.
static EVERY_SCREEN: AtomicBool = AtomicBool::new(true);

pub fn set_every_screen(every: bool) {
    EVERY_SCREEN.store(every, Ordering::Relaxed);
}

pub fn every_screen() -> bool {
    EVERY_SCREEN.load(Ordering::Relaxed)
}

/// gfx-perf: GPU copies and fills the native renderer did itself, in a
/// frame it draws in full (a post effect's captures and downscales): the
/// emulated GPU skips them, as nothing it shows reads their output. Each is
/// (kind, input, output) in physical addresses, with the frame it was noted
/// in; one not run within two frames is forgotten.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Copy {
    Transfer,
    TextureCopy,
    Fill,
}

static SKIPS: Mutex<Vec<(Copy, u32, u32, u64)>> = Mutex::new(Vec::new());
static SKIP_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// FATES_GFX_SKIP_TRACE=1: each copy noted, left out, missed or forgotten.
fn skip_trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("FATES_GFX_SKIP_TRACE").is_some())
}

pub fn note_skip(kind: Copy, input: u32, output: u32) {
    let frame = SKIP_FRAME.load(Ordering::Relaxed);
    let mut skips = SKIPS.lock().unwrap();
    if skip_trace() {
        for s in skips.iter().filter(|s| s.3 + 2 < frame) { eprintln!("skip-expired {s:X?} at {frame}"); }
        eprintln!("skip-note {kind:?} {input:08X} {output:08X} at {frame}");
    }
    skips.retain(|s| s.3 + 2 >= frame);
    skips.push((kind, input, output, frame));
}

/// a new frame: entries older than two frames go.
pub fn skip_frame() {
    SKIP_FRAME.fetch_add(1, Ordering::Relaxed);
}

/// how many copies were skipped so far.
pub fn skipped() -> u64 {
    SKIPPED.load(Ordering::Relaxed)
}

/// whether the emulated GPU should leave out this copy, which the native
/// renderer noted (taken off the list).
pub(crate) fn skip(kind: Copy, input: u32, output: u32) -> bool {
    if !active() {
        return false;
    }
    let mut skips = SKIPS.lock().unwrap();
    if skips.is_empty() {
        return false;
    }
    // an input of u32::MAX: any input
    match skips.iter().position(|s| s.0 == kind && (s.1 == input || s.1 == u32::MAX) && s.2 == output) {
        Some(at) => {
            if skip_trace() { eprintln!("skip-done {:X?} at {}", skips[at], SKIP_FRAME.load(Ordering::Relaxed)); }
            skips.remove(at);
            SKIPPED.fetch_add(1, Ordering::Relaxed);
            count_work(|w| w.ops_skipped += 1); // gfx-final
            true
        }
        None => {
            if skip_trace() { eprintln!("skip-miss {kind:?} {input:08X} {output:08X} at {}", SKIP_FRAME.load(Ordering::Relaxed)); }
            false
        }
    }
}

/// gfx-final: what the emulated GPU still did while a native renderer is
/// installed (an audit of the work left to port): command lists walked,
/// the draws they made, display transfers, texture copies and memory fills
/// run, and the GSP operations left out because the native renderer did
/// them or said nothing reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmulatedWork {
    pub lists: u64,
    pub lists_with_draws: u64,
    pub draws: u64,
    pub transfers: u64,
    pub copies: u64,
    pub fills: u64,
    /// command lists left out (`emulated_wanted`)
    pub lists_skipped: u64,
    /// transfers, copies and fills left out
    pub ops_skipped: u64,
}

static WORK: Mutex<EmulatedWork> = Mutex::new(EmulatedWork {
    lists: 0,
    lists_with_draws: 0,
    draws: 0,
    transfers: 0,
    copies: 0,
    fills: 0,
    lists_skipped: 0,
    ops_skipped: 0,
});

/// gfx-final: the work counted so far.
pub fn emulated_work() -> EmulatedWork {
    *WORK.lock().unwrap()
}

pub(crate) fn count_work(f: impl FnOnce(&mut EmulatedWork)) {
    if active() {
        f(&mut WORK.lock().unwrap());
    }
}

/// gfx-final: dev, FATES_GFX_EMU_OPS=N logs the first N GSP operations
/// the emulated GPU runs in a native session.
pub(crate) fn trace_op(what: &str, a: u32, b: u32) {
    static LEFT: std::sync::OnceLock<std::sync::atomic::AtomicU64> = std::sync::OnceLock::new();
    let left = LEFT.get_or_init(|| std::sync::atomic::AtomicU64::new(std::env::var("FATES_GFX_EMU_OPS").ok().and_then(|v| v.parse().ok()).unwrap_or(0)));
    static FROM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let from = *FROM.get_or_init(|| std::env::var("FATES_GFX_EMU_OPS_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
    if active() && SKIP_FRAME.load(Ordering::Relaxed) >= from && left.load(Ordering::Relaxed) > 0 {
        left.fetch_sub(1, Ordering::Relaxed);
        eprintln!("emu-op {what} {a:08X} {b:08X} at gfx frame {} vblank {}", SKIP_FRAME.load(Ordering::Relaxed), VBLANK.load(Ordering::Relaxed));
    }
}

/// gfx-final: whether the emulated GPU is needed right now. The native
/// renderer hands out a lease (`lease_native`) while it draws every
/// picture in full and nothing it reads is left for the emulated GPU to
/// render: until the lease runs out, command lists, fills, copies and
/// transfers are left out. A lease lasts a few VBlanks, so when the game
/// stops drawing through the hooks the native renderer sees (or the
/// renderer withdraws it with `want_emulated`), the emulated GPU runs again.
static LEASE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// the VBlank count now (System::frames), kept by `tick`
static VBLANK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const LEASE_VBLANKS: u64 = 4;

/// the native renderer: nothing needs the emulated GPU for now.
pub fn lease_native() {
    LEASE.store(VBLANK.load(Ordering::Relaxed) + LEASE_VBLANKS, Ordering::Relaxed);
}

/// gfx-final: the native renderer, installed before the game starts: the
/// set-up the SDK does before the game's first frame (clearing VRAM,
/// showing a blank buffer) is left out too, for `vblanks` VBlanks at most
/// (the renderer's first frame hands out its own lease or withdraws it).
pub fn lease_boot(vblanks: u64) {
    BOOT.store(vblanks, Ordering::Relaxed);
    LEASE.store(u64::MAX, Ordering::Relaxed);
}

/// the boot lease's length, taken from the first VBlank counted (a saved
/// state loaded at start counts from its own frame)
static BOOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// the native renderer: something needs the emulated GPU from now on.
pub fn want_emulated() {
    LEASE.store(0, Ordering::Relaxed);
}

pub(crate) fn tick(vblanks: u64) {
    let before = VBLANK.swap(vblanks, Ordering::Relaxed);
    // a saved state loaded: the frame count jumped, a lease running goes on
    // from the new count
    let lease = LEASE.load(Ordering::Relaxed);
    if vblanks != before + 1 && lease != u64::MAX && lease >= before {
        LEASE.store(vblanks + (lease - before), Ordering::Relaxed);
    }
    let boot = BOOT.swap(0, Ordering::Relaxed);
    if boot > 0 && LEASE.load(Ordering::Relaxed) == u64::MAX {
        LEASE.store(vblanks + boot, Ordering::Relaxed);
    }
}

/// native-rt: a refresh the game's threads had no time before (in real
/// time, the frontend catching up after a stall raises several at once):
/// a lease running does not count it, as the game could not draw in it.
pub(crate) fn hold_lease() {
    let lease = LEASE.load(Ordering::Relaxed);
    if lease != u64::MAX && lease >= VBLANK.load(Ordering::Relaxed) {
        LEASE.store(lease + 1, Ordering::Relaxed);
    }
}

/// native-rt: the lease and the VBlank count, for diagnostics.
pub(crate) fn lease_state() -> (u64, u64) {
    (LEASE.load(Ordering::Relaxed), VBLANK.load(Ordering::Relaxed))
}

pub(crate) fn emulated_wanted() -> bool {
    !active() || LEASE.load(Ordering::Relaxed) < VBLANK.load(Ordering::Relaxed).max(1)
}

/// the display buffer (virtual address) a screen's emulated picture was
/// just read from.
pub(crate) fn note_shown(index: usize, address: u32) {
    if let Some(slot) = SHOWN.get(index) {
        slot.store(address, Ordering::Relaxed);
    }
}

pub(crate) fn transferred(system: &mut System, output_physical: u32) {
    if !active() {
        return;
    }
    let output = crate::services::gsp::physical_to_virtual(system, output_physical);
    if let Some(hook) = HOOK.lock().unwrap().as_mut() {
        hook.transferred(system, output);
    }
}

/// a screen's picture on the GPU with the native renderer's drawing, for a
/// presenter sharing the device. None: take the CPU path (read_screen_scaled).
pub fn gpu_screen(system: &mut System, screen: zakuro_common::Screen) -> Option<(zakuro_gpu::GpuScreen, u32, u32)> {
    if !active() {
        return None;
    }
    let index = if screen == zakuro_common::Screen::Top { 0 } else { 1 };
    let (emulated, address, pick) = system.native_gfx_emulated(screen)?; // gfx-hfr: pick
    let scale = system.gpu.scale();
    // wide: the top screen's picture is wider than the console's
    let width = match screen {
        zakuro_common::Screen::Top => (screen.width() as f32 * scale as f32 * zakuro_gpu::wide::factor()).round() as u32,
        _ => screen.width() * scale,
    };
    let size = (width, screen.height() * scale);
    let mut hook = HOOK.lock().unwrap();
    let picture = hook.as_mut()?.compose_gpu(system, index, address, emulated, size, pick)?;
    Some((picture, size.0, size.1))
}

pub(crate) fn compose(system: &mut System, index: usize, picture: Screen) -> Screen {
    if !active() || index > 1 {
        return picture;
    }
    let address = SHOWN[index].load(Ordering::Relaxed);
    let mut hook = HOOK.lock().unwrap();
    match hook.as_mut().and_then(|hook| hook.compose(system, index, address, &picture)) {
        Some(native) => native,
        None => picture,
    }
}

/// gfx-hfr: the display buffer (virtual) a screen shows, from the LCD's
/// framebuffer configuration: no emulated picture needed.
pub(crate) fn shown_address(system: &mut System, index: usize) -> Option<u32> {
    let address = system.gpu.framebuffers.get(index)?.address_left();
    (address != 0).then(|| crate::services::gsp::physical_to_virtual(system, address))
}

/// gfx-hfr: the native frame a screen shows, see NativeScreens::newest.
pub(crate) fn newest(system: &mut System, index: usize) -> Option<(u64, Option<zakuro_gpu::Matches>)> {
    if !active() {
        return None;
    }
    let address = shown_address(system, index)?;
    HOOK.lock().unwrap().as_mut()?.newest(index, address)
}

/// gfx-hfr: a screen's picture at `pick` on the CPU path, over the emulated
/// picture at it; None when the native renderer has none of its own there.
pub(crate) fn compose_at(system: &mut System, index: usize, pick: Pick, emulated: &Screen) -> Option<Screen> {
    if !active() {
        return None;
    }
    let address = shown_address(system, index)?;
    HOOK.lock().unwrap().as_mut()?.compose_at(system, index, address, pick, emulated)
}
