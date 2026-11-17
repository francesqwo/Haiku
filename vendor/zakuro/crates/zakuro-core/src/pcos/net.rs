//! gate: the online services, natively and offline.
//!
//! On PC the game is never online: there are no Nintendo Network servers,
//! no friend server, no eShop and no other console nearby. The game still
//! asks, from My Castle's online features, online battles, StreetPass,
//! bonus content and the like. An IPC request to one of those services
//! (SendSyncRequest) reaches this module before anything else, and is
//! answered here the way Zakuro's HLE services answered it:
//!
//! - what a console works out by itself (ac:u's "connected?" questions,
//!   frd:u's own friend details, setting up an act:/http:/ssl: session) is
//!   answered as a console that never connected and has no friends;
//! - anything that needs the network gets NOT_CONNECTED, or "wireless is
//!   off" for local wireless (nwm::UDS, dlp:SRVR), which the game shows
//!   with its own "could not connect" messages.
//!
//! So the pure build answers these natively instead of with the "no native
//! implementation" error (an `unanswered` request), and the game sees the
//! same offline console the development build's emulated services showed.

use zakuro_common::result::errors;

use crate::kernel::ipc::CommandBuffer;
use crate::System;

/// the online services: their whole job is talking to the internet or to
/// another console (Zakuro's `is_network_service`).
fn is_network_service(name: &str) -> bool {
    matches!(
        name,
        "frd:u" | "frd:a"
            | "nwm::UDS"
            | "dlp:SRVR"
            | "http:C"
            | "ssl:C"
            | "nim:aoc" | "nim:s" | "nim:u"
            | "ac:u" | "ac:i"
            | "olv:u"
            | "act:u" | "act:a"
            | "soc:U"
    )
}

/// whether the native answers are on: always in the pure build;
/// FATES_NATIVE_NET=0 gives development builds Zakuro's services back.
fn enabled() -> bool {
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("FATES_NATIVE_NET").map_or(true, |v| v != "0"))
    }
}

/// answers svcSendSyncRequest (r0 = the session) when it goes to an online
/// service; false for any other service.
pub fn request(system: &mut System) -> bool {
    if !enabled() {
        return false;
    }
    let handle = system.cpu.regs[0];
    let service = system.kernel.handles.label(handle).to_owned();
    if !is_network_service(&service) {
        return false;
    }
    let Some(tls) = system.kernel.current().map(|thread| thread.tls) else {
        return false;
    };
    let buffer = CommandBuffer::new(tls);
    let header = buffer.header(&mut system.memory);
    let command = header.command_id();
    // online: local wireless while the game's Local Battle is up (or a
    // network it began is still going): the native service, over the
    // online session
    let answer = if service == "nwm::UDS" && super::uds::handle(system, &buffer, header) {
        Answer::Done("net.uds")
    } else {
        answer(system, &buffer, &service, command)
    };
    let name = match answer {
        Answer::Offline(code, label) => {
            buffer.reply_error(&mut system.memory, command, code);
            log_once(system, &service, command, label);
            "net.offline"
        }
        Answer::Done(name) => name,
    };
    super::stats::native(name);
    system.cpu.regs[0] = 0;
    // the thread waits for the reply as long as for any service's (see
    // kernel::svc's SendSyncRequest), so the game's threads interleave as
    // they did
    if system.kernel.current().is_some_and(|thread| thread.status == crate::kernel::thread::ThreadStatus::Running) {
        let tick = system.cpu.cycles;
        super::sched::sleep_current(system, super::svc::REQUEST_TICKS, tick);
    }
    true
}

