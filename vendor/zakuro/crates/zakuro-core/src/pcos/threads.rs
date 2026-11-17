//! os2: the game's threads as host threads.
//!
//! Every guest thread (nn::os::Thread, and the SDK's own threads: the GPU
//! interrupt receiver, the sound thread, the applet thread, the loader) runs
//! its recompiled code on a host thread of its own, created when the thread
//! first runs and ended when it ends. Exactly one of them runs guest code at
//! a time: the game's code assumes the 3DS's strict priority scheduling on
//! one core, and runs must stay deterministic for replays. The one allowed
//! to run holds the *baton* (`Shared::turn`); everyone else is parked on a
//! host primitive (`std::thread::park`), which is how a blocking wait
//! (ArbitrateAddress, WaitSynchronization, SleepThread) blocks: the thread
//! that waits parks, and the scheduler hands the baton to the thread it
//! picks, in the same fixed order every run (pcos::sched).
//!
//! The frame driver (the thread calling System::run_frame, the frontend's
//! main thread) hands the baton to the guest threads at the start of a frame
//! and gets it back when the guest clock reaches the frame's end, or the game
//! exits.
//!
//! A thread that waits gives up its host stack first: the recompiled code
//! unwinds to the thread's loop (as it did on Zakuro's single loop), and
//! enters again at r15 when the thread next runs. That keeps the code's
//! cycle accounting, and so every switch and every result, identical to the
//! single-loop runs the replays were recorded with. (Keeping the stack
//! across a wait is possible, since each thread has its own, but would cut
//! budget blocks differently, so replays would drift.)
//!
//! FATES_NATIVE_THREADS=0 runs every thread on the driver as before.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, Thread};

use crate::kernel::thread::{ThreadId, ThreadStatus};
use crate::{StepOutcome, System};

/// the stack each host thread gets: the recompiled code makes one host call
/// per guest call, as on the main thread (source/build.rs gives it 8 MiB).
const STACK: usize = 8 << 20;

/// how long a thread looks for the baton before it parks, in microseconds
/// (FATES_THREAD_SPIN_US): a hand-off to a thread still looking costs well
/// under a microsecond, waking a parked one tens of them on Windows.
fn spin_time() -> std::time::Duration {
    static US: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    std::time::Duration::from_micros(*US.get_or_init(|| std::env::var("FATES_THREAD_SPIN_US").ok().and_then(|v| v.parse().ok()).unwrap_or(0)))
}

/// where the host threads run (FATES_THREAD_CORE). "pin" (the default)
/// keeps them all, the driver too, on the core the first frame ran on (its
/// two logical processors), so the one thread running at a time always
/// finds a warm cache, as the 3DS's application core did. Measured on the
/// 3000-frame save replay, back to back, the old single loop 17.0-17.9 s:
/// pinned 11.9-13.2 s, left to Windows ("off") 18.2-22.6 s, the same ideal
/// processor for all ("ideal") 17.2-22.5 s, the ideal processor following
/// the baton ("follow") 24-47 s. A pinned core can be taken by other
/// work, though: when hand-offs keep waiting for it (see run_until), every
/// host thread is unpinned for the rest of the session.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Core {
    Follow,
    Pin,
    Ideal,
    Off,
}

fn core_mode() -> Core {
    static MODE: std::sync::OnceLock<Core> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("FATES_THREAD_CORE").as_deref() {
        Ok("pin") => Core::Pin,
        Ok("off") => Core::Off,
        Ok("follow") => Core::Follow,
        Ok("ideal") => Core::Ideal,
        _ => Core::Pin,
    })
}

#[cfg(windows)]
mod win {
    pub type Handle = isize;
    unsafe extern "system" {
        pub fn GetCurrentThread() -> Handle;
        pub fn GetCurrentProcess() -> Handle;
        pub fn GetCurrentProcessorNumber() -> u32;
        pub fn SetThreadIdealProcessor(thread: Handle, ideal: u32) -> u32;
        pub fn SetThreadAffinityMask(thread: Handle, mask: usize) -> usize;
        pub fn SetThreadPriority(thread: Handle, priority: i32) -> i32;
        pub fn GetProcessAffinityMask(process: Handle, process_mask: *mut usize, system_mask: *mut usize) -> i32;
        pub fn DuplicateHandle(process: Handle, source: Handle, target_process: Handle, target: *mut Handle, access: u32, inherit: i32, options: u32) -> i32;
    }
    pub const DUPLICATE_SAME_ACCESS: u32 = 2;

