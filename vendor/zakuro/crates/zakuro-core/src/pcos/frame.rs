//! native-final: what happens between two frames, owned by Haiku.
//!
//! On the console the LCDs refresh at 59.83 Hz. At each refresh:
//!
//! - the shared page's clock moves on (PTM updates it);
//! - the GSP latches the framebuffers the game queued in its shared memory
//!   (SetBufferSwap through the FrameBufferInfo table) and takes the GX
//!   ring it held back for the refresh;
//! - both LCDs' VBlank interrupts (PDC0 top, PDC1 bottom) reach the game's
//!   GPU interrupt receiver thread, which wakes everything waiting for the
//!   frame (nngxWaitVSync).
//!
//! This driver does all of that natively: the clock through `pcos::clock`,
//! the framebuffers, ring and interrupts through `pcos::gsp`, the wake-ups
//! through `pcos::sched`. None of it goes through Zakuro's kernel or its GSP
//! service. The refresh comes every `CYCLES_PER_FRAME` ticks of the game
//! clock, at the same point of the guest timeline as before, so replays are
//! unchanged.

use crate::System;

/// the work of one refresh, in the console's order.
pub fn end_frame(system: &mut System) {
    system.count_frame();
    // the clock the game reads moves on to now
    let tick = system.cpu.cycles;
    let boot_clock = system.boot_clock;
    super::clock::update_datetime(system.memory.phys.shared_page_mut(), boot_clock, tick);
    // the timers, without the ones closed since (they fire when the
    // scheduling pass reaches their tick)
    super::sched::gather_timers(&mut system.kernel);
    // the emulated rasterizer's deferred work, in development builds
    #[cfg(feature = "emulation")]
    system.renderer.end_frame();
    // the LCDs take the framebuffers queued for this refresh, and a GX ring
    // held back for it runs
    super::gsp::refresh(system);
    // high frame rates: the pictures in between for the frontend
    system.pace_frame();
    // both LCDs finish scanning out, in that order
    super::gsp::vblank(system);
    super::rt::periodic(system.frames); // native-rt: FATES_SCHED_STATS
    super::hang::watch(system); // savehang: the hang watchdog and thread dump
    // online: local wireless's traffic, beacons and timeouts (the emulated
    // build ticks it with its devices)
    #[cfg(not(feature = "emulation"))]
    super::uds::tick(system);
}
