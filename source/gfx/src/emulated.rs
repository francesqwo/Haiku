//! gfx-final: the emulated GPU in a native session.
//!
//! With the native renderer on, the game's code still builds its PICA
//! command lists and asks the GSP for fills, copies and display transfers.
//! Before this module the emulated GPU still walked every list (register
//! writes, shader uploads, and the draws the compacted state lists leave
//! in: scene setup, bloom quads, unit-icon captures), cleared its colour
//! and depth buffers, and ran the display transfers of pictures the native
//! path did not draw.
//!
//! Two things live here:
//!
//! - **The audit.** zakuro-core counts what the emulated GPU did
//!   (`native_gfx::emulated_work`: lists, the draws in them, transfers,
//!   copies, fills, and what was left out). This module prints it with the
//!   gfx report, traces the gfx frames that still gave the emulated GPU
//!   work (`FATES_GFX_EMU_TRACE=N`, the first N), and counts the native
//!   texture reads that brought down something the emulated GPU had drawn
//!   (`deps`): a native picture that depends on emulated drawing.
//! - **The lease.** While every picture is drawn natively in full and
//!   nothing the native path reads is left to the emulated GPU, the native
//!   renderer leases the GSP (`native_gfx::lease_native`): command lists,
//!   fills and copies are then left out. Anything that needs the emulated
//!   GPU (an emulated or hybrid frame, a picture the native path has no
//!   frame for) withdraws the lease at once, before the frame's commands
//!   run (they run at nngxRunCmdlist, after both displays were recorded),
//!   and the lease comes back only after two gfx frames without such a
//!   need. A lease lasts four VBlanks, so a game that stops drawing through
//!   the hooks the renderer sees gets the emulated GPU back by itself.
//!   `FATES_GFX_EMULATED=keep` turns the lease off (the audit still
//!   counts).
//!
//! A frame the emulated GPU draws right after a lease starts from the
//! emulated GPU's state as the last list it walked left it (registers,
//! shaders, its colour buffer). The game sets the state a draw needs per
//! prim state, per material and per callback, and hybrid frames start from
//! a native seed, so that has not shown; it is the one way leasing could
//! change a picture, and it can only happen in a frame that was going to
//! be emulated anyway.

use std::collections::BTreeMap;

use zakuro_core::native_gfx::{self, EmulatedWork};

/// FATES_GFX_EMULATED=keep: the emulated GPU always runs.
pub fn lease_enabled() -> bool {
    // native-final: nothing to keep in the pure build
    if !cfg!(feature = "emulation") {
        return true;
    }
    !crate::perf::var("FATES_GFX_EMULATED").is_ok_and(|v| v == "keep")
}

#[derive(Default)]
pub struct Audit {
    /// this gfx frame needs the emulated GPU; gfx frames since one did
    need: bool,
    clean: u32,
    /// why it was needed, how many gfx frames
    pub reasons: BTreeMap<&'static str, u64>,
    /// gfx frames with the lease given out at least once
    pub leased: u64,
    leased_now: bool,
    /// the counters at the start of this gfx frame
    last: EmulatedWork,
    traced: u64,
    /// the first frame went by
    started: bool,
    /// native texture reads that brought down emulated drawing, and the
    /// first few of them
    pub deps: u64,
    deps_traced: u32,
}

impl Audit {
    /// the audit of a renderer that starts with the game: the boot lease
    /// (native_gfx::lease_boot) goes on into its first frames
    pub fn new() -> Audit {
        Audit { clean: 2, ..Audit::default() }
    }

    /// something in this gfx frame needs the emulated GPU
    pub fn need(&mut self, why: &'static str) {
        if !self.need {
            *self.reasons.entry(why).or_default() += 1;
        }
        self.need = true;
        native_gfx::want_emulated();
    }

    /// a display's frame was drawn natively in full and handed to its
    /// transfer: the lease, unless something needed the emulated GPU in
    /// this gfx frame or the one before
    pub fn native_transfer(&mut self) {
        if lease_enabled() && !self.need && self.clean >= 2 {
            native_gfx::lease_native();
            self.leased_now = true;
        }
    }

    /// a native texture read brought down `writes` writes the emulated GPU
    /// had drawn at `addr`
    pub fn dependency(&mut self, frame: u64, addr: u32, len: u32, what: &str, writes: u64) {
        if writes == 0 {
            return;
        }
        self.deps += 1;
        if crate::perf::var_os("FATES_GFX_EMU_TRACE").is_some() && self.deps_traced < 64 {
            self.deps_traced += 1;
            eprintln!("gfx-emulated dep frame {frame}: {what} {addr:08X} ({len:X} bytes) came down from the emulated GPU ({writes} writes)");
        }
    }

    /// a new gfx frame: what the emulated GPU did in the last one, traced
    pub fn frame(&mut self, frame: u64, native: bool) {
        let now = native_gfx::emulated_work();
        let limit = crate::perf::var("FATES_GFX_EMU_TRACE").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let d = |a: u64, b: u64| a.saturating_sub(b);
        let worked = now.lists != self.last.lists || now.transfers != self.last.transfers || now.fills != self.last.fills || now.copies != self.last.copies;
        if worked && self.traced < limit {
            self.traced += 1;
            eprintln!(
                "gfx-emulated frame {}: lists {} draws {} transfers {} copies {} fills {}; needed {:?}",
                frame.saturating_sub(1),
                d(now.lists, self.last.lists),
                d(now.draws, self.last.draws),
                d(now.transfers, self.last.transfers),
                d(now.copies, self.last.copies),
                d(now.fills, self.last.fills),
                self.need,
            );
        }
        self.last = now;
        self.leased += self.leased_now as u64;
        self.leased_now = false;
        self.clean = if self.need { 0 } else { self.clean.saturating_add(1) };
        // the first frame: the boot lease becomes an ordinary one
        if !self.started {
            self.started = true;
            if native && !self.need && lease_enabled() {
                native_gfx::lease_native();
            }
        }
        self.need = false;
    }

    pub fn report(&self) -> String {
        format!("gfx-emulated: {:?}; gfx frames leased {}, needed {:?}; native reads of emulated drawing {}", native_gfx::emulated_work(), self.leased, self.reasons, self.deps)
    }
}