    /// a real handle to the calling thread.
    pub fn current_thread() -> Handle {
        let mut handle = 0;
        // SAFETY: plain Win32 calls on pseudo-handles of this process
        unsafe { DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &mut handle, 0, 0, DUPLICATE_SAME_ACCESS) };
        handle
    }
}

/// steers the thread about to get the baton onto a core, see Core.
fn steer(handle: isize, pinned: u32, first: bool) {
    #[cfg(windows)]
    // SAFETY: handle is a live thread handle of this process (a host
    // thread's, or the driver's duplicate), or 0, which the calls refuse
    unsafe {
        // smooth: the game's threads (one runs at a time) a step above
        // normal, so busy normal threads on their core wait for them rather
        // than the other way round; FATES_THREAD_PRIORITY=0 leaves them
        if handle != 0 && first && raised() {
            win::SetThreadPriority(handle, 1);
        }
        match core_mode() {
            Core::Follow if handle != 0 && !first => {
                win::SetThreadIdealProcessor(handle, win::GetCurrentProcessorNumber());
            }
            Core::Ideal if handle != 0 && first && pinned < usize::BITS => {
                win::SetThreadIdealProcessor(handle, pinned);
            }
            Core::Pin if handle != 0 && first && pinned + 1 < usize::BITS && !UNPINNED.load(Ordering::Relaxed) => {
                // the core's logical processors (smooth: from the CPU sets,
                // else the SMT pair Windows numbers next to each other)
                let mask = cpu_sets::core_mask(pinned).unwrap_or(3usize << (pinned & !1));
                let before = win::SetThreadAffinityMask(handle, mask);
                log::debug!("os2: pinned a host thread to processors 0x{mask:X} (was 0x{before:X})");
            }
            _ => {}
        }
    }
    #[cfg(not(windows))]
    let _ = (handle, pinned, first);
}

