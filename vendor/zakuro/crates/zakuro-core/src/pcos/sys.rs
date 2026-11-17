//! os: native system services, what overrides/os_sys.c calls in place of
//! SDK functions that would make an SVC or reach a service over IPC:
//! the system clock, system settings (nn::cfg), the platform query of
//! nn::applet, and the online services, which on PC are offline (nn::ndm,
//! nn::boss, nn::nim).
//!
//! Each answers what the emulated service answered, so the game sees the
//! same console as before; only the route is native.

use zakuro_common::result::errors;
use zakuro_cpu::Bus;

use super::bridge::Call;
use super::stats;
use crate::System;

/// call numbers, the same as overrides/os_sys.c
pub mod op {
    pub const TICK: u32 = 0x200;
    pub const CFG_GET_CONFIG: u32 = 0x210;
    pub const CFG_GET_REGION: u32 = 0x211;
    pub const CFG_GET_TRANSFERABLE_ID: u32 = 0x212;
    pub const APPLET_GET_TARGET_PLATFORM: u32 = 0x220;
    pub const NDM_NO_OP: u32 = 0x230;
    pub const BOSS_INITIALIZE_SESSION: u32 = 0x240;
    pub const BOSS_RECEIVE_PROPERTY: u32 = 0x241;
    pub const NIM_AOC_INITIALIZE: u32 = 0x250;
    pub const AM_GET_NUM_CONTENT_INFOS: u32 = 0x260;
    pub const AM_LIST_CONTENT_INFOS: u32 = 0x261;
    pub const AM_GET_DATA_TITLE_INFOS: u32 = 0x262;
    pub const AM_LIST_DATA_TITLE_TICKET_INFOS: u32 = 0x263;
    pub const BOSS_NO_OP: u32 = 0x242;
    pub const SRV_GET_SERVICE_HANDLE: u32 = 0x270;
    pub const SRV_REGISTER_CLIENT: u32 = 0x271;
    pub const SRV_SUBSCRIBE: u32 = 0x272;
    pub const HID_GET_IPC_HANDLES: u32 = 0x280;
    pub const APPLET_NO_OP: u32 = 0x290;
    pub const APPLET_UTILITY: u32 = 0x291;
    pub const APPLET_SET_CPU_TIME_LIMIT: u32 = 0x292;
    pub const ERR_THROW_FATAL: u32 = 0x2A0; // gate:
}

