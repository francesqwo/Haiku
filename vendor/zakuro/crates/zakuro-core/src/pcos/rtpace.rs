//! rtpace: dev probes for real-time pacing, written to the timing log
//! (FATES_SMOOTH_LOG) in real time only. Deterministic runs never read or
//! write anything here.
//!
//! - `gtick`: a game tick started. The frontend names the word the game
//!   counts its ticks in (GameTime.ticks, `set_probe`); at every hand-off
//!   between host threads, a change of it is logged at the start of the run
//!   that changed it (a = the tick count, b = which guest thread, by id).
//! - `lend`: each lend of the frontend's time to the game's threads
//!   (a = 1 for a lend that ends once they are done, b = its overrun, ms).

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::Instant;

use crate::System;

/// the game's tick counter's address, 0 for none.
static PROBE: AtomicU32 = AtomicU32::new(0);
/// the tick count last seen.
static LAST: AtomicU32 = AtomicU32::new(u32::MAX);

/// names the word the game counts its ticks in.
pub fn set_probe(address: u32) {
    PROBE.store(address, Relaxed);
}

/// whether the probes log: real time with a timing log.
#[inline]
pub fn wanted() -> bool {
    super::rt::realtime() && PROBE.load(Relaxed) != 0 && zakuro_gpu::smooth::enabled()
}

/// at a hand-off: the run that ends now started at `run_start`, by guest
/// thread `guest` (None for the frontend).
pub(crate) fn handoff(system: &mut System, run_start: Instant, guest: Option<u32>) {
    let address = PROBE.load(Relaxed);
    let mut raw = [0u8; 4];
    system.memory.read_bytes(address, &mut raw);
    let ticks = u32::from_le_bytes(raw);
    if LAST.swap(ticks, Relaxed) != ticks {
        zakuro_gpu::smooth::point("gtick", run_start, ticks as i64, guest.map_or(-1.0, |g| g as f64));
    }
}

/// a lend of the frontend's time, from start to now.
pub(crate) fn lend(start: Instant, until: Instant, settled: bool) {
    if !wanted() {
        return;
    }
    let now = Instant::now();
    zakuro_gpu::smooth::span("lend", start, now, settled as i64, now.saturating_duration_since(until).as_secs_f64() * 1e3);
}
