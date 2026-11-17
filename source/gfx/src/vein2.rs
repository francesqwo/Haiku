//! vein2: dev, how the pictures in between treat a frame's particle draws.
//!
//! `FATES_VEIN2_LOG=file`: for each upper-screen frame matched for pictures
//! in between (`hfr::track`), one line per particle draw (nw::eft, `loose`):
//!
//! ```text
//! F  frame-id game-frame draws particles
//! P  n match program entry moves lerped stays now-hash match-hash
//! ```
//!
//! `match` is the frame before's draw it was matched with (-1: none, new),
//! `lerped` / `stays` how many of its numbers move part of the way / are
//! drawn as they are now, the hashes those of its uniforms (to line a
//! windowed run up with a headless one of the same game frames).
//!
//! Nothing here runs unless the variable is set; a player never has it.

use std::io::Write;
use std::sync::Mutex;

use crate::hfr::Between;
use crate::prim::Draw;

static LOG: Mutex<Option<std::io::BufWriter<std::fs::File>>> = Mutex::new(None);

pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::perf::var("FATES_VEIN2_LOG").is_ok())
}

fn hash(d: &Draw) -> u64 {
    d.shaded.as_deref().map_or(0, |s| s.shading.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &w| (h ^ w as u64).wrapping_mul(0x100_0000_01b3)))
}

/// a frame's particle draws and their matches (track's, before the holds).
pub(crate) fn frame(before: &[Draw], now: &[Draw], between: &Between, matched_to: &[Option<usize>], own: &[u128]) {
    if !on() {
        return;
    }
    let Ok(path) = crate::perf::var("FATES_VEIN2_LOG") else { return };
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Ok(file) = std::fs::File::create(&path) else { return };
        *guard = Some(std::io::BufWriter::new(file));
    }
    let out = guard.as_mut().unwrap();
    let game = zakuro_core::native_gfx::HEADLESS_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let loose: Vec<usize> = (0..now.len()).filter(|&n| now[n].shaded.as_deref().is_some_and(|s| s.loose)).collect();
    if loose.is_empty() {
        return;
    }
    let _ = writeln!(out, "F\t{game}\t{}\t{}", now.len(), loose.len());
    for n in loose {
        let s = now[n].shaded.as_deref().unwrap();
        let t = &between.tracks[n];
        let lerped = t.changed.iter().filter(|c| c.2).count();
        let stays = t.changed.len() - lerped;
        let b = matched_to.get(n).copied().flatten();
        let _ = writeln!(
            out,
            "P\t{game}\t{n}\t{}\t{:x}\t{}\t{}\t{lerped}\t{stays}\t{:016x}\t{:016x}",
            b.map_or(-1, |b| b as i64),
            s.program.len() as u64 ^ (s.program.first().copied().unwrap_or(0) as u64) << 20,
            s.shading.get(401).copied().unwrap_or(0),
            t.moves as u8,
            hash(&now[n]),
            b.map_or(0, |b| hash(&before[b]))
        );
    }
    // FATES_VEIN2_ROWS=lo-hi: and each particle draw's placing rows (u:
    // x y z w), own rows marked *, in those game frames
    let rows = crate::perf::var("FATES_VEIN2_ROWS").ok().and_then(|v| {
        let (lo, hi) = v.split_once('-')?;
        Some((lo.trim().parse::<u64>().ok()?, hi.trim().parse::<u64>().ok()?))
    });
    if rows.is_some_and(|(lo, hi)| game >= lo && game <= hi) {
        for n in (0..now.len()).filter(|&n| now[n].shaded.as_deref().is_some_and(|s| s.loose)) {
            let s = now[n].shaded.as_deref().unwrap();
            let mut bits = s.placing.0 | s.placing.1;
            let mut line = format!("R	{game}	{n}	{:x}", s.shading.get(401).copied().unwrap_or(0));
            while bits != 0 {
                let u = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let v: Vec<String> = (0..4).map(|c| format!("{:.4}", f32::from_bits(s.shading[u * 4 + c]))).collect();
                line += &format!("	{u}{}:{}", if own.get(n).is_some_and(|o| o & (1 << u) != 0) { "*" } else { "" }, v.join(" "));
            }
            let _ = writeln!(out, "{line}");
        }
    }
    let _ = out.flush();
}
