//! wide: the top screen's shape from the PC settings' aspect mode.
//!
//! The frontend asks the GPU for an aspect ratio when a game starts
//! (zakuro_gpu::wide::request); the GPU takes it with its first surface and
//! the game's cameras follow the one it took (overrides/widescreen.c). The
//! presenter then shows the top screen at that shape.

use crate::settings::AspectMode;
use zakuro_gpu::wide::{MOST_ASPECT, NATIVE_ASPECT};

/// the aspect the top screen is drawn at for a mode, width over height.
/// auto takes the window's shape, or the display's without a window, and
/// is never narrower than the console's nor wider than 32:9.
/// ZAKURO_WIDE_ASPECT ("16:9", "21:9", "32:9", "3440x1440", "2.39" or
/// "native") overrides it, for tests without a window.
pub fn aspect(mode: AspectMode, window: Option<(u32, u32)>, display: Option<(u32, u32)>) -> f32 {
    if let Some(aspect) = std::env::var("ZAKURO_WIDE_ASPECT").ok().and_then(|text| parse(&text)) {
        return aspect.clamp(NATIVE_ASPECT, MOST_ASPECT);
    }
    let ratio = |(w, h): (u32, u32)| (w > 0 && h > 0).then(|| w as f32 / h as f32);
    let aspect = match mode {
        AspectMode::Native => NATIVE_ASPECT,
        AspectMode::Wide16x9 => 16.0 / 9.0,
        // 21:9 as ultrawide monitors are, 3440x1440 and 2560x1080 alike
        AspectMode::Wide21x9 => 64.0 / 27.0,
        AspectMode::Wide32x9 => 32.0 / 9.0,
        AspectMode::Auto => window.and_then(ratio).or_else(|| display.and_then(ratio)).unwrap_or(NATIVE_ASPECT),
    };
    // wide-ui: a fixed shape within a few percent of the window's own is
    // meant to be the window's: "21:9" on a 3440x1440 display (43:18,
    // 2.389) or a 3840x1600 one (2.4) fills it, instead of leaving thin
    // strips at its sides
    let aspect = match (mode, window.and_then(ratio)) {
        (AspectMode::Wide16x9 | AspectMode::Wide21x9 | AspectMode::Wide32x9, Some(own)) if (aspect / own - 1.0).abs() <= SNAP => own,
        _ => aspect,
    };
    aspect.clamp(NATIVE_ASPECT, MOST_ASPECT)
}

/// wide-ui: how near (as a share) a fixed aspect has to be to the
/// window's to take the window's exactly.
const SNAP: f32 = 0.04;

/// wide-ui: tells the renderer what fills the wide sides of the 5:3
/// screens (pc.wide_sides).
pub fn sides(sides: crate::settings::WideSides) {
    zakuro_gpu::wide::set_sides(sides.code());
}

