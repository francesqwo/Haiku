//! native-final: always-on counters of every way a session can still reach
//! Zakuro's emulation code. A headless run prints them at the end
//! (`emulation:` line). In the shipped `pure` build most of these paths are
//! not compiled in at all; the counters then prove the remaining entry
//! points (the ones a pure build answers with an error instead) were never
//! taken.

use std::sync::atomic::{AtomicU64, Ordering};

/// what a session reached.
#[derive(Clone, Copy, Debug)]
pub enum Reached {
    /// an SVC answered by Zakuro's kernel dispatcher (development builds)
    KernelSvc,
    /// an IPC request handled by one of Zakuro's HLE services
    Service,
    /// a scheduling pass by Zakuro's guest scheduler (`Kernel::schedule`)
    Scheduler,
    /// an audio frame of the emulated DSP
    Dsp,
    /// a GX operation run by the emulated GPU (command list, transfer,
    /// copy, fill)
    Gpu,
    /// a GX operation the native renderer could not cover and the pure
    /// build had no emulated GPU for (a frame would have been emulated)
    GpuUnavailable,
    /// a step of the HLE device ticks (ir, cam, uds, dlp)
    DeviceTick,
    /// an SVC or IPC request nothing answers (pure build)
    Unanswered,
}

const KINDS: usize = 8;
static COUNTS: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

pub fn count(what: Reached) {
    let before = COUNTS[what as usize].fetch_add(1, Ordering::Relaxed);
    // native-rt: with FATES_SCHED_STATS, where the first ones came
    if super::rt::stats_wanted() && before < 8 {
        let (lease, vblank) = crate::native_gfx::lease_state();
        eprintln!("emulation: {what:?} reached (lease {lease}, vblank {vblank}): {}", std::backtrace::Backtrace::force_capture().to_string().lines().filter(|line| line.contains("zakuro") || line.contains("haiku") || line.contains("fates")).take(12).collect::<Vec<_>>().join(" | "));
    }
}

pub fn get(what: Reached) -> u64 {
    COUNTS[what as usize].load(Ordering::Relaxed)
}

/// whether this build has any emulation fallback compiled in.
pub const PURE: bool = !cfg!(feature = "emulation");

/// the summary line a headless run prints; `interpreted` is this run's.
pub fn summary(interpreted: u64) -> String {
    use Reached::*;
    let total: u64 = COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum::<u64>() + interpreted;
    format!(
        "emulation: {} build; kernel svcs {}, service requests {}, guest scheduler passes {}, emulated DSP frames {}, emulated GPU ops {} (unavailable {}), device ticks {}, unanswered {}, interpreted instructions {}: {}",
        if PURE { "pure" } else { "development (fallbacks linked)" },
        get(KernelSvc),
        get(Service),
        get(Scheduler),
        get(Dsp),
        get(Gpu),
        get(GpuUnavailable),
        get(DeviceTick),
        get(Unanswered),
        interpreted,
        if total == 0 { "nothing emulated ran" } else { "EMULATION RAN" }
    )
}
