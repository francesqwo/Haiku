//! hid:USER, buttons, circle pad, touch screen and motion sensors: the IPC
//! requests. native-final: the shared memory and its per-frame update are
//! native (pcos::hid); this is the request handler development builds keep.

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::System;

pub use crate::pcos::hid::*;

#[cfg(feature = "emulation")] // native-final: the IPC request handler, development builds only
pub fn handle(system: &mut System, buffer: &CommandBuffer, header: Header) -> bool {
    match header.command_id() {
        // GetIPCHandles -> shared memory plus five event handles.
        0x000A => {
            ensure_resources(system);
            let handle = system.services.hid.shared_memory_handle.unwrap_or(0);
            let events = system.services.hid.events.clone();

            buffer.set(&mut system.memory, 0, Header::new(0x000A, 1, 7).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, Descriptor::handles(6));
            buffer.set(&mut system.memory, 3, handle);
            for (i, event) in events.iter().enumerate() {
                buffer.set(&mut system.memory, 4 + i as u32, *event);
            }
            true
        }
        // EnableAccelerometer / DisableAccelerometer / EnableGyroscopeLow /
        // DisableGyroscopeLow
        0x0011..=0x0014 => {
            let hid = &mut system.services.hid;
            match header.command_id() {
                0x0011 => hid.accelerometer_users += 1,
                0x0012 => hid.accelerometer_users = hid.accelerometer_users.saturating_sub(1),
                0x0013 => hid.gyroscope_users += 1,
                _ => hid.gyroscope_users = hid.gyroscope_users.saturating_sub(1),
            }
            buffer.reply(&mut system.memory, header.command_id(), &[]);
            true
        }
        // GetGyroscopeLowRawToDpsCoefficient
        0x0015 => {
            buffer.reply(&mut system.memory, 0x0015, &[(14.375f32).to_bits()]);
            true
        }
        // GetGyroscopeLowCalibrateParam
        0x0016 => {
            buffer.reply(&mut system.memory, 0x0016, &[0, 0, 0, 0, 0]);
            true
        }
        // GetSoundVolume
        0x0017 => {
            buffer.reply(&mut system.memory, 0x0017, &[0x3F]);
            true
        }
        _ => false,
    }
}

