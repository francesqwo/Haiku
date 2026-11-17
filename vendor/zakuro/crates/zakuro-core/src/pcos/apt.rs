//! os2: the start-up requests that still went to emulated services: the
//! applet (APT) lifecycle and srv's notifications.
//!
//! - nn::applet::CTR::detail::APPLET::Initialize / GetLockHandle / Enable /
//!   ReceiveParameter / GlanceParameter: the application registers with the
//!   applet manager, gets its notification and parameter events and the
//!   transition lock, and receives the Wakeup parameter it starts with. Here
//!   the events and the lock are made directly, the Wakeup parameter is
//!   queued and its event signalled natively, and the parameter is handed
//!   over from the same state the APT service kept (services.apt), so a
//!   library applet started later still finds it.
//! - nn::srv::detail::Service::EnableNotification: the semaphore srv's
//!   notification thread waits on (no notification ever comes on PC).
//!
//! Each answers what the emulated service answered.

use zakuro_cpu::Bus;

use super::bridge::Call;
use super::stats;
use crate::kernel::object::KObject;
use crate::kernel::sync::{self, ResetType};
use crate::services::apt::{Parameter, SIGNAL_WAKEUP};
use crate::System;

/// call numbers, the same as in overrides/os2_native.c.
pub mod op {
    pub const APT_INITIALIZE: u32 = 0x620;
    pub const APT_GET_LOCK_HANDLE: u32 = 0x621;
    pub const APT_ENABLE: u32 = 0x622;
    pub const APT_RECEIVE_PARAMETER: u32 = 0x623;
    pub const APT_GLANCE_PARAMETER: u32 = 0x624;
    pub const SRV_ENABLE_NOTIFICATION: u32 = 0x630;
}

/// "no parameter is waiting".
const NO_PARAMETER: u32 = 0xC8A0_CFFC;

pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    let name = match number {
        // Initialize(app id, attributes, Handle* notification, Handle*
        // parameter)
        op::APT_INITIALIZE => {
            let (app, notification_out, parameter_out) = (call.arg(system, 0), call.arg(system, 2), call.arg(system, 3));
            let (_, notification) = system.kernel.create_event(ResetType::OneShot, "APT:notification");
            let (parameter_object, parameter) = system.kernel.create_event(ResetType::OneShot, "APT:parameter");
            let apt = &mut system.services.apt;
            apt.notification_event = Some(notification);
            apt.parameter_event = Some(parameter);
            apt.initialized = true;
            // the application starts by being handed a Wakeup parameter
            apt.parameter = Some(Parameter { sender: 0, destination: app, signal: SIGNAL_WAKEUP, buffer: Vec::new(), object: None });
            super::sched::signal_event(system, parameter_object);
            system.memory.write32(notification_out, notification);
            system.memory.write32(parameter_out, parameter);
            call.ret(0);
            "apt.Initialize"
        }
        // GetLockHandle(Handle* lock, attributes, u32* attributes out, u32*
        // power button state)
        op::APT_GET_LOCK_HANDLE => {
            let (lock_out, attributes, attributes_out, power_out) =
                (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2), call.arg(system, 3));
            let lock = match system.services.apt.lock {
                Some(handle) => handle,
                None => {
                    let kernel = &mut system.kernel;
                    let object = kernel.objects.insert(KObject::Mutex(sync::Mutex::new("APT:lock")));
                    let handle = kernel.handles.create(&mut kernel.objects, object, "APT:lock");
                    system.services.apt.lock = Some(handle);
                    handle
                }
            };
            system.memory.write32(attributes_out, attributes);
            system.memory.write32(power_out, 0);
            system.memory.write32(lock_out, lock);
            call.ret(0);
            "apt.GetLockHandle"
        }
        op::APT_ENABLE => {
            call.ret(0);
            "apt.Enable"
        }
        // Receive/GlanceParameter(u32* sender, app id, u32* signal, u8*
        // buffer, buffer size, int* size, Handle* handle)
        op::APT_RECEIVE_PARAMETER | op::APT_GLANCE_PARAMETER => {
            let glance = number == op::APT_GLANCE_PARAMETER;
            let parameter = if glance { system.services.apt.parameter.clone() } else { system.services.apt.parameter.take() };
            match parameter {
                Some(parameter) => {
                    let (sender_out, signal_out, data, capacity) =
                        (call.arg(system, 0), call.arg(system, 2), call.arg(system, 3), call.arg(system, 4));
                    let (size_out, handle_out) = (call.arg(system, 5), call.arg(system, 6));
                    let size = (parameter.buffer.len() as u32).min(capacity);
                    // the receiver gets its own handle to what came along
                    let handle = parameter.object.map_or(0, |object| {
                        let kernel = &mut system.kernel;
                        kernel.handles.create(&mut kernel.objects, object, "APT:parameter object")
                    });
                    if size > 0 && data != 0 {
                        system.memory.write_bytes(data, &parameter.buffer[..size as usize]);
                    }
                    system.memory.write32(sender_out, parameter.sender);
                    system.memory.write32(signal_out, parameter.signal);
                    system.memory.write32(size_out, size);
                    system.memory.write32(handle_out, handle);
                    call.ret(0);
                }
                None => call.ret(NO_PARAMETER),
            }
            if glance { "apt.GlanceParameter" } else { "apt.ReceiveParameter" }
        }
        // EnableNotification(Handle* semaphore)
        op::SRV_ENABLE_NOTIFICATION => {
            let out = call.arg(system, 0);
            let handle = match system.services.srv.notification_semaphore {
                Some(handle) => handle,
                None => {
                    let kernel = &mut system.kernel;
                    let semaphore = sync::Semaphore { name: "srv:notification".into(), count: 0, max_count: 16 };
                    let object = kernel.objects.insert(KObject::Semaphore(semaphore));
                    let handle = kernel.handles.create(&mut kernel.objects, object, "srv:notification");
                    system.services.srv.notification_semaphore = Some(handle);
                    handle
                }
            };
            system.memory.write32(out, handle);
            call.ret(0);
            "srv.EnableNotification"
        }
        _ => return false,
    };
    stats::native(name);
    true
}
