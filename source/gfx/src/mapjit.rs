//! mapjit: dev, where the map's meshes are on the screen in each picture.
//!
//! `FATES_MAPJIT_LOG=file`: for each upper-screen frame matched for
//! pictures in between (`hfr_track`), one vertex of each 3D mesh draw (its
//! first index) run through the draw's own vertex program, as the pictures
//! in between place it at t = 0, 1/2 and 1 (t = 0 is where the picture after
//! the frame before starts it, t = 1 the frame's own place). Lines:
//!
//! ```text
//! F  frame-id game-frame us draws moves held
//! D  n key vertex-bytes copies moves x0 y0 xh yh x1 y1 w1
//! ```
//!
//! x is the screen's long axis, y its short axis, both in normalised device
//! units (-1..1), w the vertex's distance along the view. `copies` is how many
//! draws of the frame have the same key (a tree clump drawn 15 times: 15).
//! `FATES_MAPJIT_MOST=n` logs at most n draws a frame (largest first), for
//! real-time runs; default all.
//!
//! Nothing here runs unless the variable is set; a player never has it.

use std::io::Write;
use std::sync::Mutex;

use crate::hfr::Between;
use crate::prim::Draw;

static LOG: Mutex<Option<(std::time::Instant, std::io::BufWriter<std::fs::File>)>> = Mutex::new(None);

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_MAPJIT_LOG").is_ok())
}

fn most() -> usize {
    static MOST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MOST.get_or_init(|| crate::perf::var("FATES_MAPJIT_MOST").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX))
}

/// a frame's draws and their matches, after hfr_track's rules (holds
/// included).
pub(crate) fn frame(id: u64, draws: &[Draw], between: &Between) {
    if !on() {
        return;
    }
    let Ok(path) = crate::perf::var("FATES_MAPJIT_LOG") else { return };
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Ok(file) = std::fs::File::create(&path) else { return };
        *guard = Some((std::time::Instant::now(), std::io::BufWriter::new(file)));
    }
    let (start, out) = guard.as_mut().unwrap();
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let moves = between.tracks.iter().filter(|t| t.moves).count();
    let _ = writeln!(out, "F\t{id}\t{game}\t{}\t{}\t{moves}\t{}", start.elapsed().as_micros(), draws.len(), between.vanished);
    let keys: Vec<Option<u64>> = draws.iter().map(crate::hfr::draw_key).collect();
    let mut copies: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for k in keys.iter().flatten() {
        *copies.entry(*k).or_default() += 1;
    }
    let mut order: Vec<usize> = (0..draws.len()).filter(|&n| draws[n].shaded.as_deref().is_some_and(|s| !s.loose && !s.indices.is_empty())).collect();
    if most() < order.len() {
        order.sort_by_key(|&n| std::cmp::Reverse(draws[n].shaded.as_deref().map_or(0, |s| s.vertices.len())));
        order.truncate(most());
        order.sort_unstable();
    }
    for n in order {
        let s = draws[n].shaded.as_deref().unwrap();
        let track = between.tracks.get(n);
        let moving = track.is_some_and(|t| t.moves);
        let at = |t: f32| -> Option<(f32, f32, f32)> {
            let placed;
            let s = match (moving, track) {
                (true, Some(tr)) => {
                    placed = crate::hfr::shaded_at(s, &tr.changed, t);
                    &placed
                }
                _ => s,
            };
            crate::wide2::project_points(s, &[s.indices[0]]).into_iter().next()
        };
        let (Some(p0), Some(ph), Some(p1)) = (at(0.0), at(0.5), at(1.0)) else { continue };
        let key = keys[n].unwrap_or(0);
        let _ = writeln!(
            out,
            "D\t{n}\t{key:016x}\t{}\t{}\t{}\t{:.5}\t{:.5}\t{:.5}\t{:.5}\t{:.5}\t{:.5}\t{:.2}",
            s.vertices.len(),
            copies.get(&key).copied().unwrap_or(1),
            moving as u8,
            p0.0,
            p0.1,
            ph.0,
            ph.1,
            p1.0,
            p1.1,
            p1.2
        );
    }
    let _ = out.flush();
}