/// smooth: the processors' cores and performance classes, from Windows'
/// CPU sets (GetSystemCpuSetInformation), for choosing where Core::Pin pins.
/// On a hybrid CPU (the i9-13900KS: 8 performance cores with two logical
/// processors each, 16 efficiency cores) the first frame can run on an
/// efficiency core; the game then ran pinned there, about half as fast.
mod cpu_sets {
    /// (logical processor, core, efficiency class) of group 0's processors.
    fn processors() -> &'static [(u32, u32, u8)] {
        static LIST: std::sync::OnceLock<Vec<(u32, u32, u8)>> = std::sync::OnceLock::new();
        LIST.get_or_init(|| {
            let mut list = Vec::new();
            #[cfg(windows)]
            {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetSystemCpuSetInformation(info: *mut u8, length: u32, returned: *mut u32, process: isize, flags: u32) -> i32;
                }
                let mut needed = 0u32;
                // SAFETY: a size query, then a buffer of that size
                unsafe { GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut needed, 0, 0) };
                let mut buffer = vec![0u8; needed as usize];
                if needed > 0 && unsafe { GetSystemCpuSetInformation(buffer.as_mut_ptr(), needed, &mut needed, 0, 0) } != 0 {
                    let mut at = 0usize;
                    while at + 20 <= needed as usize {
                        let size = u32::from_le_bytes(buffer[at..at + 4].try_into().unwrap()) as usize;
                        let kind = u32::from_le_bytes(buffer[at + 4..at + 8].try_into().unwrap());
                        // CpuSetInformation: Id u32, Group u16, LogicalProcessorIndex u8,
                        // CoreIndex u8, LastLevelCacheIndex u8, NumaNodeIndex u8, EfficiencyClass u8
                        if kind == 0 && size >= 20 && u16::from_le_bytes(buffer[at + 12..at + 14].try_into().unwrap()) == 0 {
                            list.push((buffer[at + 14] as u32, buffer[at + 15] as u32, buffer[at + 18]));
                        }
                        if size == 0 {
                            break;
                        }
                        at += size;
                    }
                }
            }
            list
        })
    }

    /// the logical processors of the core a processor is on.
    pub fn core_mask(processor: u32) -> Option<usize> {
        let list = processors();
        let core = list.iter().find(|p| p.0 == processor)?.1;
        let mask = list.iter().filter(|p| p.1 == core && p.0 < usize::BITS).fold(0usize, |mask, p| mask | 1 << p.0);
        (mask != 0).then_some(mask)
    }

    /// rtpace: `first`'s core when no other Haiku process has claimed it,
    /// else the first other core of the fastest class (not core 0) none has;
    /// the claim is a named mutex per core, held until the process ends. The
    /// processor to pin to (`first` when every one is claimed).
    pub fn claim(first: u32) -> u32 {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn CreateMutexW(attributes: *const u8, initial_owner: i32, name: *const u16) -> isize;
                fn GetLastError() -> u32;
                fn CloseHandle(handle: isize) -> i32;
            }
            const ERROR_ALREADY_EXISTS: u32 = 183;
            let list = processors();
            let Some(best) = list.iter().map(|p| p.2).max() else { return first };
            let core0 = list.iter().find(|p| p.0 == 0).map(|p| p.1);
            let first_core = list.iter().find(|p| p.0 == first).map(|p| p.1);
            let mut cores: Vec<(u32, u32)> = Vec::new(); // (core, its first processor)
            for p in list.iter().filter(|p| p.2 == best && Some(p.1) != core0) {
                if !cores.iter().any(|c| c.0 == p.1) {
                    cores.push((p.1, p.0));
                }
            }
            cores.sort_by_key(|c| (Some(c.0) != first_core, c.1));
            for (core, processor) in cores {
                let name: Vec<u16> = format!(r"Local\Haiku-game-core-{core}").encode_utf16().chain([0]).collect();
                // SAFETY: a plain call with a NUL-terminated name
                let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
                if handle == 0 {
                    continue;
                }
                // SAFETY: read straight after the call that set it
                if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                    unsafe { CloseHandle(handle) };
                    continue;
                }
                // kept open: the claim lasts as long as the process
                return processor;
            }
        }
        first
    }

    /// the processor itself when it is on a core of the fastest class other
    /// than core 0 (where much of the system's interrupt and DPC work goes),
    /// else the first processor of such a core.
    pub fn performance_processor(current: u32) -> u32 {
        let list = processors();
        let Some(best) = list.iter().map(|p| p.2).max() else { return current };
        let core0 = list.iter().find(|p| p.0 == 0).map(|p| p.1);
        if list.iter().any(|p| p.0 == current && p.2 == best && Some(p.1) != core0) {
            return current;
        }
        list.iter().find(|p| p.2 == best && Some(p.1) != core0).or_else(|| list.iter().find(|p| p.2 == best)).map_or(current, |p| p.0)
    }
}

/// rtpace: the logical processors of the core the game's threads are
/// pinned to (Core::Pin), 0 when they are not pinned: other busy threads of
/// the program (the drawing thread of pictures in between) keep off it, so
/// they do not share the game's core through its other logical processor.
pub fn game_core_mask() -> usize {
    match PINNED_CPU.load(Ordering::Relaxed) {
        u64::MAX => 0,
        _ if UNPINNED.load(Ordering::Relaxed) || core_mode() != Core::Pin => 0,
        cpu => cpu_sets::core_mask(cpu as u32).unwrap_or(3usize << (cpu as u32 & !1)),
    }
}

/// rtpace: the processor Core::Pin chose, for game_core_mask.
static PINNED_CPU: AtomicU64 = AtomicU64::new(u64::MAX);

/// smooth: whether the game's threads run above normal priority.
fn raised() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("FATES_THREAD_PRIORITY").is_ok_and(|v| v == "0"))
}

/// smooth: the frontend found its thread (the driver, pinned with the
/// game's) woken late, see report_late.
static LATE: AtomicBool = AtomicBool::new(false);

/// smooth: the frontend's thread woke from a wait several milliseconds late,
/// a sign the pinned core is taken; counts as a slow frame in run_until.
pub fn report_late() {
    LATE.store(true, Ordering::Relaxed);
}

/// set once the pinned core proved busy, see run_until.
static UNPINNED: AtomicBool = AtomicBool::new(false);

/// a frame whose hand-offs waited this long in all for their thread to run
/// found the pinned core busy; this many of them within SLOW_WINDOW frames
/// unpin the threads.
const SLOW_FRAME: u64 = 4_000_000;
const SLOW_FRAMES: usize = 3;
const SLOW_WINDOW: u64 = 600;

