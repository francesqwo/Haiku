//! gate: the applet manager's library-applet requests, natively.
//!
//! The online paths end in a library applet: when a connection fails the
//! SDK cancels any library applet still registered and starts the system's
//! error display (erreula) with the error code; friend and Mii pickers work
//! the same way. The start-up APT calls are native already (pcos::apt, via
//! overrides/os2_native.c), but these requests went over IPC to Zakuro's APT
//! service, so in the pure build they were unanswered: IsRegistered failed
//! and the SDK threw a fatal error, which stops the game.
//!
//! They are answered here as Zakuro's services/apt.rs answered them, over
//! the same state (system.services.apt): there is no applet to run on PC,
//! so a library applet closes as soon as it starts and hands its caller a
//! blank result the size of what it was given (what Citra's applets do),
//! and the game carries on with its own messages.

use zakuro_cpu::Bus;

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::kernel::object::{KObject, ObjectId, SharedMemory};
use crate::services::apt::Parameter;
use crate::System;

/// the running title's own applet id.
const APPLICATION: u32 = 0x300;
/// the id a title asks IsRegistered about for any library applet.
const ANY_LIBRARY_APPLET: u32 = 0x400;
/// signals a parameter can carry.
const SIGNAL_REQUEST: u32 = 2;
const SIGNAL_RESPONSE: u32 = 3;
const SIGNAL_WAKEUP_BY_EXIT: u32 = 10;

/// whether these answers are on: always in the pure build;
/// FATES_NATIVE_APPLET=0 gives development builds Zakuro's APT back.
fn enabled() -> bool {
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("FATES_NATIVE_APPLET").map_or(true, |v| v != "0"))
    }
}

/// answers svcSendSyncRequest (r0 = the session) to APT when it is one of
/// the requests below; false for anything else.
pub fn request(system: &mut System) -> bool {
    if !enabled() {
        return false;
    }
    let handle = system.cpu.regs[0];
    let gpu = match system.kernel.handles.label(handle) {
        "APT:U" | "APT:A" | "APT:S" => false,
        "gsp::Gpu" => true,
        _ => return false,
    };
    let Some(tls) = system.kernel.current().map(|thread| thread.tls) else {
        return false;
    };
    let buffer = CommandBuffer::new(tls);
    let command = buffer.header(&mut system.memory).command_id();
    let answered = if gpu { answer_gpu(system, &buffer, command) } else { answer(system, &buffer, tls, command) };
    let Some(name) = answered else {
        return false;
    };
    super::stats::native(name);
    system.cpu.regs[0] = 0;
    // the thread waits for the reply as for any service's
    if system.kernel.current().is_some_and(|thread| thread.status == crate::kernel::thread::ThreadStatus::Running) {
        let tick = system.cpu.cycles;
        super::sched::sleep_current(system, super::svc::REQUEST_TICKS, tick);
    }
    true
}

