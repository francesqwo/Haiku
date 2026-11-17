//! os: counts of what still reaches the emulated kernel and services, and
//! of what the native OS layer answered instead, per frame. Off unless
//! FATES_OS_STATS names a file; then the report is rewritten there every
//! FATES_OS_STATS_EVERY frames (default 500), and a line per report is
//! appended to the same name with a .tsv extension.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Counts {
    svc: BTreeMap<u32, u64>,
    ipc: BTreeMap<(String, u16), u64>,
    native: BTreeMap<String, u64>,
    /// svc number and the address after it, with FATES_OS_STATS_SITES
    sites: BTreeMap<(u32, u32), u64>,
}

impl Counts {
    fn totals(&self) -> (u64, u64, u64) {
        (self.svc.values().sum(), self.ipc.values().sum(), self.native.values().sum())
    }
}

struct Stats {
    path: std::path::PathBuf,
    every: u64,
    total: Counts,
    window: Counts,
    window_start: u64,
}

fn stats() -> Option<&'static Mutex<Stats>> {
    static STATS: OnceLock<Option<Mutex<Stats>>> = OnceLock::new();
    STATS
        .get_or_init(|| {
            let path = std::env::var_os("FATES_OS_STATS")?;
            let every = std::env::var("FATES_OS_STATS_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
            Some(Mutex::new(Stats { path: path.into(), every, total: Counts::default(), window: Counts::default(), window_start: 0 }))
        })
        .as_ref()
}

/// whether counting is on.
pub fn enabled() -> bool {
    stats().is_some()
}

/// an svc reached the emulated kernel, from the code at pc (after it).
pub fn svc(number: u32, pc: u32) {
    static SITES: OnceLock<bool> = OnceLock::new();
    if let Some(stats) = stats() {
        let mut s = stats.lock().unwrap();
        *s.total.svc.entry(number).or_default() += 1;
        *s.window.svc.entry(number).or_default() += 1;
        if *SITES.get_or_init(|| std::env::var_os("FATES_OS_STATS_SITES").is_some()) {
            *s.total.sites.entry((number, pc)).or_default() += 1;
        }
    }
}

/// an IPC request reached an emulated service.
pub fn ipc(service: &str, command: u16) {
    if let Some(stats) = stats() {
        let mut s = stats.lock().unwrap();
        *s.total.ipc.entry((service.to_owned(), command)).or_default() += 1;
        *s.window.ipc.entry((service.to_owned(), command)).or_default() += 1;
    }
}

/// the native OS layer answered a call itself.
pub fn native(name: &str) {
    if let Some(stats) = stats() {
        let mut s = stats.lock().unwrap();
        *s.total.native.entry(name.to_owned()).or_default() += 1;
        *s.window.native.entry(name.to_owned()).or_default() += 1;
    }
}

pub fn svc_name(number: u32) -> &'static str {
    match number {
        0x01 => "ControlMemory",
        0x02 => "QueryMemory",
        0x03 => "ExitProcess",
        0x08 => "CreateThread",
        0x09 => "ExitThread",
        0x0A => "SleepThread",
        0x0B => "GetThreadPriority",
        0x0C => "SetThreadPriority",
        0x11 => "GetCurrentProcessorNumber",
        0x13 => "CreateMutex",
        0x14 => "ReleaseMutex",
        0x15 => "CreateSemaphore",
        0x16 => "ReleaseSemaphore",
        0x17 => "CreateEvent",
        0x18 => "SignalEvent",
        0x19 => "ClearEvent",
        0x1A => "CreateTimer",
        0x1B => "SetTimer",
        0x1C => "CancelTimer",
        0x1D => "ClearTimer",
        0x1E => "CreateMemoryBlock",
        0x1F => "MapMemoryBlock",
        0x20 => "UnmapMemoryBlock",
        0x21 => "CreateAddressArbiter",
        0x22 => "ArbitrateAddress",
        0x23 => "CloseHandle",
        0x24 => "WaitSynchronization1",
        0x25 => "WaitSynchronizationN",
        0x27 => "DuplicateHandle",
        0x28 => "GetSystemTick",
        0x2A => "GetSystemInfo",
        0x2B => "GetProcessInfo",
        0x2D => "ConnectToPort",
        0x32 => "SendSyncRequest",
        0x35 => "GetProcessId",
        0x37 => "GetThreadId",
        0x38 => "GetResourceLimit",
        0x39 => "GetResourceLimitLimitValues",
        0x3A => "GetResourceLimitCurrentValues",
        0x3C => "Break",
        0x3D => "OutputDebugString",
        0x54..=0x56 => "CacheMaintenance",
        _ => "?",
    }
}

