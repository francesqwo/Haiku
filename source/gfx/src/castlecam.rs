//! castlecam: models the camera turned out of view, in the pictures in
//! between.
//!
//! The game draws only what is in its camera's view. While the camera
//! turns (My Castle's third-person view, a battle camera panning), models
//! leave the view at its trailing edge: in the frame before they were
//! drawn, in the frame they are not. A picture in between shows the camera
//! part of the way, where those models are still partly in view, but the
//! frame has no draw for them: they left a bare strip at the screen's edge.
//! gfxleft's rule then held the whole frame (no pictures in between: the
//! picture stood for the game's whole frame, then jumped), and turning in
//! My Castle that was a frame held every 7-20 frames (the castle's walls and
//! ground pieces going out of view), a stutter several times a turn, and
//! walking more often still.
//!
//! A model gone from the frame is drawn in the pictures in between as a
//! "ghost" when it can be placed by the camera's move: its camera rows
//! (rows with the very numbers a model in both frames had in the frame
//! before, which that model moved: the view matrix) move as that model's
//! moved, its own rows (its place in the world) stay, much as gfxmap's
//! `follow_camera` places a model new in the frame (that one asks for the
//! same program; here any program will do, the castle's billboards and
//! walls having programs of their own). Only when the ghost, placed by the frame's camera, is out of the
//! view (so the camera's turn is why it went), opaque and writing depth.
//! A model gone for another reason (hidden, swapped for another) is left
//! to the rules as before. FATES_GFX_HFR_GHOSTS=0 turns this off.

use std::collections::HashMap;
use std::sync::Arc;

use crate::hfr::Track;
use crate::prim::Draw;

/// a model of the frame before drawn in the frame's pictures in between.
#[derive(Clone)]
pub struct Ghost {
    /// drawn before the frame's draw `at` (its place among them)
    pub at: usize,
    /// the frame before's draw with its camera rows as the frame has them
    pub draw: Draw,
    /// the camera rows' values in the frame before
    pub track: Track,
    /// its draw in the frame before, and that draw's texture keys (set by
    /// hfr_track: the renderer finds its textures by them)
    pub from: usize,
    pub keys: [Option<u64>; 3],
}

pub fn ghosts_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_GHOSTS").map_or(true, |v| v != "0"))
}

fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_GFX_HFR_TRACE").is_ok_and(|v| v == "1" || v == "ghosts"))
}

/// the ghosts of the frame before's models gone now (`taken`: matched by a
/// draw of the frame), and per draw of the frame before whether it got one.
pub(crate) fn ghosts(before: &[Draw], now: &[Draw], tracks: &[Track], matched_to: &[Option<usize>], taken: &[bool]) -> (Vec<Ghost>, Vec<bool>) {
    let mut out = Vec::new();
    let mut ghosted = vec![false; before.len()];
    if !ghosts_on() {
        return (out, ghosted);
    }
    // the frame's draw that matched each draw of the frame before
    let mut now_of = vec![None; before.len()];
    for (n, m) in matched_to.iter().enumerate() {
        if let Some(b) = *m {
            if b < before.len() {
                now_of[b] = Some(n);
            }
        }
    }
    // the rows the frame's models moved, by their numbers in the frame
    // before: their numbers now and before
    let mut rows: HashMap<[u32; 4], ([u32; 4], [f32; 4])> = HashMap::new();
    for (m, d) in now.iter().enumerate() {
        let (Some(bm), true) = (matched_to[m], tracks[m].moves) else { continue };
        let (Some(o), Some(ob)) = (d.shaded.as_deref(), before.get(bm).and_then(|d| d.shaded.as_deref())) else { continue };
        let mut bits = o.placing.0 | o.placing.1;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let in_row: Vec<&(u16, f32, bool)> = tracks[m].changed.iter().filter(|c| c.0 as usize / 4 == u).collect();
            if in_row.is_empty() || in_row.iter().any(|c| !c.2) {
                continue;
            }
            let (Some(was), Some(is)) = (row_of(&ob.shading, u), row_of(&o.shading, u)) else { continue };
            rows.entry(was).or_insert((is, was.map(f32::from_bits)));
        }
    }
    let mut skipped = 0;
    for b in 0..before.len() {
        if taken.get(b).copied().unwrap_or(true) {
            continue;
        }
        let Some(s) = before[b].shaded.as_deref() else { continue };
        let opaque = s.key.blend.map_or(true, |x| x >> 16 == 0x0101);
        if s.loose || !opaque || !s.key.depth_write {
            continue;
        }
        // its rows that a model in both frames had, the same numbers, in the
        // frame before, and moved (whole, none jumped): the camera's
        let mut shaded = s.clone();
        let mut changed: Vec<(u16, f32, bool)> = Vec::new();
        let mut found = 0;
        let mut bits = s.placing.0 | s.placing.1;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let Some(row) = row_of(&s.shading, u) else { continue };
            let Some((now_row, was)) = rows.get(&row) else { continue };
            for c in 0..4 {
                shaded.shading[u * 4 + c] = now_row[c];
                if now_row[c] != row[c] {
                    changed.push(((u * 4 + c) as u16, was[c], true));
                }
            }
            found += 1;
        }
        // a matrix's three rows at least
        if found < 3 || changed.is_empty() {
            continue;
        }
        // placed by the frame's camera, it must be out of the view: else
        // it went for another reason than the camera's move
        if !out_of_view(&shaded) {
            skipped += 1;
            continue;
        }
        let mut draw = before[b].clone();
        draw.shaded = Some(Arc::new(shaded));
        // after the frame's draw of the nearest model before it in the frame before
        let at = (0..b).rev().find_map(|p| now_of[p]).map_or(0, |n| n + 1);
        out.push(Ghost { at, draw, track: Track::moving(changed), from: b, keys: [None; 3] });
        ghosted[b] = true;
    }
    if trace() && (!out.is_empty() || skipped > 0) {
        eprintln!(
            "castlecam ghosts: game {} {} models gone with the camera's turn drawn in between, {} gone in view left alone",
            zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed),
            out.len(),
            skipped
        );
    }
    (out, ghosted)
}