/// lets every host thread, and the driver, run on any processor from now on.
fn unpin(handles: impl Iterator<Item = isize>) {
    UNPINNED.store(true, Ordering::Relaxed);
    #[cfg(windows)]
    // SAFETY: plain Win32 calls on this process's thread handles
    unsafe {
        let (mut process, mut system) = (0usize, 0usize);
        if win::GetProcessAffinityMask(win::GetCurrentProcess(), &mut process, &mut system) == 0 || process == 0 {
            return;
        }
        for handle in handles.filter(|&handle| handle != 0) {
            win::SetThreadAffinityMask(handle, process);
        }
    }
    #[cfg(not(windows))]
    let _ = handles.count();
}

/// whether the game's threads run on host threads, unless
/// FATES_NATIVE_THREADS=0.
pub fn enabled() -> bool {
    // native-final: the pure build has no other scheduler
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    return from_env();
}

#[cfg(feature = "emulation")]
fn from_env() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("FATES_NATIVE_THREADS").map_or(true, |value| value != "0");
        log::info!("os2: the game's threads run {}", if on { "on host threads" } else { "on the frame driver (FATES_NATIVE_THREADS=0)" });
        on
    })
}

/// who may run: the driver, or a guest thread by its key.
type Seat = u64;
const DRIVER: Seat = 0;

/// a guest thread's seat: its guest id (never reused), plus one.
fn seat_of(system: &System, id: ThreadId) -> Seat {
    system.kernel.thread(id).guest_id as Seat + 1
}

/// the system pointer, handed between host threads with the baton.
#[derive(Clone, Copy)]
struct SystemPtr(*mut System);
// SAFETY: only the holder of the baton dereferences it, and handing the
// baton on (a release store, an acquire load) orders everything it did
// before the next holder's accesses.
unsafe impl Send for SystemPtr {}

/// what the host threads share.
struct Shared {
    /// the seat allowed to run.
    turn: AtomicU64,
    /// the frame's end, in guest ticks.
    deadline: AtomicU64,
    /// the system, set by the driver at each frame.
    system: Mutex<SystemPtr>,
    /// the driver's host thread, to wake it.
    driver: Mutex<Option<Thread>>,
    /// a guest thread's panic, re-raised on the driver.
    panic: Mutex<Option<Box<dyn std::any::Any + Send>>>,
    /// counters for the summary: hand-offs between host threads, parks.
    handoffs: AtomicU64,
    parks: AtomicU64,
    /// when the baton was last handed on, and the total time from a hand-off
    /// to the receiver running, in nanoseconds since base.
    base: std::time::Instant,
    handed_at: AtomicU64,
    latency: AtomicU64,
    /// the driver's thread handle, and the core Core::Pin keeps them on.
    driver_handle: AtomicU64,
    pinned: AtomicU64,
}

/// one guest thread's host thread.
struct Host {
    /// the guest thread's slot in the kernel's table.
    slot: ThreadId,
    thread: Thread,
    /// its Win32 handle, for steer
    handle: isize,
    quit: Arc<AtomicBool>,
    _join: JoinHandle<()>,
}

/// the host threads of a system.
pub struct HostThreads {
    shared: Arc<Shared>,
    hosts: HashMap<Seat, Host>,
    /// host threads started and ended, for the summary.
    started: u64,
    ended: u64,
    /// the frames whose hand-offs waited long for the pinned core.
    slow: std::collections::VecDeque<u64>,
}

impl Default for HostThreads {
    fn default() -> Self {
        HostThreads {
            shared: Arc::new(Shared {
                turn: AtomicU64::new(DRIVER),
                deadline: AtomicU64::new(0),
                system: Mutex::new(SystemPtr(std::ptr::null_mut())),
                driver: Mutex::new(None),
                panic: Mutex::new(None),
                handoffs: AtomicU64::new(0),
                parks: AtomicU64::new(0),
                base: std::time::Instant::now(),
                handed_at: AtomicU64::new(0),
                latency: AtomicU64::new(0),
                driver_handle: AtomicU64::new(0),
                pinned: AtomicU64::new(u64::MAX),
            }),
            hosts: HashMap::new(),
            started: 0,
            ended: 0,
            slow: Default::default(),
        }
    }
}

impl Drop for HostThreads {
    fn drop(&mut self) {
        for (_, host) in self.hosts.drain() {
            host.quit.store(true, Ordering::Release);
            host.thread.unpark();
        }
    }
}