fn answer(system: &mut System, buffer: &CommandBuffer, tls: u32, command: u16) -> Option<&'static str> {
    Some(match command {
        // Finalize / the rest of Enable's family: acknowledged
        0x0003 | 0x0004 => {
            buffer.reply(&mut system.memory, command, &[]);
            "apt.acknowledged"
        }
        // GetAppletManInfo: position, requested id, the HOME Menu, the
        // title as the active applet
        0x0005 => {
            buffer.reply(&mut system.memory, command, &[0, 0, 0x101, APPLICATION]);
            "apt.GetAppletManInfo"
        }
        // GetAppletInfo(app id): the title's id, media type, registered,
        // loaded, attributes (a library applet the SDK polls for after
        // starting one counts as there and loaded)
        0x0006 => {
            let title = system.kernel.program_id;
            buffer.reply(&mut system.memory, command, &[title as u32, (title >> 32) as u32, 0, 1, 1, 0]);
            "apt.GetAppletInfo"
        }
        // IsRegistered(app id): any library applet is there only while the
        // title prepares or runs one
        0x0009 => {
            let applet = buffer.get(&mut system.memory, 1);
            let apt = &system.services.apt;
            let registered = applet != ANY_LIBRARY_APPLET || apt.library_applet.is_some() || apt.keyboard.is_some();
            buffer.reply(&mut system.memory, command, &[registered as u32]);
            "apt.IsRegistered"
        }
        // InquireNotification: none pending
        0x000B => {
            buffer.reply(&mut system.memory, command, &[0]);
            "apt.InquireNotification"
        }
        // SendParameter(sender, destination, signal, size, handle, buffer):
        // the applet being started answers its request with a block to
        // capture the screens into
        0x000C => {
            let destination = buffer.get(&mut system.memory, 2);
            let signal = buffer.get(&mut system.memory, 3);
            let size = buffer.get(&mut system.memory, 4);
            let data = read_static(system, buffer, 7, size);
            if system.services.apt.library_applet == Some(destination) && signal == SIGNAL_REQUEST {
                let capture_size = data.get(..4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                let block = capture_block(system, capture_size);
                send_parameter(system, Parameter { sender: destination, destination: APPLICATION, signal: SIGNAL_RESPONSE, buffer: Vec::new(), object: block });
            }
            buffer.reply(&mut system.memory, command, &[]);
            "apt.SendParameter"
        }
        // CancelParameter: done
        0x000F => {
            system.services.apt.parameter = None;
            buffer.reply(&mut system.memory, command, &[1]);
            "apt.CancelParameter"
        }
        // PreloadLibraryApplet / PrepareToStartLibraryApplet(applet id): the
        // applet counts as there from now on
        0x0016 | 0x0018 => {
            system.services.apt.library_applet = Some(buffer.get(&mut system.memory, 1));
            buffer.reply(&mut system.memory, command, &[]);
            "apt.PrepareToStartLibraryApplet"
        }
        // StartLibraryApplet(applet id, size, handle, buffer): it closes at
        // once with a blank result (the software keyboard and the Mii
        // selector have answers of their own, as in Zakuro)
        0x001E => {
            let applet = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let data = read_static(system, buffer, 5, size);
            if applet == crate::services::keyboard::APPLET_ID {
                let handle = buffer.get(&mut system.memory, 4);
                let memory = system.kernel.resolve(handle);
                system.services.apt.library_applet = None;
                crate::services::keyboard::start(system, data, memory);
                buffer.reply(&mut system.memory, command, &[]);
                return Some("apt.StartLibraryApplet");
            }
            let result = if crate::services::mii_selector::APPLET_IDS.contains(&applet) {
                log::info!("apt: the Mii selector picks the console's Mii");
                crate::services::mii_selector::result()
            } else {
                log::info!("apt: library applet 0x{applet:03X} started with {} bytes, closing it right away (none on PC)", data.len());
                vec![0; data.len()]
            };
            system.services.apt.library_applet = None;
            send_parameter(system, Parameter { sender: applet, destination: APPLICATION, signal: SIGNAL_WAKEUP_BY_EXIT, buffer: result, object: None });
            buffer.reply(&mut system.memory, command, &[]);
            "apt.StartLibraryApplet"
        }
        // the application-launching machinery and ReplySleepQuery /
        // ReplySleepNotificationComplete: nothing to do with one title and
        // no sleep
        0x0015 | 0x0017 | 0x0019 | 0x001B | 0x001F | 0x003E | 0x003F | 0x0043 | 0x0055 => {
            buffer.reply(&mut system.memory, command, &[]);
            "apt.acknowledged"
        }
        // SendCaptureBufferInfo(size, buffer)
        0x0040 => {
            let size = buffer.get(&mut system.memory, 1);
            system.services.apt.capture_info = read_static(system, buffer, 2, size);
            buffer.reply(&mut system.memory, command, &[]);
            "apt.SendCaptureBufferInfo"
        }
        // ReceiveCaptureBufferInfo(size): the info given back
        0x0041 => {
            let (pointer, capacity) = buffer.static_buffer(&mut system.memory, tls, 0);
            let info = system.services.apt.capture_info.clone();
            let size = (info.len() as u32).min(buffer.get(&mut system.memory, 1)).min(capacity);
            if size > 0 && pointer != 0 {
                system.memory.write_bytes(pointer, &info[..size as usize]);
            }
            buffer.set(&mut system.memory, 0, Header::new(command, 2, 2).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, size);
            buffer.set(&mut system.memory, 3, Descriptor::static_buffer(size, 0));
            buffer.set(&mut system.memory, 4, pointer);
            "apt.ReceiveCaptureBufferInfo"
        }
        // CheckNew3DSApp / CheckNew3DS, and the platform: 1 a 3DS, 2 a New 3DS
        0x0101 | 0x0102 => {
            let new3ds = system.config.new3ds as u32;
            buffer.reply(&mut system.memory, command, &[new3ds]);
            "apt.CheckNew3DS"
        }
        0x0103 => {
            let platform = if system.config.new3ds { 2 } else { 1 };
            buffer.reply(&mut system.memory, command, &[platform]);
            "apt.GetTargetPlatform"
        }
        // GetScreenCapPostPermission
        0x0056 => {
            buffer.reply(&mut system.memory, command, &[1]);
            "apt.GetScreenCapPostPermission"
        }
        _ => return None,
    })
}