/// runs a native system call, false when it is one to leave to IPC.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    let name = match number {
        // svcGetSystemTick and nn::os::Tick::GetSystemCurrent: the clock the
        // guest scheduler keeps, as far as this thread has run, and the
        // time the call takes on the console
        op::TICK => {
            // SAFETY: the context is the run's, see bridge::os_native
            let tick = unsafe { crate::recompiled::ticks_of(&mut *call.ctx) };
            call.ctx.r[0] = tick as u32;
            call.ctx.r[1] = (tick >> 32) as u32;
            call.ctx.budget -= crate::pcos::svc::SYSTEM_TICK_COST as i32;
            // an svc, not a request: no wait
            call.wait = None;
            "os.GetSystemTick"
        }
        // nn::cfg GetConfig(void* out, size, block id)
        op::CFG_GET_CONFIG => {
            let (out, size, block) = (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2));
            crate::services::cfg::write_config_block(system, block, out, size);
            call.ret(0);
            "cfg.GetConfig"
        }
        // nn::cfg GetRegion(CfgRegionCode* out)
        op::CFG_GET_REGION => {
            let out = call.arg(system, 0);
            let region = system.config.region;
            system.memory.write8(out, region);
            call.ret(0);
            "cfg.GetRegion"
        }
        // nn::cfg GetTransferableId(salt, u64* out)
        op::CFG_GET_TRANSFERABLE_ID => {
            let out = call.arg(system, 1);
            let hash = crate::services::cfg::CONSOLE_HASH;
            system.memory.write32(out, hash as u32);
            system.memory.write32(out + 4, (hash >> 32) as u32);
            call.ret(0);
            "cfg.GetTransferableId"
        }
        // nn::applet GetTargetPlatform(TargetPlatform* out): 1 a 3DS, 2 a New 3DS
        op::APPLET_GET_TARGET_PLATFORM => {
            let out = call.arg(system, 0);
            let platform = if system.config.new3ds { 2 } else { 1 };
            system.memory.write8(out, platform);
            call.ret(0);
            "applet.GetTargetPlatform"
        }
        // nn::ndm Suspend/Resume/OverrideDefault daemons, Enter/Leave
        // exclusive state: there are no network daemons on PC
        op::NDM_NO_OP => {
            call.ret(0);
            "ndm.daemons"
        }
        // nn::boss InitializeSession: SpotPass has nothing to deliver
        // offline, its session opens and stays empty
        op::BOSS_INITIALIZE_SESSION => {
            call.ret(0);
            "boss.InitializeSession"
        }
        // nn::boss ReceiveProperty(this, property, u8* buffer, size,
        // u32* out size): the tasks the game registered, zeros for the rest
        op::BOSS_RECEIVE_PROPERTY => {
            use crate::services::boss::{TASK_ID_LIST, TOTAL_TASKS};
            let property = call.arg(system, 1) & 0xFFFF;
            let (buffer, size, out) = (call.arg(system, 2), call.arg(system, 3), call.arg(system, 4));
            let mut value = vec![0u8; size.min(0x1_0000) as usize];
            let tasks = &system.services.boss.tasks;
            let known: Vec<u8> = match property {
                TOTAL_TASKS => (tasks.len() as u16).to_le_bytes().to_vec(),
                TASK_ID_LIST => tasks.iter().flatten().copied().collect(),
                _ => Vec::new(),
            };
            let shared = known.len().min(value.len());
            value[..shared].copy_from_slice(&known[..shared]);
            if buffer != 0 {
                system.memory.write_bytes(buffer, &value);
            }
            if out != 0 {
                system.memory.write32(out, size);
            }
            call.ret(0);
            "boss.ReceiveProperty"
        }
        // nn::nim Aoc::Initialize: the eShop is not reachable offline
        op::NIM_AOC_INITIALIZE => {
            call.ret(errors::NOT_CONNECTED.0);
            "nim.Aoc.Initialize"
        }
        // nn::boss GetTaskIdList(this): offline, nothing to list
        op::BOSS_NO_OP => {
            call.ret(0);
            "boss.GetTaskIdList"
        }
        op::AM_GET_NUM_CONTENT_INFOS..=op::AM_LIST_DATA_TITLE_TICKET_INFOS => add_on_content(system, call, number),
        // gate: nn::err FatalErr::Throw(this, const FatalErrInfo& info): the
        // report err:f took, logged as Zakuro's err:f logged it. The SDK
        // then stops the thread that threw, as on the console.
        op::ERR_THROW_FATAL => {
            let info = call.arg(system, 1);
            report_fatal_error(system, info);
            call.ret(0);
            "err.ThrowFatal"
        }
        // nn::srv GetServiceHandle(Handle* out, const char* name, int length,
        // flags): a session to the service, made directly
        op::SRV_GET_SERVICE_HANDLE => {
            let (out, name, length) = (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2));
            if !(1..=8).contains(&length) {
                return false;
            }
            let mut bytes = vec![0u8; length as usize];
            system.memory.read_bytes(name, &mut bytes);
            let name = String::from_utf8_lossy(&bytes).trim_end_matches(char::from(0)).to_owned();
            use crate::kernel::object::{ClientSession, KObject};
            let object = system.kernel.objects.insert(KObject::ClientSession(ClientSession { service: name.clone(), subhandle: 0 }));
            let handle = system.kernel.handles.create(&mut system.kernel.objects, object, &name);
            log::debug!("os: srv GetServiceHandle(\"{name}\") -> 0x{handle:X}");
            system.services_seen.insert(name);
            system.memory.write32(out, handle);
            call.ret(0);
            "srv.GetServiceHandle"
        }
        // RegisterClient, Subscribe(notification): nothing to do natively
        op::SRV_REGISTER_CLIENT | op::SRV_SUBSCRIBE => {
            call.ret(0);
            "srv.RegisterClient/Subscribe"
        }
        // nn::hid GetIPCHandles(six Handle*): the shared memory and the
        // five input events
        op::HID_GET_IPC_HANDLES => {
            crate::pcos::hid::ensure_resources(system); // native-final
            let mut handles = vec![system.services.hid.shared_memory_handle.unwrap_or(0)];
            handles.extend(system.services.hid.events.iter().copied());
            for i in 0..6 {
                let out = call.arg(system, i);
                if out != 0 {
                    system.memory.write32(out, handles.get(i).copied().unwrap_or(0));
                }
            }
            call.ret(0);
            "hid.GetIPCHandles"
        }
        // nn::applet NotifyToWait, SetScreenCapturePostPermission
        op::APPLET_NO_OP => {
            call.ret(0);
            "applet.NotifyToWait/SetScreenCapturePostPermission"
        }
        // AppletUtility(id, in, in size, out, out size, int* result)
        op::APPLET_UTILITY => {
            let result = call.arg(system, 5);
            if result != 0 {
                system.memory.write32(result, 0);
            }
            call.ret(0);
            "applet.AppletUtility"
        }
        // SetApplicationCpuTimeLimit(int, percent)
        op::APPLET_SET_CPU_TIME_LIMIT => {
            system.services.apt.cpu_time_limit = call.arg(system, 1);
            call.ret(0);
            "applet.SetApplicationCpuTimeLimit"
        }
        _ => return false,
    };
    stats::native(name);
    true
}