impl HostThreads {
    /// a summary line for the end of a run.
    pub fn summary(&self) -> String {
        format!(
            "os2: host threads: {} live, {} started, {} ended; {} hand-offs ({:.2?} from hand-off to running), {} parks; boot3: {} LDREX reservations cleared after the frontend ran",
            self.hosts.len(),
            self.started,
            self.ended,
            self.shared.handoffs.load(Ordering::Relaxed),
            std::time::Duration::from_nanos(self.shared.latency.load(Ordering::Relaxed)),
            self.shared.parks.load(Ordering::Relaxed),
            broken_reservations()
        )
    }

    /// savehang: who holds the baton and how the host threads are placed,
    /// for pcos::hang's dump.
    pub fn baton(&self) -> String {
        let turn = self.shared.turn.load(Ordering::Acquire);
        format!(
            "baton with {}, threads {}",
            if turn == DRIVER { "the frontend".to_owned() } else { format!("guest id {}", turn - 1) },
            if UNPINNED.load(Ordering::Relaxed) { "unpinned (the pinned core was busy)" } else { "as placed at start" }
        )
    }

    /// the host thread of a seat, started if it has none yet.
    fn host(&mut self, seat: Seat, slot: ThreadId, name: &str, pinned: u32) -> &Host {
        let started = &mut self.started;
        let shared = &self.shared;
        self.hosts.entry(seat).or_insert_with(|| {
            *started += 1;
            let quit = Arc::new(AtomicBool::new(false));
            let (shared, stop) = (shared.clone(), quit.clone());
            let join = std::thread::Builder::new()
                .name(format!("guest {name}"))
                .stack_size(STACK)
                .spawn(move || guest_main(shared, seat, stop))
                .expect("os2: a host thread for a guest thread");
            #[cfg(windows)]
            let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&join) as isize;
            #[cfg(not(windows))]
            let handle = 0;
            steer(handle, pinned, true);
            Host { slot, thread: join.thread().clone(), handle, quit, _join: join }
        })
    }

    /// ends the host threads whose guest thread ended or is gone (a state
    /// was loaded), parked as they are.
    fn reconcile(&mut self, system: &System) {
        let live = |seat: Seat, host: &Host| {
            system.kernel.threads.get(host.slot as usize).is_some_and(|thread| {
                thread.guest_id as Seat + 1 == seat && thread.status != ThreadStatus::Dead
            })
        };
        let gone: Vec<Seat> = self.hosts.iter().filter(|(&seat, host)| !live(seat, host)).map(|(&seat, _)| seat).collect();
        for seat in gone {
            if let Some(host) = self.hosts.remove(&seat) {
                host.quit.store(true, Ordering::Release);
                host.thread.unpark();
                self.ended += 1;
            }
        }
    }
}

/// waits for the baton; false when the thread is to end instead.
fn wait_turn(shared: &Shared, seat: Seat, quit: Option<&AtomicBool>) -> bool {
    let mut spins = 0u32;
    let mut since: Option<std::time::Instant> = None;
    loop {
        if shared.turn.load(Ordering::Acquire) == seat {
            let now = shared.base.elapsed().as_nanos() as u64;
            shared.latency.fetch_add(now.saturating_sub(shared.handed_at.load(Ordering::Relaxed)), Ordering::Relaxed);
            return true;
        }
        if quit.is_some_and(|quit| quit.load(Ordering::Acquire)) {
            return false;
        }
        spins = spins.wrapping_add(1);
        if spins % 64 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let started = *since.get_or_insert_with(std::time::Instant::now);
        if started.elapsed() < spin_time() {
            // let a thread waiting for this core have it
            std::thread::yield_now();
        } else {
            shared.parks.fetch_add(1, Ordering::Relaxed);
            std::thread::park();
        }
    }
}

