//! native-final: the one entry point for the game's supervisor calls.
//!
//! Recompiled code that reaches an `svc` (from a step, or in place inside
//! the code) lands here. Native code answers every call the game makes:
//!
//! - threads and synchronisation: `pcos::sched` (os2);
//! - memory, process information, the clock and debug output:
//!   `pcos::process` (native-final), which owns the 52 start-up calls that
//!   used to reach Zakuro's kernel.
//!
//! Anything else is a call the game was never seen to make. In a development
//! build (`--no-default-features`, without `pure`) it still falls back to
//! Zakuro's kernel dispatcher, counted in `pcos::emu`. In the shipped `pure`
//! build there is no kernel dispatcher to fall back to: the call is logged,
//! counted and answered with "not implemented", which is what the console
//! answers for an SVC a process may not use.

use super::emu;
use crate::System;

/// how long a request to a service keeps the thread waiting, what Citra
/// measured for one to GSP on a New 3DS (the console timing every native
/// replacement of a request keeps).
pub(crate) const REQUEST_TICKS: u64 = crate::kernel::thread::nanos_to_ticks(39_000);

/// ticks between two svcGetSystemTick calls in a row on the console.
pub(crate) const SYSTEM_TICK_COST: u64 = 150;

/// whether recompiled code can have an svc run in place, from inside the
/// code. the ones that map memory, start or end threads or the process,
/// reach services, which load code, or that nothing implements stop the
/// code, and the step runs them. (The same set as before: which calls stop
/// the code decides where budget blocks are cut, which replays depend on.)
pub fn in_place(number: u32) -> bool {
    matches!(number, 0x02 | 0x0A..=0x0C | 0x11 | 0x13..=0x1D | 0x21..=0x25 | 0x27 | 0x28 | 0x2A | 0x2B | 0x35 | 0x37..=0x3A | 0x3D | 0x54..=0x56)
}

/// runs one supervisor call natively.
pub fn dispatch(system: &mut System, number: u32) {
    if super::sched::svc(system, number) {
        return;
    }
    if super::process::svc(system, number) {
        return;
    }
    // gate: a request to an online service, answered offline natively, or a
    // library-applet request to APT
    if number == 0x32 && (super::net::request(system) || super::applet::request(system) || super::local::request(system)) {
        return;
    }
    unanswered(system, number);
}

/// a call no native layer answers.
#[cfg(feature = "emulation")]
fn unanswered(system: &mut System, number: u32) {
    // development builds: Zakuro's kernel, counted
    emu::count(emu::Reached::KernelSvc);
    crate::kernel::svc::dispatch(system, number);
}

/// a call no native layer answers: there is no emulated kernel in the pure
/// build. SendSyncRequest (0x32), an IPC request to a service, is the one
/// the game could still make on a path no replay has reached; it gets a
/// well-formed error reply so the SDK's error handling sees a failed
/// request rather than garbage.
#[cfg(not(feature = "emulation"))]
fn unanswered(system: &mut System, number: u32) {
    use zakuro_common::result::errors;
    use zakuro_cpu::Bus;
    emu::count(emu::Reached::Unanswered);
    let pc = system.cpu.current_pc();
    if number == 0x32 {
        let handle = system.cpu.regs[0];
        let name = system.kernel.handles.label(handle).to_owned();
        if let Some(thread) = system.kernel.current() {
            let buffer = thread.tls + 0x80;
            let header = system.memory.read32(buffer);
            log::error!("native-final: unanswered IPC request 0x{:04X} to \"{name}\" at 0x{pc:08X}: no native implementation, answered with an error", header >> 16);
            // header: same command id, one normal word (the result)
            system.memory.write32(buffer, (header & 0xFFFF_0000) | 0x40);
            system.memory.write32(buffer + 4, errors::UNIMPLEMENTED.0);
        }
        system.cpu.regs[0] = 0;
        return;
    }
    log::error!("native-final: unanswered svc 0x{number:02X} ({}) at 0x{pc:08X}", super::stats::svc_name(number));
    system.unimplemented_svcs.insert(number);
    system.cpu.regs[0] = errors::UNIMPLEMENTED.0;
}