enum Answer {
    /// a reply was written
    Done(&'static str),
    /// the request needs the network: this error, and its name for the log
    Offline(u32, &'static str),
}

fn answer(system: &mut System, buffer: &CommandBuffer, service: &str, command: u16) -> Answer {
    let memory = &mut system.memory;
    let offline = match service {
        "nwm::UDS" | "dlp:SRVR" => Answer::Offline(errors::UDS_WIRELESS_OFF.0, "WIRELESS_OFF"),
        _ => Answer::Offline(errors::NOT_CONNECTED.0, "NOT_CONNECTED"),
    };
    match service {
        // Wi-Fi connection: never connected
        "ac:u" | "ac:i" => match command {
            // GetWifiStatus (0 = not connected), GetLastErrorCode, GetStatus
            0x000A | 0x000D | 0x000E => buffer.reply(memory, command, &[0]),
            // CloseAsync and GetCloseResult
            0x0005 | 0x0008 => buffer.reply(memory, command, &[]),
            // RegisterDisconnectEvent and SetClientVersion
            0x0030 | 0x0040 => buffer.reply(memory, command, &[]),
            // IsConnected, no
            0x003E => buffer.reply(memory, command, &[0]),
            _ => return offline,
        },
        // friends: this console's own details, as one that never logged in
        // and has no friends (Zakuro's services/frd.rs)
        "frd:u" | "frd:a" => {
            let (region, language) = (system.config.region, system.config.language);
            match command {
                // HasLoggedIn and IsOnline, neither
                0x0001 | 0x0002 => buffer.reply(memory, command, &[0]),
                // Logout, AttachToEventNotification, SetNotificationMask,
                // SetClientSdkVersion
                0x0004 | 0x0020 | 0x0021 | 0x0032 => buffer.reply(memory, command, &[]),
                // GetMyFriendKey: no principal id or friend code seed
                0x0005 => buffer.reply(memory, command, &[0; 4]),
                // GetMyPreference: private
                0x0006 => buffer.reply(memory, command, &[0; 3]),
                // GetMyProfile: region, country, area, language; platform 3DS
                0x0007 => {
                    let country: u32 = if region == crate::services::cfg::REGION_USA { 49 } else { 110 };
                    let first = region as u32 | country << 8 | (language as u32) << 24;
                    buffer.reply(memory, command, &[first, 2]);
                }
                // GetMyPresence, into a static buffer that stays as it is
                0x0008 => buffer.reply(memory, command, &[]),
                // GetMyScreenName, eleven UTF-16 units
                0x0009 => {
                    let mut name = [0u8; 24];
                    for (i, unit) in "Haiku".encode_utf16().enumerate() {
                        name[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
                    }
                    let words: Vec<u32> = name.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
                    buffer.reply(memory, command, &words);
                }
                // GetMyMii
                0x000A => buffer.reply(memory, command, &[0; 24]),
                // GetMyLocalAccountId
                0x000B => buffer.reply(memory, command, &[1]),
                // GetMyPlayingGame, GetMyFavoriteGame
                0x000C | 0x000D => buffer.reply(memory, command, &[0; 4]),
                // GetMyNcPrincipalId
                0x000E => buffer.reply(memory, command, &[0]),
                // GetMyComment
                0x000F => buffer.reply(memory, command, &[0; 9]),
                // GetFriendKeyList and GetEventNotification: none
                0x0011 | 0x0022 => buffer.reply(memory, command, &[0]),
                // lookups on friends the game names, of which it has none
                0x0012..=0x001A => buffer.reply(memory, command, &[]),
                // IsIncludedInFriendList
                0x001B => buffer.reply(memory, command, &[0]),
                // GetLastResponseResult: the servers were not reached
                0x0023 => buffer.reply(memory, command, &[errors::NOT_CONNECTED.0]),
                // ResultToErrorCode: no code to show
                0x0027 => buffer.reply(memory, command, &[0]),
                _ => return offline,
            }
        }
        // sockets: Zakuro answered every soc:U request with success and
        // zeros (services::unimplemented). The SDK stops the game (svcBreak)
        // when InitializeSockets fails, which Clear Blacklist and the other
        // online paths reach before they connect; nothing is ever connected,
        // so ac:/frd:/the servers turn the request down afterwards
        "soc:U" => {
            buffer.reply(memory, command, &[]);
            for index in 2..8 {
                buffer.set(memory, index, 0);
            }
        }
        // setting a session up works offline, only its requests need the
        // network
        "act:u" | "act:a" | "http:C" | "ssl:C" if command == 0x0001 => buffer.reply(memory, command, &[]),
        // online: ssl:C GenerateRandomData(size, buffer): random bytes, from
        // the system. The game's Pia session makes its keys and ids with it
        // when an online (Local) battle starts; nothing else asks
        "ssl:C" if command == 0x0011 => {
            let size = (buffer.get(memory, 1) as usize).min(0x10000);
            let pointer = buffer.get(memory, 3);
            let mut bytes = vec![0u8; size];
            if getrandom::getrandom(&mut bytes).is_err() {
                let seed = system.cpu.cycles as usize;
                bytes = (0..size).map(|i| seed.wrapping_mul(2654435761).wrapping_add(i * 40503) as u8).collect();
            }
            memory.write_bytes(pointer, &bytes);
            buffer.reply(memory, command, &[(size as u32) << 4 | 12, pointer]);
        }
        _ => return offline,
    }
    Answer::Done(match service {
        "ac:u" | "ac:i" => "net.ac",
        "frd:u" | "frd:a" => "net.frd",
        "soc:U" => "net.soc",
        "ssl:C" => "net.ssl", // online: GenerateRandomData
        _ => "net.session",
    })
}

/// the first time each request is turned down, in the log.
fn log_once(system: &mut System, service: &str, command: u16, label: &str) {
    let count = system.services.unimplemented.entry((service.to_owned(), command)).or_insert(0);
    *count += 1;
    if *count == 1 {
        log::info!("net: {service} command 0x{command:04X} needs the network, answered {label} (offline)");
    }
}
