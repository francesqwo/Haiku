//! castlecam: My Castle's third-person view, its camera while it turns.
//!
//! Dev: `FATES_CASTLECAM_LOG=file` writes one line per game tick of the
//! view's follow camera (overrides/cameras.c reports it through the cameras
//! hook, context CAM_CASTLE_REPORT): the game's frame, the wall time in
//! microseconds since the first line, then the floats the game reported
//! (see `REPORT_FIELDS`). With FATES_GFX_PACE_LOG (every present's frame and
//! fraction) and `dev/castlecam/jitter.py` it gives the camera's yaw on
//! every picture the display got.

use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

/// what each report line holds after the frame and the time.
pub const REPORT_FIELDS: &str = "se.x se.y se.z st.x st.y st.z fe.x fe.y fe.z ft.x ft.y ft.z in0 in1 in2 in3 corrin.x corrin.y corrin.z follow.x follow.y follow.z look.x look.y pull lift_raw lift";

static LOG: Mutex<Option<(Instant, std::io::BufWriter<std::fs::File>)>> = Mutex::new(None);

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_CASTLECAM_LOG").is_some())
}

/// the report comes in parts (the cameras hook's frame holds 13 floats):
/// part 0 starts a line, the others add to it, the last (`LAST_PART`)
/// writes it.
const LAST_PART: u32 = 2;
static PARTS: Mutex<Vec<f32>> = Mutex::new(Vec::new());

pub fn report_part(part: u32, values: &[f32]) {
    if !enabled() {
        return;
    }
    let mut parts = PARTS.lock().unwrap_or_else(|e| e.into_inner());
    if part == 0 {
        parts.clear();
    }
    parts.extend_from_slice(values);
    if part == LAST_PART {
        let line = std::mem::take(&mut *parts);
        drop(parts);
        report(&line);
    }
}

/// one tick's report: `values` as the game sent them.
fn report(values: &[f32]) {
    if !enabled() {
        return;
    }
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Some(path) = std::env::var_os("FATES_CASTLECAM_LOG") else { return };
        let Ok(file) = std::fs::File::create(path) else { return };
        let mut out = std::io::BufWriter::new(file);
        let _ = writeln!(out, "# frame us {REPORT_FIELDS}");
        *guard = Some((Instant::now(), out));
    }
    let (start, out) = guard.as_mut().unwrap();
    let frame = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let mut line = format!("{frame}\t{}", start.elapsed().as_micros());
    for v in values {
        line.push_str(&format!("\t{v:.5}"));
    }
    let _ = writeln!(out, "{line}");
    let _ = out.flush(); // a window is stopped by killing it
}

/// castlecam: dev, FATES_CASTLECAM_FAKE_MOUSE=<pixels a second>: a mouse
/// dragged at that steady speed, its movement coming in at the frontend's
/// polls on the wall clock as a real mouse's does (the camera script's drags
/// come with the game's frames instead), for measuring the mouse turn. In
/// real time the game's ticks run 33.4 ms apart on average but sd 6.3 ms
/// (8 to 67), so a steady mouse gives the ticks uneven turns (sd 9 % at
/// 700 pixels a second); taking the movement up to each refresh's due time
/// was tried and measured worse (a tick's logic runs after either of its
/// two refreshes): see dev/reports/castlecam.md.
pub fn fake_mouse(per_pixel: f32) -> f32 {
    static SPEED: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let Some(speed) = *SPEED.get_or_init(|| std::env::var("FATES_CASTLECAM_FAKE_MOUSE").ok().and_then(|v| v.parse().ok())) else { return 0.0 };
    let now = Instant::now();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let turn = last.map_or(0.0, |was| speed * now.saturating_duration_since(was).as_secs_f32().min(0.1) * per_pixel);
    *last = Some(now);
    turn
}