/// nn::am, what the game asks about its downloadable content: the DLC
/// installed in data/dlc (or given as a CIA), the same answers am:app gave.
fn add_on_content(system: &mut System, call: &mut Call, number: u32) -> &'static str {
    use crate::services::misc::{write_content_info, CONTENT_INFO_SIZE, TITLE_NOT_FOUND};
    match number {
        // GetNumContentInfos(int* out, media, u64 title)
        op::AM_GET_NUM_CONTENT_INFOS => {
            let out = call.arg(system, 0);
            let title = call.arg64(system, 2);
            match system.dlc.iter().find(|dlc| dlc.title_id == title) {
                Some(dlc) => {
                    let count = dlc.contents.len() as u32;
                    system.memory.write32(out, count);
                    call.ret(0);
                }
                None => call.ret(TITLE_NOT_FOUND),
            }
            "am.GetNumContentInfos"
        }
        // ListContentInfos(int* out, ContentInfo* infos, count, media, u64
        // title, first)
        op::AM_LIST_CONTENT_INFOS => {
            let (out, infos, count) = (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2) as usize);
            let title = call.arg64(system, 4);
            let first = call.arg(system, 6) as usize;
            let Some(dlc) = system.dlc.iter().find(|dlc| dlc.title_id == title) else {
                call.ret(TITLE_NOT_FOUND);
                return "am.ListContentInfos";
            };
            let contents: Vec<zakuro_fs::Content> = dlc.contents.iter().skip(first).take(count).copied().collect();
            for (i, content) in contents.iter().enumerate() {
                write_content_info(system, infos + i as u32 * CONTENT_INFO_SIZE, content);
            }
            system.memory.write32(out, contents.len() as u32);
            call.ret(0);
            "am.ListContentInfos"
        }
        // GetDataTitleInfos(ProgramInfo* out, media, const u64* titles, count)
        op::AM_GET_DATA_TITLE_INFOS => {
            let (out, titles, count) = (call.arg(system, 0), call.arg(system, 2), call.arg(system, 3));
            let mut missing = false;
            for i in 0..count {
                let title = system.memory.read32(titles + i * 8) as u64 | (system.memory.read32(titles + i * 8 + 4) as u64) << 32;
                let version = system.dlc.iter().find(|dlc| dlc.title_id == title).map(|dlc| dlc.version);
                log::info!("os: the game asked natively for DLC {title:016X}: {}", version.map_or("not installed".to_owned(), |v| format!("installed, v{v}")));
                let Some(version) = version else {
                    missing = true;
                    continue;
                };
                let at = out + i * 0x18;
                for (offset, word) in [(0, title as u32), (4, (title >> 32) as u32), (8, 0), (12, 0), (16, version as u32), (20, 0x40)] {
                    system.memory.write32(at + offset, word);
                }
            }
            call.ret(if missing { TITLE_NOT_FOUND } else { 0 });
            "am.GetDataTitleInfos"
        }
        // ListDataTitleTicketInfos(int* out, TicketInfo* infos, count, u64
        // title, first): a ticket for each DLC installed
        _ => {
            let (out, infos, count) = (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2));
            let title = call.arg64(system, 4);
            let first = call.arg(system, 6);
            let version = system.dlc.iter().find(|dlc| dlc.title_id == title).map(|dlc| dlc.version);
            match version.filter(|_| first == 0 && count > 0) {
                Some(version) => {
                    for (offset, word) in [(0, title as u32), (4, (title >> 32) as u32), (8, 0), (12, 0), (16, version as u32), (20, 0)] {
                        system.memory.write32(infos + offset, word);
                    }
                    system.memory.write32(out, 1);
                }
                None => system.memory.write32(out, 0),
            }
            call.ret(0);
            "am.ListDataTitleTicketInfos"
        }
    }
}

/// gate: what the game put in a FatalErrInfo (the words err:f's Throw
/// request carries after its header), logged and kept in fatal_errors.
fn report_fatal_error(system: &mut System, info: u32) {
    let first = system.memory.read32(info);
    let kind = (first & 0xFF) as u8;
    let result = system.memory.read32(info + 4);
    let pc = system.memory.read32(info + 8);
    let kind_name = match kind {
        0 => "generic",
        1 => "corrupted",
        2 => "card removed",
        3 => "exception",
        4 => "result failure",
        5 => "logged",
        _ => "unknown",
    };
    log::error!("=== the game reported a fatal error ===");
    log::error!("  type    {kind_name} ({kind})");
    log::error!("  result  0x{result:08X}");
    log::error!("  pc      0x{pc:08X}");
    if kind != 3 {
        let mut bytes = [0u8; 0x60];
        system.memory.read_bytes(info + 0x10, &mut bytes);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let message = String::from_utf8_lossy(&bytes[..end]);
        if !message.trim().is_empty() && message.chars().all(|c| !c.is_control()) {
            log::error!("  message \"{}\"", message.trim());
        }
    }
    system.fatal_errors.push(format!("{kind_name} error 0x{result:08X} at pc 0x{pc:08X}"));
}
