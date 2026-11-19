//! vow: ir:rst (the New 3DS's ZL, ZR and C-stick), natively.
//!
//! Fates itself never opens ir:rst, but code mods built with libctru-style
//! code (V.O.W Edition) do, at boot: GetHandles, then Initialize. The pure
//! build had no answer, so the mod's request failed. They are answered as
//! Zakuro's services/ir_rst.rs answers them, with the same block and event,
//! which services::ir_rst::tick fills from the frontend's input.

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::System;

/// answers svcSendSyncRequest (r0 = the session) to ir:rst; false for any
/// other service or a request not covered.
pub fn request(system: &mut System) -> bool {
    let handle = system.cpu.regs[0];
    if system.kernel.handles.label(handle) != "ir:rst" {
        return false;
    }
    let Some(tls) = system.kernel.current().map(|thread| thread.tls) else {
        return false;
    };
    let buffer = CommandBuffer::new(tls);
    let command = buffer.header(&mut system.memory).command_id();
    let name = match command {
        // GetHandles -> the block and the update event
        0x0001 => {
            let (memory, event) = crate::services::ir_rst::resources(system);
            let kernel = &mut system.kernel;
            let memory = kernel.handles.create(&mut kernel.objects, memory, "ir:rst shared memory");
            let event = kernel.handles.create(&mut kernel.objects, event, "ir:rst update event");
            buffer.set(&mut system.memory, 0, Header::new(command, 1, 3).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, Descriptor::handles(2));
            buffer.set(&mut system.memory, 3, memory);
            buffer.set(&mut system.memory, 4, event);
            "ir:rst.GetHandles"
        }
        // Initialize(update period in ms, raw C-stick)
        0x0002 => {
            let milliseconds = buffer.get(&mut system.memory, 1);
            crate::services::ir_rst::initialize(system, milliseconds);
            buffer.reply(&mut system.memory, command, &[]);
            "ir:rst.Initialize"
        }
        0x0003 => {
            crate::services::ir_rst::shutdown(system);
            buffer.reply(&mut system.memory, command, &[]);
            "ir:rst.Shutdown"
        }
        // what the console reads of the C-stick's own chip, nothing
        // titles act on
        0x0004..=0x0009 => {
            buffer.reply(&mut system.memory, command, &[0, 0]);
            "ir:rst.Other"
        }
        _ => return false,
    };
    super::stats::native(name);
    system.cpu.regs[0] = 0;
    if system.kernel.current().is_some_and(|thread| thread.status == crate::kernel::thread::ThreadStatus::Running) {
        let tick = system.cpu.cycles;
        super::sched::sleep_current(system, super::svc::REQUEST_TICKS, tick);
    }
    true
}