/// hands the baton to a seat, starting its host thread if it has none.
fn hand(system: &mut System, to: Seat) {
    let shared = system.host_threads.shared.clone();
    shared.handoffs.fetch_add(1, Ordering::Relaxed);
    // native-rt: why, counted (the seat handing over holds the baton)
    let from = shared.turn.load(Ordering::Relaxed);
    super::rt::note_handoff(system, (from != DRIVER).then(|| (from - 1) as u32), to == DRIVER);
    let pinned = shared.pinned.load(Ordering::Relaxed) as u32;
    let thread = if to == DRIVER {
        steer(shared.driver_handle.load(Ordering::Relaxed) as isize, pinned, false);
        shared.driver.lock().unwrap().clone()
    } else {
        let slot = system.kernel.current_thread.expect("a guest thread is current");
        let name = system.kernel.thread(slot).name.clone();
        let host = system.host_threads.host(to, slot, &name, pinned);
        steer(host.handle, pinned, false);
        Some(host.thread.clone())
    };
    // rtpace: dev, a game tick that started in the run ending now
    if super::rtpace::wanted() {
        let run_start = shared.base + std::time::Duration::from_nanos(shared.handed_at.load(Ordering::Relaxed));
        super::rtpace::handoff(system, run_start, (from != DRIVER).then(|| (from - 1) as u32));
    }
    shared.handed_at.store(shared.base.elapsed().as_nanos() as u64, Ordering::Relaxed);
    shared.turn.store(to, Ordering::Release);
    if let Some(thread) = thread {
        thread.unpark();
    }
}

/// runs steps for as long as the baton's holder is the current thread;
/// prepared says the scheduling part of the step was done (by the thread
/// that handed the baton over). The seat to hand the baton to.
fn drive(system: &mut System, deadline: u64, seat: Seat, mut prepared: bool) -> Seat {
    loop {
        if !prepared {
            // native-rt: in real time, the lend's end
            if super::rt::over(system, deadline) || system.exited || system.broke {
                return DRIVER;
            }
            system.coverage.profile.steps += 1;
            match system.step_prepare(Some(deadline)) {
                Some(StepOutcome::Ran) => continue,
                Some(_) => return DRIVER,
                None => {}
            }
            let current = system.kernel.current_thread.expect("a thread runs after a scheduling pass that found one");
            let next = seat_of(system, current);
            if next != seat {
                return next;
            }
        }
        prepared = false;
        // native-rt: with FATES_SCHED_STATS, a step that took long is logged
        let started = super::rt::stats_wanted().then(std::time::Instant::now);
        let (pc, lr) = (system.cpu.regs[15], system.cpu.regs[14]);
        let outcome = system.step_run(Some(deadline));
        if let Some(took) = started.map(|started| started.elapsed()).filter(|took| took.as_millis() >= 50) {
            let thread = system.kernel.current().map_or("?".to_owned(), |thread| thread.name.clone());
            eprintln!("sched: a step of {thread} from 0x{pc:08X} (lr 0x{lr:08X}) took {took:.1?}, now at 0x{:08X}, frame {}", system.cpu.regs[15], system.frames);
        }
        if outcome != StepOutcome::Ran {
            return DRIVER;
        }
    }
}

/// a guest thread's host thread: waits for the baton, runs the thread's
/// steps, hands the baton on.
fn guest_main(shared: Arc<Shared>, seat: Seat, quit: Arc<AtomicBool>) {
    while wait_turn(&shared, seat, Some(&quit)) {
        let SystemPtr(system) = *shared.system.lock().unwrap();
        // SAFETY: the baton gives this thread the system, see SystemPtr
        let system = unsafe { &mut *system };
        let deadline = shared.deadline.load(Ordering::Relaxed);
        let next = match std::panic::catch_unwind(AssertUnwindSafe(|| drive(system, deadline, seat, true))) {
            Ok(next) => next,
            Err(payload) => {
                *shared.panic.lock().unwrap() = Some(payload);
                DRIVER
            }
        };
        hand(system, next);
    }
}

/// boot3: reservations a run_until found left over from the run before
/// (a thread stopped between LDREX and STREX), and cleared.
static BROKEN_RESERVATIONS: AtomicU64 = AtomicU64::new(0);

/// boot3: the count of BROKEN_RESERVATIONS, for the summary.
pub fn broken_reservations() -> u64 {
    BROKEN_RESERVATIONS.load(Ordering::Relaxed)
}

/// boot3: clears the running thread's exclusive monitor, true when it was
/// set. Real time only, unless FATES_CLREX=all (deterministic runs keep the
/// cuts the replays were recorded with; FATES_CLREX=0 turns it off, for
/// comparing).
fn exclusive_cleared(system: &mut System) -> bool {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    let mode = *MODE.get_or_init(|| match std::env::var("FATES_CLREX").as_deref() {
        Ok("0") => 0,
        Ok("all") => 2,
        _ => 1,
    });
    let applies = match mode {
        0 => false,
        1 => super::rt::realtime(),
        _ => true,
    };
    applies && system.cpu.exclusive_addr.take().is_some()
}