fn parse(text: &str) -> Option<f32> {
    let text = text.trim().to_ascii_lowercase();
    if text == "native" || text == "5:3" {
        return Some(NATIVE_ASPECT);
    }
    for separator in [':', 'x'] {
        if let Some((w, h)) = text.split_once(separator) {
            let (w, h): (f32, f32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
            return (h > 0.0).then(|| w / h);
        }
    }
    text.parse().ok()
}

/// asks the GPU for the top screen's shape, before the game draws.
pub fn request(mode: AspectMode, window: Option<(u32, u32)>, display: Option<(u32, u32)>) {
    let aspect = aspect(mode, window, display);
    log::info!("wide: asking for the top screen at {aspect:.3}:1 ({mode:?})");
    zakuro_gpu::wide::request(aspect);
    if let Ok(mut pending) = PENDING.lock() {
        *pending = Some((aspect, std::time::Instant::now()));
    }
}

/// the shape last seen and since when, see tick.
static PENDING: std::sync::Mutex<Option<(f32, std::time::Instant)>> = std::sync::Mutex::new(None);

/// how long the shape has to stay the same before the GPU takes it, so
/// that dragging a window's edge does not remake the top screen's buffers
/// every frame.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// follows the settings and the window while a game runs: a shape that
/// changed and then stayed for a moment is asked for, and the GPU takes it
/// between two frames (the game's cameras follow on their own).
pub fn tick(mode: AspectMode, window: Option<(u32, u32)>, display: Option<(u32, u32)>) {
    // a minimised window has no shape to follow
    if window.is_some_and(|(w, h)| w == 0 || h == 0) {
        return;
    }
    let aspect = aspect(mode, window, display);
    let Ok(mut pending) = PENDING.lock() else { return };
    match *pending {
        Some((seen, since)) if (seen - aspect).abs() < 0.001 => {
            let asked = zakuro_gpu::wide::requested();
            if since.elapsed() >= SETTLE && (asked - aspect).abs() >= 0.001 {
                log::info!("wide: the top screen's shape changed to {aspect:.3}:1 ({mode:?})");
                zakuro_gpu::wide::request(aspect);
            }
        }
        _ => *pending = Some((aspect, std::time::Instant::now())),
    }
}

/// the widest top screen Auto gives beside the docked panel, 21:9.
const PANEL_MOST_ASPECT: f32 = 64.0 / 27.0;

/// the room the window has for the top screen: all of it, or, with the
/// bottom screen docked beside it (the bottom screen's Panel mode), the
/// widest top screen up to 21:9 that still gets the window's full height
/// next to the panel at its size. auto follows this rather than the whole
/// window, so a docked panel and a wide top screen share the window.
pub fn room(window: Option<(u32, u32)>, settings: &crate::settings::Settings, composes: bool) -> Option<(u32, u32)> {
    use zakuro_gpu::backend::bottom_panel::{panel_top_aspect, PanelMode};
    let (w, h) = window?;
    if w == 0 || h == 0 || crate::bottom_screen::BottomScreen::mode(settings, composes) != Some(PanelMode::Panel) {
        return window;
    }
    // up to 21:9 beside the panel, see panel_top_aspect
    let aspect = panel_top_aspect((w, h), settings.pc.bottom_size, NATIVE_ASPECT, PANEL_MOST_ASPECT);
    Some(((aspect * h as f32).round() as u32, h))
}

/// the top screen's width at its height of 240 for the presenter: the
/// shape the GPU draws it at once it is wider than the console's, or the
/// one the settings give otherwise.
pub fn top_width(shape_width: f32) -> f32 {
    let aspect = zakuro_gpu::wide::effective_aspect();
    if aspect > NATIVE_ASPECT + 0.001 {
        240.0 * aspect
    } else {
        shape_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_give_their_shapes() {
        assert!((aspect(AspectMode::Native, Some((3440, 1440)), None) - NATIVE_ASPECT).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide16x9, None, None) - 16.0 / 9.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Auto, Some((3440, 1440)), None) - 3440.0 / 1440.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Auto, Some((800, 800)), None) - NATIVE_ASPECT).abs() < 1e-6);
        assert!((aspect(AspectMode::Auto, Some((7680, 1080)), None) - MOST_ASPECT).abs() < 1e-6);
        assert_eq!(parse("21:9"), Some(21.0 / 9.0));
        // wide-ui: a fixed shape near the window's takes the window's
        assert!((aspect(AspectMode::Wide21x9, Some((3440, 1440)), None) - 3440.0 / 1440.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide21x9, Some((2560, 1080)), None) - 2560.0 / 1080.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide21x9, Some((3840, 1600)), None) - 2.4).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide16x9, Some((1920, 1200)), None) - 16.0 / 9.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide21x9, Some((1920, 1080)), None) - 64.0 / 27.0).abs() < 1e-6);
        assert!((aspect(AspectMode::Wide32x9, Some((5120, 1440)), None) - 32.0 / 9.0).abs() < 1e-6);
        assert_eq!(parse("3440x1440"), Some(3440.0 / 1440.0));
    }
}