fn row_of(shading: &[u32], u: usize) -> Option<[u32; 4]> {
    Some([*shading.get(u * 4)?, *shading.get(u * 4 + 1)?, *shading.get(u * 4 + 2)?, *shading.get(u * 4 + 3)?])
}

/// models new in the frame (camera's turn brought them into view) that
/// gfxmap's follow_camera could not place (it asks for a matched model of
/// the same program): their rows with the very numbers a model in both
/// frames has now, which that model moved, move as that model's did (their
/// own rows stay). The tracks, by draw.
pub(crate) fn follow_new(before: &[Draw], now: &[Draw], tracks: &[Track], matched_to: &[Option<usize>], placed: &[bool]) -> Vec<(usize, Track)> {
    let mut out = Vec::new();
    if !ghosts_on() {
        return out;
    }
    // the rows the frame's models moved, by their numbers now: their numbers before
    let mut rows: HashMap<[u32; 4], [f32; 4]> = HashMap::new();
    for (m, d) in now.iter().enumerate() {
        let (Some(bm), true) = (matched_to[m], tracks[m].moves) else { continue };
        let (Some(o), Some(ob)) = (d.shaded.as_deref(), before.get(bm).and_then(|d| d.shaded.as_deref())) else { continue };
        let mut bits = o.placing.0 | o.placing.1;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let in_row: Vec<&(u16, f32, bool)> = tracks[m].changed.iter().filter(|c| c.0 as usize / 4 == u).collect();
            if in_row.is_empty() || in_row.iter().any(|c| !c.2) {
                continue;
            }
            let (Some(was), Some(is)) = (row_of(&ob.shading, u), row_of(&o.shading, u)) else { continue };
            rows.entry(is).or_insert(was.map(f32::from_bits));
        }
    }
    for (n, d) in now.iter().enumerate() {
        if matched_to[n].is_some() || placed.get(n).copied().unwrap_or(true) {
            continue;
        }
        let Some(s) = d.shaded.as_deref().filter(|s| !s.loose) else { continue };
        let mut changed = Vec::new();
        let mut found = 0;
        let mut bits = s.placing.0 | s.placing.1;
        while bits != 0 {
            let u = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let Some(row) = row_of(&s.shading, u) else { continue };
            let Some(was) = rows.get(&row) else { continue };
            for c in 0..4 {
                if f32::from_bits(row[c]) != was[c] {
                    changed.push(((u * 4 + c) as u16, was[c], true));
                }
            }
            found += 1;
        }
        if found >= 3 && !changed.is_empty() {
            out.push((n, Track::moving(changed)));
        }
    }
    out
}

/// whether a draw's vertices (a sample of them) are all outside one side of
/// the view, or all behind the eye.
fn out_of_view(s: &crate::scene::Shaded) -> bool {
    let Some(points) = crate::wide2::clip_points(s, 256) else { return false };
    if points.is_empty() {
        return false;
    }
    let all = |f: &dyn Fn(&[f32; 4]) -> bool| points.iter().all(f);
    all(&|p| p[3] <= 1e-5) || all(&|p| p[0] > p[3]) || all(&|p| p[0] < -p[3]) || all(&|p| p[1] > p[3]) || all(&|p| p[1] < -p[3])
}

/// the ghosts put among `items` (a frame's draws, or their texture keys):
/// each before the item it is drawn before, in the ghosts' order.
fn interleave<T>(items: Vec<T>, ghosts: &[Ghost], mut item: impl FnMut(&Ghost) -> T) -> Vec<T> {
    let mut order: Vec<usize> = (0..ghosts.len()).collect();
    order.sort_by_key(|&g| ghosts[g].at); // stable: the ghosts' order within a place
    let mut out = Vec::with_capacity(items.len() + ghosts.len());
    let mut next = order.iter().peekable();
    for (n, it) in items.into_iter().enumerate() {
        while let Some(&&g) = next.peek().filter(|&&&g| ghosts[g].at <= n) {
            out.push(item(&ghosts[g]));
            next.next();
        }
        out.push(it);
    }
    for &g in next {
        out.push(item(&ghosts[g]));
    }
    out
}

/// the frame's draws at `t` with the ghosts among them.
pub(crate) fn with_ghosts(draws: Vec<Draw>, ghosts: &[Ghost], t: f32) -> Vec<Draw> {
    if ghosts.is_empty() {
        return draws;
    }
    interleave(draws, ghosts, |g| {
        let mut d = g.draw.clone();
        if let Some(s) = g.draw.shaded.as_deref() {
            d.shaded = Some(Arc::new(crate::hfr::shaded_at(s, &g.track.changed, t)));
        }
        d
    })
}

/// the frame's texture keys with the ghosts' among them, as with_ghosts
/// places their draws.
pub(crate) fn keys_with_ghosts<'a>(keys: &'a [[Option<u64>; 3]], ghosts: &[Ghost]) -> std::borrow::Cow<'a, [[Option<u64>; 3]]> {
    if ghosts.is_empty() {
        return std::borrow::Cow::Borrowed(keys);
    }
    std::borrow::Cow::Owned(interleave(keys.to_vec(), ghosts, |g| g.keys))
}