/// runs the guest threads until the guest clock reaches deadline (the
/// frame's end), the game exits or faults. Called by the frame driver.
pub fn run_until(system: &mut System, deadline: u64) -> StepOutcome {
    let shared = system.host_threads.shared.clone();
    // boot3: the frontend ran between this run and the last (the refresh:
    // the GPU's interrupt queue and command ring written natively, the
    // frontend's own pokes), as an interrupt would on the console, whose
    // return from it clears the exclusive monitor. A thread stopped between
    // its LDREX and STREX (a budget check ends a lend there) must not have
    // its STREX succeed over what was written meanwhile: it lost the
    // interrupt queue's newest entries (P3D, so the game waited forever for
    // its command list at boot, a black screen) or the command ring's count.
    // Its STREX now fails and the game's loop reads the word again.
    if exclusive_cleared(system) {
        BROKEN_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
    }
    {
        let mut host_threads = std::mem::take(&mut system.host_threads);
        host_threads.reconcile(system);
        system.host_threads = host_threads;
    }
    *shared.system.lock().unwrap() = SystemPtr(system);
    *shared.driver.lock().unwrap() = Some(std::thread::current());
    #[cfg(windows)]
    if shared.driver_handle.load(Ordering::Relaxed) == 0 {
        shared.driver_handle.store(win::current_thread() as u64, Ordering::Relaxed);
        // SAFETY: a plain query
        // FATES_THREAD_PIN_CPU picks the processor, for tests
        let cpu = std::env::var("FATES_THREAD_PIN_CPU").ok().and_then(|v| v.parse().ok());
        // smooth: a performance core, the one the first frame ran on if it
        // is one (on a hybrid CPU it can be an efficiency core)
        let chosen = cpu.unwrap_or_else(|| cpu_sets::performance_processor(unsafe { win::GetCurrentProcessorNumber() }));
        // rtpace: a core no other Haiku running has its game's threads on
        // (each chose the same first performance core, so a test run or a
        // second game took the game's core in turns: lends overran by 20-40 ms)
        let chosen = if cpu.is_none() && core_mode() == Core::Pin { cpu_sets::claim(chosen) } else { chosen };
        shared.pinned.store(chosen as u64, Ordering::Relaxed);
        PINNED_CPU.store(chosen as u64, Ordering::Relaxed); // rtpace
        if core_mode() == Core::Pin {
            log::info!("smooth: the game's threads run on processors 0x{:X}", cpu_sets::core_mask(chosen).unwrap_or(3usize << (chosen & !1)));
        }
        steer(shared.driver_handle.load(Ordering::Relaxed) as isize, shared.pinned.load(Ordering::Relaxed) as u32, true);
    }
    shared.deadline.store(deadline, Ordering::Relaxed);
    shared.turn.store(DRIVER, Ordering::Release);

    let waited = shared.latency.load(Ordering::Relaxed);
    let next = drive(system, deadline, DRIVER, false);
    if next != DRIVER {
        hand(system, next);
        wait_turn(&shared, DRIVER, None);
        if let Some(payload) = shared.panic.lock().unwrap().take() {
            std::panic::resume_unwind(payload);
        }
    }
    // a pinned core other work keeps busy: hand-offs wait for it
    if core_mode() == Core::Pin && !UNPINNED.load(Ordering::Relaxed) {
        let waited = shared.latency.load(Ordering::Relaxed) - waited;
        let frame = system.frames;
        let threads = &mut system.host_threads;
        // smooth: or the driver woke late
        if waited > SLOW_FRAME || LATE.swap(false, Ordering::Relaxed) {
            threads.slow.push_back(frame);
        }
        while threads.slow.front().is_some_and(|&at| at + SLOW_WINDOW < frame) {
            threads.slow.pop_front();
        }
        if threads.slow.len() >= SLOW_FRAMES {
            log::warn!(
                "os2: the core the game's threads are pinned to is busy (hand-offs waited {:.1} ms in frame {frame}), they run on any core from now on",
                waited as f64 / 1e6
            );
            let driver = shared.driver_handle.load(Ordering::Relaxed) as isize;
            unpin(threads.hosts.values().map(|host| host.handle).chain([driver]));
        }
    }
    if system.exited {
        StepOutcome::Exited
    } else if system.broke {
        StepOutcome::Faulted
    } else {
        StepOutcome::Ran
    }
}