/// the GPU's side of starting a library applet: the title saves its part
/// of VRAM, tells the applet where its framebuffers are and gets the GPU
/// back afterwards. The other GSP calls are native already (pcos::gsp, via
/// overrides/os2_native.c); these three only come with an applet.
fn answer_gpu(system: &mut System, buffer: &CommandBuffer, command: u16) -> Option<&'static str> {
    Some(match command {
        // ImportDisplayCaptureInfo: the framebuffers both screens show
        0x0018 => {
            let info = system.display_capture_info();
            buffer.set(&mut system.memory, 0, Header::new(command, 9, 0).0);
            buffer.set(&mut system.memory, 1, 0);
            for (i, value) in info.iter().enumerate() {
                buffer.set(&mut system.memory, i as u32 + 2, *value);
            }
            "gsp.ImportDisplayCaptureInfo"
        }
        // SaveVramSysArea / RestoreVramSysArea: nothing else uses the
        // title's VRAM on PC (Zakuro answered success too)
        0x0019 | 0x001A => {
            buffer.reply(&mut system.memory, command, &[]);
            "gsp.SaveRestoreVramSysArea"
        }
        _ => return None,
    })
}

/// queues a parameter for the title and signals the event it waits on.
fn send_parameter(system: &mut System, parameter: Parameter) {
    system.services.apt.parameter = Some(parameter);
    let event = system.services.apt.parameter_event.and_then(|handle| system.kernel.resolve(handle));
    if let Some(event) = event {
        super::sched::signal_event(system, event);
    }
}

/// the bytes a request carries in the static buffer whose descriptor sits at
/// word index, at most size of them.
fn read_static(system: &mut System, buffer: &CommandBuffer, index: u32, size: u32) -> Vec<u8> {
    let descriptor = buffer.get(&mut system.memory, index);
    let pointer = buffer.get(&mut system.memory, index + 1);
    let length = size.min((descriptor >> 14) & 0x3FFFF);
    let mut data = vec![0; length as usize];
    if pointer != 0 {
        system.memory.read_bytes(pointer, &mut data);
    }
    data
}

/// a block of at least size bytes a library applet hands over for the screen
/// capture, made once and kept while it is big enough.
fn capture_block(system: &mut System, size: u32) -> Option<ObjectId> {
    if let Some((object, capacity)) = system.services.apt.capture_block {
        if capacity >= size {
            return Some(object);
        }
    }
    let size = zakuro_common::bits::align_up(size.max(0x1000), 0x1000);
    let block = system.memory.phys.allocate(crate::memory::MemoryRegion::Base, size)?;
    let object = system.kernel.objects.insert(KObject::SharedMemory(SharedMemory {
        name: "APT:capture".into(),
        address: 0,
        size,
        paddr: block.addr,
        mapped_at: None,
    }));
    // APT keeps it: a title closing every handle it was given must not take
    // it away from the next applet
    system.kernel.objects.add_ref(object);
    system.services.apt.capture_block = Some((object, size));
    Some(object)
}
