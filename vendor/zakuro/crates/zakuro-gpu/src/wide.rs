//! wide: the top screen drawn wider than the console's 5:3.
//!
//! The game draws the top screen into a 400x240 buffer (kept rotated, 240
//! pixels a row and 400 rows, so a row is a column of the screen). With
//! widescreen on, the game's own cameras are made k times wider
//! horizontally (overrides/widescreen.c), and the GPU gives each buffer of
//! the top screen's size an image k times taller in rows, so the 400 guest
//! rows cover 400·k·scale image rows instead of 400·scale. Draws map onto
//! the whole image as they always did, so the wider scene lands in the
//! wider image unstretched, and what guest memory gets back is the same
//! picture squeezed to 400 rows, which is what the game expects to find
//! there. The display transfer carries the wide image to the framebuffer's
//! surface, which is wide too, and the presenter shows it at its real
//! width.
//!
//! A native renderer does the equivalent by making the top screen's colour
//! and depth targets (and the framebuffer it copies them to) `240·aspect`
//! pixels wide at the console's resolution, setting the viewport and
//! scissor of every draw into them through the same scale along x, and
//! resampling rows wherever such a target is copied into one of the normal
//! width (the bottom screen and the small copies the game makes of the top
//! screen).
//!
//! The factor is set once per process from the settings (`request`) and
//! taken by the hardware rasterizer when it makes its first surface; the
//! game's cameras read the one the GPU took (`effective_aspect`), so the
//! two always agree, and stay at the console's when the GPU cannot widen.

use std::sync::atomic::{AtomicU32, Ordering};

/// the console's top-screen aspect ratio, 400:240.
pub const NATIVE_ASPECT: f32 = 400.0 / 240.0;

/// the widest the top screen is drawn, 32:9.
pub const MOST_ASPECT: f32 = 32.0 / 9.0;

/// the aspect asked for, f32 bits, 0 for none.
static REQUESTED: AtomicU32 = AtomicU32::new(0);
/// the aspect the GPU draws at, f32 bits, 0 for the console's.
static EFFECTIVE: AtomicU32 = AtomicU32::new(0);

/// asks for the top screen at an aspect ratio, width over height, the
/// console's 5:3 or less for none. the GPU takes it when it makes its
/// surfaces.
pub fn request(aspect: f32) {
    let aspect = if aspect.is_finite() { aspect.clamp(NATIVE_ASPECT, MOST_ASPECT) } else { NATIVE_ASPECT };
    REQUESTED.store(aspect.to_bits(), Ordering::Relaxed);
}

/// the aspect asked for.
pub fn requested() -> f32 {
    match f32::from_bits(REQUESTED.load(Ordering::Relaxed)) {
        a if a > NATIVE_ASPECT => a,
        _ => NATIVE_ASPECT,
    }
}

/// the aspect the GPU draws the top screen at, the console's when it is
/// not widened.
pub fn effective_aspect() -> f32 {
    match f32::from_bits(EFFECTIVE.load(Ordering::Relaxed)) {
        a if a > NATIVE_ASPECT => a,
        _ => NATIVE_ASPECT,
    }
}

pub(crate) fn set_effective(aspect: f32) {
    EFFECTIVE.store(aspect.to_bits(), Ordering::Relaxed);
}

/// how many times wider than the console's the top screen is drawn.
pub fn factor() -> f32 {
    effective_aspect() / NATIVE_ASPECT
}

/// the effective aspect for the game's camera code, see
/// overrides/widescreen.c, which the program hands it as a hook.
pub extern "C" fn effective_aspect_c() -> f32 {
    effective_aspect()
}

/// wide-ui: what fills the wide sides of screens whose content is 5:3 by
/// nature (menus, the title, talk scenes over still pictures, telops):
/// 0 the game's own art framing the 5:3 (the default), 1 the layers
/// extended (wide2/wide3), 2 black. The frontend sets it from
/// `pc.wide_sides`; the native renderer reads it every frame.
static SIDES: AtomicU32 = AtomicU32::new(0);

/// wide-ui: sets what fills the wide sides (see SIDES).
pub fn set_sides(sides: u32) {
    SIDES.store(sides.min(2), Ordering::Relaxed);
}

/// wide-ui: what fills the wide sides (see SIDES).
pub fn sides() -> u32 {
    SIDES.load(Ordering::Relaxed)
}

/// native-final: the aspect the native renderer draws the top screen at,
/// taken from the one asked for at a render scale exactly as the hardware
/// rasterizer took it (whole image rows at the scale, so the game's cameras
/// see the same aspect). Says whether it changed.
pub fn take_native(scale: u32) -> bool {
    let rows = image_rows(scale, requested() / NATIVE_ASPECT);
    let aspect = aspect_of(scale, rows);
    let before = f32::from_bits(EFFECTIVE.load(Ordering::Relaxed));
    if before == aspect {
        return false;
    }
    set_effective(aspect);
    if rows != 400 * scale {
        log::info!("wide: the top screen is drawn at {aspect:.3}:1, {rows} rows for its 400");
    }
    true
}

/// whether a guest buffer is one of the top screen's, which are drawn wide:
/// 400 rows, 240 or 256 pixels each, as the game's colour and depth buffers
/// and the framebuffers display transfers leave are.
pub(crate) fn widens(width: u32, height: u32) -> bool {
    height == 400 && (width == 240 || width == 256)
}

/// the image rows of a top-screen buffer (400 guest rows) at a scale and
/// a widening.
pub(crate) fn image_rows(scale: u32, k: f32) -> u32 {
    if k <= 1.0 {
        return 400 * scale;
    }
    ((400.0 * scale as f32 * k).round() as u32).max(400 * scale)
}

/// the aspect the top screen has at an image height, for reporting what
/// was taken.
pub(crate) fn aspect_of(scale: u32, image_rows: u32) -> f32 {
    NATIVE_ASPECT * image_rows as f32 / (400 * scale) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_follow_the_aspect() {
        assert_eq!(image_rows(3, 1.0), 1200);
        let k = (3440.0 / 1440.0) / NATIVE_ASPECT;
        let rows = image_rows(3, k);
        assert!((aspect_of(3, rows) - 3440.0 / 1440.0).abs() < 0.005);
    }
}
