//! cpu: native-only execution of the game's code, and the headless tools
//! for checking it.
//!
//! - `enable(on)` turns native-only mode on: reaching code that would need
//!   Zakuro's ARM interpreter logs the address and the guest callstack and
//!   stops with a message (a dialog when there is a window), see
//!   `zakuro_core::coverage`. fates-pc turns it on unless
//!   `FATES_ALLOW_INTERPRETER=1`.
//! - headless runs take `FATES_LOAD=<state>` (go on from a saved state, at
//!   its frame, the replay following on at that frame) and
//!   `FATES_SAVE_AT=<frame>:<file>[,<frame>:<file>...]`, so a long replay
//!   can be cut into checkpoints.
//! - headless runs print frame-time percentiles at the end, and with
//!   `FATES_CPU_PROFILE=1` the cost of the CPU dispatch, see
//!   `zakuro_core::System::cpu_profile`.

use std::time::Duration;

use zakuro_core::System;

/// turns native-only execution on or off, and has the stop message shown in
/// a dialog as well as the log.
pub fn enable(on: bool) {
    zakuro_core::coverage::set_native_only(on);
    zakuro_core::coverage::set_stop_notice(notice);
}

fn notice(message: &str) {
    // a dialog only makes sense with a window, a headless run has the log
    if std::env::args().any(|arg| arg == "--headless") {
        return;
    }
    let short: String = message.lines().take(4).collect::<Vec<_>>().join("\n");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Haiku: code that was not recompiled")
        .set_description(short)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// the saved state FATES_LOAD names, loaded, and the frame it was saved at.
pub fn headless_load(system: &mut System) -> Option<u64> {
    let file = std::env::var_os("FATES_LOAD").filter(|file| !file.is_empty())?;
    let started = std::time::Instant::now();
    // statecheck: refused when made with another file layer
    match std::fs::read(&file).map_err(|error| error.to_string()).and_then(|bytes| crate::statecheck::load_raw(system, &bytes, &file.to_string_lossy())) {
        Ok(_) => {
            println!("loaded {} at frame {} in {:.2?}", file.to_string_lossy(), system.frames, started.elapsed());
            Some(system.frames)
        }
        Err(error) => {
            eprintln!("could not load {}: {error}", file.to_string_lossy());
            std::process::exit(1);
        }
    }
}

/// the frames FATES_SAVE_AT saves the state at, and where.
pub fn save_points() -> Vec<(u64, String)> {
    std::env::var("FATES_SAVE_AT")
        .unwrap_or_default()
        .split(',')
        .filter_map(|spec| {
            let (at, file) = spec.split_once(':')?;
            Some((at.trim().parse().ok()?, file.trim().to_owned()))
        })
        .collect()
}

/// saves the state when frame is one of the save points.
pub fn headless_save(system: &mut System, frame: u64, points: &[(u64, String)]) {
    for (_, file) in points.iter().filter(|(at, _)| *at == frame) {
        match system.save_state().and_then(|bytes| std::fs::write(file, &bytes).map_err(|error| error.to_string())) {
            Ok(()) => println!("frame {frame}: saved the state to {file}"),
            Err(error) => println!("frame {frame}: could not save the state, {error}"),
        }
    }
}

/// vowperf: HAIKU_FRAME_LOG=<file> writes how long each frame of a headless
/// run took, one line a frame (the frame, its microseconds), for finding the
/// long frames of a scene. A test tool, off unless named.
pub fn frame_log(first: u64, times: &[Duration]) {
    let Some(path) = std::env::var_os("HAIKU_FRAME_LOG") else { return };
    let mut text = String::with_capacity(times.len() * 12);
    for (i, time) in times.iter().enumerate() {
        text += &format!("{}	{}
", first + i as u64 + 1, time.as_micros());
    }
    let _ = std::fs::write(&path, text);
}

/// what a headless run prints about how long its frames took.
pub fn frame_times(times: &mut [Duration]) -> String {
    if times.is_empty() {
        return String::new();
    }
    times.sort_unstable();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    let total: Duration = times.iter().sum();
    format!(
        "frame time: mean {:.2?}, p50 {:.2?}, p90 {:.2?}, p99 {:.2?}, max {:.2?} over {} frames",
        total / times.len() as u32,
        at(0.5),
        at(0.9),
        at(0.99),
        times[times.len() - 1],
        times.len()
    )
}

/// FATES_DUMP_TEXT=<file> writes the game's code segment as it is in memory
/// at the end of a headless run, for checking the recompiler's work offline.
pub fn dump_text(system: &mut System) {
    let Some(path) = std::env::var_os("FATES_DUMP_TEXT") else { return };
    let mut bytes = vec![0u8; 5_308_700];
    system.memory.read_bytes(0x0010_0000, &mut bytes);
    match std::fs::write(&path, &bytes) {
        Ok(()) => println!("wrote the code segment to {}", path.to_string_lossy()),
        Err(error) => println!("could not write {}: {error}", path.to_string_lossy()),
    }
}
