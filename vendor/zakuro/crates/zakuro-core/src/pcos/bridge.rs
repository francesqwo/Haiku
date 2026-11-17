//! os: the bridge from game overrides (overrides/os_*.c) into the native
//! platform layer.
//!
//! An override replaces an SDK function that would make an SVC or an IPC
//! request. It calls `os_native_hook(ctx, op)`, a C function pointer the
//! executable sets at startup to [`os_native`]. The native side reads the
//! arguments where the ARM calling convention puts them (r0-r3, then the
//! guest stack), does the work on the host, writes outputs to guest memory
//! and the result to r0, and returns 1. Returning 0 hands the call back to
//! the override, which then runs the original SDK code (the IPC path), so a
//! native implementation can decline anything it doesn't cover.
//!
//! Calls run synchronously on the guest thread that made them, inside the
//! recompiled code's run, on the same System the code runs on; nothing here
//! touches the guest scheduler.

use recomp_abi::Context;
use zakuro_cpu::Bus;

use crate::System;

/// the call numbers the overrides use; os_native_abi in the C side has to
/// match ABI.
pub mod op {
    pub const ABI: u32 = 1;

    pub const FS_INITIALIZE: u32 = 0x100;
    pub const FS_OPEN_FILE: u32 = 0x101;
    pub const FS_OPEN_FILE_DIRECTLY: u32 = 0x102;
    pub const FS_OPEN_ARCHIVE: u32 = 0x103;
    pub const FS_CLOSE_ARCHIVE: u32 = 0x104;
    pub const FS_CONTROL_ARCHIVE: u32 = 0x105;
    pub const FS_FORMAT_SAVE_DATA: u32 = 0x106;
    pub const FS_IS_SDMC_DETECTED: u32 = 0x107;
    pub const FS_IS_SDMC_WRITABLE: u32 = 0x108;
    pub const FS_GET_PRIORITY: u32 = 0x109;
    pub const FS_SET_PRIORITY: u32 = 0x10A;
    pub const FS_CREATE_FILE: u32 = 0x10B;
    pub const FS_DELETE_FILE: u32 = 0x10C;
    pub const FS_RENAME_FILE: u32 = 0x10D;
    pub const FS_OPEN_DIRECTORY: u32 = 0x10E;
    pub const FS_CREATE_EXT_SAVE_DATA: u32 = 0x10F;
    pub const FS_DELETE_EXT_SAVE_DATA: u32 = 0x110;
    pub const FS_GET_ARCHIVE_RESOURCE: u32 = 0x111;

    pub const FILE_SET_PRIORITY: u32 = 0x120;
    pub const FILE_OPEN_LINK: u32 = 0x121;
    pub const FILE_READ: u32 = 0x122;
    pub const FILE_CLOSE: u32 = 0x123;
    pub const FILE_WRITE: u32 = 0x124;
    pub const FILE_GET_SIZE: u32 = 0x125;
    pub const FILE_SET_SIZE: u32 = 0x126;

    pub const DIRECTORY_READ: u32 = 0x130;
    pub const DIRECTORY_CLOSE: u32 = 0x131;

    /// system services, overrides/os_sys.c
    pub const SYS_FIRST: u32 = 0x200;
    pub const SYS_LAST: u32 = 0x2FF;
}

/// the arguments of the call an override made, as the ARM procedure call
/// standard places them, and its result.
pub struct Call<'a> {
    pub ctx: &'a mut Context,
    /// how long, in ticks, the call keeps the thread waiting, as the IPC
    /// request it replaces did on the console (see console_timing); None for
    /// a call that replaces no request.
    pub wait: Option<u64>,
}

impl Call<'_> {
    /// the nth 32-bit argument word: r0-r3, then the stack.
    pub fn arg(&self, system: &mut System, n: usize) -> u32 {
        if n < 4 {
            self.ctx.r[n]
        } else {
            system.memory.read32(self.ctx.r[13].wrapping_add(4 * (n as u32 - 4)))
        }
    }

    /// a 64-bit argument in words n and n+1 (n is even, as the ABI aligns).
    pub fn arg64(&self, system: &mut System, n: usize) -> u64 {
        self.arg(system, n) as u64 | (self.arg(system, n + 1) as u64) << 32
    }

    /// the result, in r0.
    pub fn ret(&mut self, value: u32) {
        self.ctx.r[0] = value;
    }
}

/// whether the native layer is on: FATES_NATIVE_OS=0 turns every native
/// call back into the original SDK code, for comparisons.
pub fn enabled() -> bool {
    // native-final: the pure build has no SDK-to-service fallback
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    return from_env();
}

#[cfg(feature = "emulation")]
fn from_env() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("FATES_NATIVE_OS").map_or(true, |value| value != "0");
        log::info!("os: the native OS layer is {}", if on { "on" } else { "off (FATES_NATIVE_OS=0)" });
        on
    })
}

/// whether native calls keep console timing: each waits as long as the
/// IPC request it replaces did (39 us, a file read as long as the game card
/// takes). On by default, so the native layer changes no game timing;
/// FATES_NATIVE_TIMING=fast makes them return at once.
pub fn console_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_NATIVE_TIMING").map_or(true, |value| value != "fast"))
}

/// native-rt: whether this session's native calls keep console timing:
/// in real time only with FATES_NATIVE_TIMING=console, see pcos::rt.
fn timed() -> bool {
    console_timing() && (!super::rt::realtime() || super::rt::console_timing_in_realtime())
}

