//! gate: StreetPass (cecd:u) and the amiibo reader (nfc:u), natively.
//!
//! My Castle's crystal ball reaches both: Settings/Info > StreetPass asks
//! the StreetPass daemon for its state, and amiibo starts the NFC reader.
//! The pure build had no answer for either request. They are answered here
//! as Zakuro's services/cecd.rs and services/nfc.rs answered them: the
//! daemon is there and does what it is told but never meets another
//! console, and the reader is there and scans but no tag ever comes into
//! range. The game then shows its own messages.

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::kernel::object::{KObject, ObjectId};
use crate::kernel::sync::ResetType;
use crate::services::cecd::{IDLE, WORKING};
use crate::System;

/// the reader's states as GetTagState reports them.
const NFC_NOT_INITIALIZED: u32 = 0;
const NFC_NOT_SCANNING: u32 = 1;
const NFC_SCANNING: u32 = 2;
/// what CommunicationGetStatus reports once the reader is up.
const NFC_COMMUNICATION_READY: u32 = 2;

/// whether these answers are on: always in the pure build;
/// FATES_NATIVE_LOCAL=0 gives development builds Zakuro's services back.
fn enabled() -> bool {
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("FATES_NATIVE_LOCAL").map_or(true, |v| v != "0"))
    }
}

/// answers svcSendSyncRequest (r0 = the session) to cecd:u or nfc:u; false
/// for any other service or a request not covered.
pub fn request(system: &mut System) -> bool {
    if !enabled() {
        return false;
    }
    let handle = system.cpu.regs[0];
    let cecd = match system.kernel.handles.label(handle) {
        "cecd:u" | "cecd:s" => true,
        "nfc:u" | "nfc:m" => false,
        _ => return false,
    };
    let Some(tls) = system.kernel.current().map(|thread| thread.tls) else {
        return false;
    };
    let buffer = CommandBuffer::new(tls);
    let command = buffer.header(&mut system.memory).command_id();
    let answered = if cecd { streetpass(system, &buffer, command) } else { amiibo(system, &buffer, command) };
    let Some(name) = answered else {
        return false;
    };
    super::stats::native(name);
    system.cpu.regs[0] = 0;
    if system.kernel.current().is_some_and(|thread| thread.status == crate::kernel::thread::ThreadStatus::Running) {
        let tick = system.cpu.cycles;
        super::sched::sleep_current(system, super::svc::REQUEST_TICKS, tick);
    }
    true
}

fn streetpass(system: &mut System, buffer: &CommandBuffer, command: u16) -> Option<&'static str> {
    Some(match command {
        // Start and Stop: the game waits for the state changed event
        0x000B | 0x000C => {
            system.services.cecd.state = if command == 0x000B { WORKING } else { IDLE };
            let changed = cecd_events(system)[1];
            super::sched::signal_event(system, changed);
            buffer.reply(&mut system.memory, command, &[]);
            "cecd.StartStop"
        }
        // GetCecdState
        0x000E => {
            let state = system.services.cecd.state;
            buffer.reply(&mut system.memory, command, &[state]);
            "cecd.GetCecdState"
        }
        // GetCecInfoEventHandle and GetChangeStateEventHandle
        0x000F | 0x0010 => {
            let object = cecd_events(system)[command as usize - 0x000F];
            reply_handle(system, buffer, command, object, "cecd:u event");
            "cecd.GetEventHandle"
        }
        // OpenAndRead: the inbox reads empty, nobody was ever met
        0x0012 => {
            buffer.reply(&mut system.memory, command, &[0]);
            "cecd.OpenAndRead"
        }
        _ => return None,
    })
}

fn amiibo(system: &mut System, buffer: &CommandBuffer, command: u16) -> Option<&'static str> {
    let values: &[u32] = match command {
        // Initialize / Shutdown
        0x0001 => {
            system.services.nfc.tag_state = NFC_NOT_SCANNING;
            &[]
        }
        0x0002 => {
            system.services.nfc.tag_state = NFC_NOT_INITIALIZED;
            &[]
        }
        // StartCommunication, StopCommunication, ResetTagScanState
        0x0003 | 0x0004 | 0x0008 => &[],
        // StartTagScanning / StopTagScanning
        0x0005 => {
            system.services.nfc.tag_state = NFC_SCANNING;
            &[]
        }
        0x0006 => {
            system.services.nfc.tag_state = NFC_NOT_SCANNING;
            &[]
        }
        // GetTagInRangeEvent and GetTagOutOfRangeEvent
        0x000B | 0x000C => {
            let object = nfc_events(system)[command as usize - 0x000B];
            reply_handle(system, buffer, command, object, "nfc:u event");
            return Some("nfc.GetTagEvent");
        }
        // GetTagState
        0x000D => {
            let state = system.services.nfc.tag_state;
            buffer.reply(&mut system.memory, command, &[state]);
            return Some("nfc.GetTagState");
        }
        // CommunicationGetStatus / CommunicationGetResult
        0x000F => &[NFC_COMMUNICATION_READY],
        0x0012 => &[0],
        _ => return None,
    };
    buffer.reply(&mut system.memory, command, values);
    Some("nfc.reader")
}

/// a reply that hands the caller a handle to object.
fn reply_handle(system: &mut System, buffer: &CommandBuffer, command: u16, object: ObjectId, label: &str) {
    let handle = system.kernel.handles.create(&mut system.kernel.objects, object, label);
    buffer.set(&mut system.memory, 0, Header::new(command, 1, 2).0);
    buffer.set(&mut system.memory, 1, 0);
    buffer.set(&mut system.memory, 2, Descriptor::handles(1));
    buffer.set(&mut system.memory, 3, handle);
}

/// two one-shot events the service keeps (a title closing its handles must
/// not take them away), made the first time they are asked for.
fn make_events(system: &mut System, names: [&str; 2]) -> [ObjectId; 2] {
    names.map(|name| {
        let object = system.kernel.objects.insert(KObject::Event(crate::kernel::sync::Event::new(ResetType::OneShot, name)));
        system.kernel.objects.add_ref(object);
        object
    })
}

fn cecd_events(system: &mut System) -> [ObjectId; 2] {
    if let Some(events) = system.services.cecd.events {
        return events;
    }
    let events = make_events(system, ["cecd:u:InfoEvent", "cecd:u:ChangeStateEvent"]);
    system.services.cecd.events = Some(events);
    events
}

fn nfc_events(system: &mut System) -> [ObjectId; 2] {
    if let Some(events) = system.services.nfc.events {
        return events;
    }
    let events = make_events(system, ["nfc:u:TagInRange", "nfc:u:TagOutOfRange"]);
    system.services.nfc.events = Some(events);
    events
}
