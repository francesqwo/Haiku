//! os: native nn::y2r, the YUV-to-RGB converter movies are played through,
//! what overrides/os_y2r.c calls in place of the SDK's y2r:u IPC stubs
//! (nn::y2r::CTR::detail::Y2r::*).
//!
//! The settings go straight into the converter's state and StartConversion
//! converts on the host at once (services::y2r::convert, the hardware's
//! arithmetic bit for bit), then signals the transfer-end event, which the
//! movie player's thread waits on. The event stays a kernel event: threads
//! and their waits are still on the guest scheduler.

use zakuro_cpu::Bus;

use super::bridge::Call;
use super::stats;
use crate::services::y2r::{self as hw, Buffer, STANDARD};
use crate::System;

/// call numbers, the same as overrides/os_y2r.c
pub mod op {
    pub const SET_INPUT_FORMAT: u32 = 0x300;
    pub const SET_OUTPUT_FORMAT: u32 = 0x301;
    pub const SET_ROTATION: u32 = 0x302;
    pub const SET_BLOCK_ALIGNMENT: u32 = 0x303;
    pub const SET_TRANSFER_END_INTERRUPT: u32 = 0x304;
    pub const GET_TRANSFER_END_EVENT: u32 = 0x305;
    pub const SET_SENDING_Y: u32 = 0x306;
    pub const SET_SENDING_U: u32 = 0x307;
    pub const SET_SENDING_V: u32 = 0x308;
    pub const SET_RECEIVING: u32 = 0x309;
    pub const SET_INPUT_LINE_WIDTH: u32 = 0x30A;
    pub const SET_INPUT_LINES: u32 = 0x30B;
    pub const SET_STANDARD_COEFFICIENT: u32 = 0x30C;
    pub const SET_ALPHA: u32 = 0x30D;
    pub const START_CONVERSION: u32 = 0x30E;
    pub const STOP_CONVERSION: u32 = 0x30F;
    pub const IS_BUSY_CONVERSION: u32 = 0x310;
    pub const DRIVER_INITIALIZE: u32 = 0x311;
    pub const DRIVER_FINALIZE: u32 = 0x312;
}

/// runs a native y2r call, false when it is not one.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    let a0 = call.arg(system, 0);
    let name = match number {
        op::SET_INPUT_FORMAT => {
            system.services.y2r.input_format = a0 & 0xFF;
            "y2r.SetInputFormat"
        }
        op::SET_OUTPUT_FORMAT => {
            system.services.y2r.output_format = a0 & 0xFF;
            "y2r.SetOutputFormat"
        }
        op::SET_ROTATION => {
            system.services.y2r.rotation = a0 & 0xFF;
            "y2r.SetRotation"
        }
        op::SET_BLOCK_ALIGNMENT => {
            system.services.y2r.tiled = a0 & 0xFF != 0;
            "y2r.SetBlockAlignment"
        }
        op::SET_TRANSFER_END_INTERRUPT => {
            system.services.y2r.end_interrupt = a0 & 0xFF != 0;
            "y2r.SetTransferEndInterrupt"
        }
        // GetTransferEndEvent(Handle* out): a handle to the end event
        op::GET_TRANSFER_END_EVENT => {
            let object = hw::event(system);
            let handle = system.kernel.handles.create(&mut system.kernel.objects, object, "y2r end event");
            system.memory.write32(a0, handle);
            "y2r.GetTransferEndEvent"
        }
        // SetSendingY/U/V and SetReceiving(Handle process, address, image
        // size, transfer unit, gap)
        op::SET_SENDING_Y..=op::SET_RECEIVING => {
            let target = Buffer {
                address: call.arg(system, 1),
                transfer: call.arg(system, 3) as u16,
                gap: call.arg(system, 4) as u16,
            };
            let y2r = &mut system.services.y2r;
            match number {
                op::SET_SENDING_Y => y2r.y = target,
                op::SET_SENDING_U => y2r.u = target,
                op::SET_SENDING_V => y2r.v = target,
                _ => y2r.output = target,
            }
            "y2r.SetBuffer"
        }
        op::SET_INPUT_LINE_WIDTH => {
            system.services.y2r.line_width = a0 & 0xFFFF;
            "y2r.SetInputLineWidth"
        }
        op::SET_INPUT_LINES => {
            system.services.y2r.lines = a0 & 0xFFFF;
            "y2r.SetInputLines"
        }
        op::SET_STANDARD_COEFFICIENT => {
            let index = a0 & 0xFF;
            system.services.y2r.standard = index;
            system.services.y2r.coefficients = STANDARD[index as usize % 4];
            "y2r.SetStandardCoefficient"
        }
        op::SET_ALPHA => {
            system.services.y2r.alpha = a0 & 0xFFFF;
            "y2r.SetAlpha"
        }
        // StartConversion: convert now, then signal the end
        op::START_CONVERSION => {
            hw::convert(system);
            let object = hw::event(system);
            super::sched::signal_event(system, object); // os2
            "y2r.StartConversion"
        }
        op::STOP_CONVERSION => "y2r.StopConversion",
        // IsBusyConversion(bool* out): a conversion is done when it starts
        op::IS_BUSY_CONVERSION => {
            system.memory.write8(a0, 0);
            "y2r.IsBusyConversion"
        }
        op::DRIVER_INITIALIZE | op::DRIVER_FINALIZE => {
            let event = system.services.y2r.event;
            system.services.y2r = hw::Y2rState::default();
            system.services.y2r.event = event;
            "y2r.Driver"
        }
        _ => return false,
    };
    call.ret(0);
    stats::native(name);
    true
}
