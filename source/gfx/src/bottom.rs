//! gfx-bottom: frames the game draws without a clear, mostly the lower
//! screen.
//!
//! Both displays render into one colour buffer (one nn::gr FrameBuffer,
//! literal 0x00106564), so an uncleared frame starts from whatever was
//! rendered into that buffer last: in practice the other display's picture
//! of the same game frame. The game only leaves a display uncleared when its
//! first draws cover the whole screen (the battle map's lower screen: the
//! checkered background, then the forecast panels and portraits), so what
//! the frame starts from never shows.
//!
//! The native renderer used to require the display's previous native
//! picture (`chain_valid`) for such a frame. After a save-state load, a
//! scene change, or any frame the emulated path drew, there was none, and the
//! lower screen stayed emulated for good (the battle segments: 0 of 849
//! lower frames native). Now:
//!
//! - an uncleared frame without a native predecessor starts from a seed
//!   colour (black; `FATES_GFX_NOCLEAR_BLACK=0` restores the old
//!   behaviour, `FATES_GFX_NOCLEAR_FILL=r,g,b` picks another colour to
//!   prove the game covers it);
//! - an uncleared frame whose display list also holds islands the native
//!   path draws (the field ShadowScene on the lower display) chains from
//!   the previous native picture like any other uncleared frame;
//! - a frame meant to chain from a picture the renderer's target no longer
//!   holds (a hybrid composed over it, a scale change) starts from the seed
//!   instead of being dropped.
//!
//! `seeded`, `chained` and `reseeded` count those cases per display; they
//! are printed with the `gfx:` report line (FATES_GFX_REPORT).

/// counters, per display (0 upper, 1 lower).
#[derive(Default, Debug, Clone)]
pub struct Stats {
    /// uncleared frames without a native predecessor, started from the seed
    pub seeded: [u64; 2],
    /// uncleared frames with native islands, drawn natively in full over
    /// the previous native picture
    pub chained_islands: [u64; 2],
    /// frames meant to chain whose predecessor's picture was gone; started
    /// from the seed at render time
    pub reseeded: [u64; 2],
    /// uncleared frames with islands the native path does not draw, made
    /// hybrids instead of being left to the emulated path
    pub hybrid_seeded: [u64; 2],
    /// repeated DisplayBuffer::Transfer calls with nothing rendered in
    /// between, left out of the queue
    pub repeats: u64,
    /// queue entries whose transfer never ran, dropped
    pub stale: u64,
    /// transfers that ran with nothing queued
    pub unqueued: u64,
}

/// the colour an uncleared frame without a native predecessor starts from,
/// or None to leave such frames to the emulated path (the old behaviour,
/// FATES_GFX_NOCLEAR_BLACK=0).
pub fn seed() -> Option<[f32; 4]> {
    static SEED: std::sync::OnceLock<Option<[f32; 4]>> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        if std::env::var("FATES_GFX_NOCLEAR_BLACK").is_ok_and(|v| v == "0") {
            return None;
        }
        // dev: a loud colour shows any pixel the game leaves uncovered
        if let Ok(v) = std::env::var("FATES_GFX_NOCLEAR_FILL") {
            let c: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
            if c.len() >= 3 {
                let n = |x: f32| if x > 1.0 { x / 255.0 } else { x };
                return Some([n(c[0]), n(c[1]), n(c[2]), 1.0]);
            }
        }
        Some([0.0, 0.0, 0.0, 1.0])
    })
}

/// dev: FATES_GFX_BOTTOM_TRACE=from-to (gfx frames) logs each display
/// list's decision.
pub fn traced(frame: u64) -> bool {
    static RANGE: std::sync::OnceLock<Option<(u64, u64)>> = std::sync::OnceLock::new();
    RANGE
        .get_or_init(|| {
            let v = std::env::var("FATES_GFX_BOTTOM_TRACE").ok()?;
            let (a, b) = v.split_once('-').unwrap_or((&v, &v));
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .is_some_and(|(a, b)| frame >= a && frame <= b)
}

/// queues the frame a DisplayBuffer::Transfer call carries for its display
/// buffer, for `Gfx::transferred` to pop when that transfer runs.
///
/// Each call adds a transfer to the game's command list, but not every one
/// runs: in the battle segment the game calls Transfer three times for the
/// lower buffer in one frame (gfx frame 63 after loading f11000) and one
/// transfer runs. The two extra entries stayed at the queue's front, and
/// from then on every picture of that buffer was the one from two rounds
/// before (each third lower-screen picture six game frames old). So:
///
/// - a repeat (nothing rendered since this buffer's last entry, in the same
///   frame) is left out: it carries the same picture;
/// - entries from more than two gfx frames ago are dropped: the game reuses
///   a display buffer only after its transfer ran, so theirs never will.
pub(crate) fn queue(queue: &mut std::collections::VecDeque<(u64, Option<crate::Frame>)>, now: u64, frame: Option<crate::Frame>, stats: &mut Stats) {
    let before = queue.len();
    queue.retain(|(at, _)| at + 2 >= now);
    stats.stale += (before - queue.len()) as u64;
    if frame.is_none() && queue.back().is_some_and(|(at, _)| *at == now) {
        stats.repeats += 1;
        return;
    }
    queue.push_back((now, frame));
}

/// dev: FATES_GFX_BOTTOM_PEEK=virt,len,frame[,phys] syncs and reads guest
/// memory at the start of a gfx frame and writes it to peek-<virt>-<frame>.bin.
pub fn peek(system: &mut zakuro_core::System, frame: u64) {
    let Ok(v) = std::env::var("FATES_GFX_BOTTOM_PEEK") else { return };
    let p: Vec<u32> = v.split(',').filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()).collect();
    if p.len() < 3 || p[2] as u64 != frame {
        return;
    }
    let (addr, len) = (p[0], p[1]);
    let physical = zakuro_core::services::gsp::virtual_to_physical(system, addr);
    system.sync_gpu(addr, len);
    let mut bytes = vec![0u8; len as usize];
    system.memory.read_bytes(addr, &mut bytes);
    let distinct: std::collections::BTreeSet<u8> = bytes.iter().copied().collect();
    eprintln!("gfx-bottom peek {addr:08X} (physical {physical:08X}) frame {frame}: {} distinct byte values", distinct.len());
    let _ = std::fs::write(format!("peek-{addr:08X}-{frame}.bin"), &bytes);
}

/// UnitIconInstance::OnBegin (0x001F23D0): for each unit icon still to be
/// made, UnitIconObject::CaptureTexture points the framebuffer at the
/// icon's own RGBA4 texture, draws the sheet into it and puts the display's
/// framebuffer back. Nothing of it lands on the display, so a frame whose
/// other islands are native is drawn natively in full, the compacted list
/// keeping this callback for the emulated path to render the texture, which
/// the native draws then read (synced down by its virtual address).
/// FATES_GFX_OFFSCREEN_ISLANDS=0: such frames stay hybrids.
pub const UNIT_ICON_INSTANCE_ON_BEGIN: u32 = 0x001F_23D0;

pub fn offscreen(kind: u32) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    kind == UNIT_ICON_INSTANCE_ON_BEGIN && *ON.get_or_init(|| !std::env::var("FATES_GFX_OFFSCREEN_ISLANDS").is_ok_and(|v| v == "0"))
}