/// the hook the overrides call, see the module docs.
///
/// # Safety
///
/// ctx has to be the context of a run of recompiled code
/// (recompiled::Library::run), which the overrides are only ever given.
pub unsafe extern "C" fn os_native(ctx: *mut Context, op: u32) -> i32 {
    // audio: the native sound layer, with its own switch (FATES_NATIVE_AUDIO);
    // only its replacements of IPC requests keep their console wait
    if (super::snd::op::FIRST..=super::snd::op::LAST).contains(&op) {
        // SAFETY: as below
        let system = unsafe { &mut *crate::recompiled::system_of(ctx) };
        let mut call = Call { ctx: unsafe { &mut *ctx }, wait: None };
        let handled = super::snd::call(system, &mut call, op);
        if handled && timed() { // native-rt: timed()
            if let Some(wait) = call.wait {
                // SAFETY: as below
                let tick = unsafe { crate::recompiled::ticks_of(ctx) };
                super::sched::sleep_current(system, wait, tick); // os2
                unsafe { crate::recompiled::leave_if_rescheduling(ctx) };
            }
        }
        return handled as i32;
    }
    if !enabled() {
        return 0;
    }
    // SAFETY: as above, the system is the one the code runs in
    let system = unsafe { &mut *crate::recompiled::system_of(ctx) };
    let mut call = Call { ctx: unsafe { &mut *ctx }, wait: Some(crate::pcos::svc::REQUEST_TICKS) };
    let handled = match op {
        0x100..=0x1FF => super::fs::call(system, &mut call, op),
        op::SYS_FIRST..=op::SYS_LAST => super::sys::call(system, &mut call, op),
        0x300..=0x3FF => super::y2r::call(system, &mut call, op),
        0x400..=0x4FF => super::sync::call(system, &mut call, op),
        // os2: GPU service calls, then APT/srv/cec/ndm start-up
        0x600..=0x61F => super::gsp::call(system, &mut call, op),
        0x620..=0x6FF => super::apt::call(system, &mut call, op),
        // offline-msg: an online feature that failed, see online.rs
        super::online::op::FIRST..=super::online::op::LAST => super::online::call(system, &mut call, op),
        // online: the game's Local Battle, see uds.rs
        super::uds::op::FIRST..=super::uds::op::LAST => super::uds::call(system, &mut call, op),
        _ => false,
    };
    // console timing: the thread waits as long as the request it replaces
    // kept it waiting, so the game's threads interleave as they did over
    // IPC and runs and replays stay the same
    if handled && timed() { // native-rt: timed()
        if let Some(wait) = call.wait {
            // SAFETY: as above
            let tick = unsafe { crate::recompiled::ticks_of(ctx) };
            super::sched::sleep_current(system, wait, tick); // os2
        }
    }
    // a call that woke a thread, or put this one to sleep, lets the
    // scheduler run the next, as an svc would
    if handled {
        // SAFETY: as above
        unsafe { crate::recompiled::leave_if_rescheduling(ctx) };
    }
    handled as i32
}

/// SVCs made by recompiled code that the native layer answers in place of
/// the kernel, the way an override answers an SDK call: the system tick
/// (svc 0x28 is inlined into some 50 places of the game and the SDK, which
/// no override can reach), the processor number and cache maintenance.
/// true when it answered.
///
/// # Safety
///
/// as for os_native.
pub unsafe fn native_svc(ctx: *mut Context, number: u32) -> bool {
    if !matches!(number, 0x0B | 0x11 | 0x19 | 0x22 | 0x28 | 0x54..=0x56) || !enabled() {
        return false;
    }
    // the synchronisation fast paths, see sync.rs; FATES_NATIVE_SYNC=0
    // leaves them to the kernel
    if matches!(number, 0x0B | 0x19 | 0x22) {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("FATES_NATIVE_SYNC").map_or(true, |value| value != "0")) {
            return false;
        }
        // SAFETY: as above
        let system = unsafe { &mut *crate::recompiled::system_of(ctx) };
        let ctx = unsafe { &mut *ctx };
        return match super::sync::svc(system, ctx, number) {
            Some(name) => {
                system.idle_interrupted(true);
                super::stats::native(name);
                true
            }
            None => false,
        };
    }
    // SAFETY: as above
    let ctx = unsafe { &mut *ctx };
    let name = match number {
        0x28 => {
            // SAFETY: as above
            let tick = unsafe { crate::recompiled::ticks_of(ctx) };
            ctx.r[0] = tick as u32;
            ctx.r[1] = (tick >> 32) as u32;
            ctx.budget -= crate::pcos::svc::SYSTEM_TICK_COST as i32;
            "svc.GetSystemTick"
        }
        0x11 => {
            ctx.r[0] = 0;
            "svc.GetCurrentProcessorNumber"
        }
        // the host's caches are coherent
        _ => {
            ctx.r[0] = 0;
            "svc.CacheMaintenance"
        }
    };
    super::stats::native(name);
    true
}

/// the hook for the executable to put in os_native_hook, when the
/// overrides linked in speak this ABI.
pub fn hook(abi: u32) -> Option<unsafe extern "C" fn(*mut Context, u32) -> i32> {
    if abi != op::ABI {
        log::warn!("os: the overrides speak native call ABI {abi}, this build {}, the native OS layer is off", op::ABI);
        return None;
    }
    Some(os_native)
}