fn render(counts: &Counts, frames: u64, title: &str) -> String {
    let n = frames.max(1) as f64;
    let (svcs, ipcs, natives) = counts.totals();
    let mut out = format!(
        "## {title}: {frames} frames\n\nper frame: {:.2} SVCs, {:.2} IPC requests, {:.2} native OS calls\n\n",
        svcs as f64 / n,
        ipcs as f64 / n,
        natives as f64 / n
    );
    let mut svc: Vec<_> = counts.svc.iter().collect();
    svc.sort_by(|a, b| b.1.cmp(a.1));
    out += "| svc | name | count | per frame |\n|---|---|---|---|\n";
    for (number, count) in svc {
        out += &format!("| 0x{number:02X} | {} | {count} | {:.3} |\n", svc_name(*number), *count as f64 / n);
    }
    let mut by_service: BTreeMap<&str, u64> = BTreeMap::new();
    for ((service, _), count) in &counts.ipc {
        *by_service.entry(service).or_default() += count;
    }
    let mut services: Vec<_> = by_service.into_iter().collect();
    services.sort_by(|a, b| b.1.cmp(&a.1));
    out += "\n| service | IPC requests | per frame |\n|---|---|---|\n";
    for (service, count) in services {
        out += &format!("| {service} | {count} | {:.3} |\n", count as f64 / n);
    }
    let mut ipc: Vec<_> = counts.ipc.iter().collect();
    ipc.sort_by(|a, b| b.1.cmp(a.1));
    out += "\n| service | command | count | per frame |\n|---|---|---|---|\n";
    for ((service, command), count) in ipc {
        out += &format!("| {service} | 0x{command:04X} | {count} | {:.3} |\n", *count as f64 / n);
    }
    if !counts.sites.is_empty() {
        let mut sites: Vec<_> = counts.sites.iter().collect();
        sites.sort_by(|a, b| b.1.cmp(a.1));
        out += "\n| svc | after | count | per frame |\n|---|---|---|---|\n";
        for ((number, pc), count) in sites.into_iter().take(60) {
            out += &format!("| 0x{number:02X} {} | 0x{pc:08X} | {count} | {:.3} |\n", svc_name(*number), *count as f64 / n);
        }
    }
    if !counts.native.is_empty() {
        let mut native: Vec<_> = counts.native.iter().collect();
        native.sort_by(|a, b| b.1.cmp(a.1));
        out += "\n| native call | count | per frame |\n|---|---|---|\n";
        for (name, count) in native {
            out += &format!("| {name} | {count} | {:.3} |\n", *count as f64 / n);
        }
    }
    out + "\n"
}

/// a frame ended; frame is the system's frame count.
pub fn frame(frame: u64) {
    let Some(stats) = stats() else { return };
    let mut s = stats.lock().unwrap();
    if frame == 0 || frame % s.every != 0 {
        return;
    }
    let window = render(&s.window, frame - s.window_start, &format!("frames {}..{}", s.window_start, frame));
    let mut report = String::from("# OS calls reaching emulation\n\n");
    report += &render(&s.total, frame, "since boot");
    report += &window;
    let _ = std::fs::write(&s.path, report);
    let (svcs, ipcs, natives) = s.window.totals();
    let line = format!("{}\t{frame}\t{svcs}\t{ipcs}\t{natives}\n", s.window_start);
    s.window = Counts::default();
    s.window_start = frame;
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(s.path.with_extension("tsv")) {
        let _ = file.write_all(line.as_bytes());
    }
}
