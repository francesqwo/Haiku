//! bottom: where the bottom screen goes on a single PC display, and what the
//! presenter draws around the screens (a background and frames) when the
//! frontend composes the window itself rather than using a console layout.
//!
//! everything here is plain math on window pixels, so the frontend's touch
//! mapping and the presenter's drawing use the very same rectangles, and the
//! tests below check them without a GPU.

use super::{Overlay, Viewport};

/// the bottom screen's resolution.
pub const BOTTOM_SIZE: (f32, f32) = (320.0, 240.0);

/// how the bottom screen is shown next to a big top screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    /// framed, docked beside the top screen in space kept for it.
    Panel,
    /// framed, floating over the top screen where it was put, always
    /// shown ("Always shown").
    Overlay,
    /// framed, floating where it was put, slid in from the nearest window
    /// edge while wanted (Auto and Manual).
    OnDemand,
}

// bottom2: where a floating bottom screen goes, anchored to one of eight
// places around the window's edge, so a resized window keeps it sensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 8] = [Anchor::TopLeft, Anchor::Top, Anchor::TopRight, Anchor::Left, Anchor::Right, Anchor::BottomLeft, Anchor::Bottom, Anchor::BottomRight];

    /// 0 the left (top), 0.5 the middle, 1 the right (bottom).
    pub fn grid(self) -> (f32, f32) {
        match self {
            Anchor::TopLeft => (0.0, 0.0),
            Anchor::Top => (0.5, 0.0),
            Anchor::TopRight => (1.0, 0.0),
            Anchor::Left => (0.0, 0.5),
            Anchor::Right => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 1.0),
            Anchor::Bottom => (0.5, 1.0),
            Anchor::BottomRight => (1.0, 1.0),
        }
    }

    /// the anchor for a frame whose middle is at (gx, gy) of the window.
    pub fn from_grid(gx: f32, gy: f32) -> Anchor {
        let col = if gx < 1.0 / 3.0 { 0 } else if gx > 2.0 / 3.0 { 2 } else { 1 };
        let row = if gy < 1.0 / 3.0 { 0 } else if gy > 2.0 / 3.0 { 2 } else { 1 };
        match (col, row) {
            (0, 0) => Anchor::TopLeft,
            (1, 0) => Anchor::Top,
            (2, 0) => Anchor::TopRight,
            (0, 1) => Anchor::Left,
            (2, 1) => Anchor::Right,
            (0, 2) => Anchor::BottomLeft,
            (1, 2) => Anchor::Bottom,
            (2, 2) => Anchor::BottomRight,
            // the very middle goes with the nearer of top and bottom
            _ if gy < 0.5 => Anchor::Top,
            _ => Anchor::Bottom,
        }
    }
}

/// a floating bottom screen's place and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub anchor: Anchor,
    /// the frame's distance from the anchor's edges, in window heights: in
    /// from the left or right edge, down or up from the top or bottom, or
    /// right and down of the middle (either sign) where it is centred.
    pub dx: f32,
    pub dy: f32,
    /// the bottom screen's height (inside the frame), in window heights.
    pub height: f32,
}

/// the smallest and largest floating bottom screen, in window heights.
pub const FLOAT_LEAST: f32 = 0.15;
pub const FLOAT_MOST: f32 = 0.9;

impl Default for Placement {
    fn default() -> Self {
        // the right edge's middle: the top screen's corners hold Fates'
        // forecast, terrain and unit windows
        Placement { anchor: Anchor::Right, dx: 0.02, dy: 0.0, height: 0.33 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

/// what the layout is worked out from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelParams {
    pub mode: PanelMode,
    pub side: PanelSide,
    /// the bottom screen's height relative to the top screen's, 0.3 to 1.
    pub size: f32,
    /// the top screen's width over its height, from the picture's real size.
    pub top_aspect: f32,
    /// the top screen scaled by whole multiples of the console's 240 lines.
    pub integer: bool,
    /// bottom2: where the floating modes (Overlay, OnDemand) put it.
    pub place: Placement,
}

/// the frame's thickness in art pixels (Dialog.png's border and the first
/// row of its checker corners), which is scaled with the bottom screen.
pub const FRAME_INSET: f32 = 12.0;

/// the top screen never gives up more than this share of its size to make
/// room for the docked panel; past that the panel gets smaller instead.
pub const MIN_TOP_SHARE: f32 = 0.75;

/// where everything goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelPlaces {
    pub top: Viewport,
    /// the bottom screen's pixels, which may lie partly outside the window
    /// while it slides.
    pub bottom: Viewport,
    /// the frame's outer edge around the bottom screen.
    pub frame: Viewport,
    /// window pixels per art pixel for the frame and the cursor, a whole
    /// number so the art stays crisp.
    pub art_scale: f32,
}

impl PanelPlaces {
    /// a mix of two layouts, for sliding between them, t from 0 (self) to 1.
    pub fn lerp(&self, other: &PanelPlaces, t: f32) -> PanelPlaces {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        let view = |a: Viewport, b: Viewport| Viewport {
            x: mix(a.x, b.x),
            y: mix(a.y, b.y),
            width: mix(a.width, b.width),
            height: mix(a.height, b.height),
        };
        PanelPlaces {
            top: view(self.top, other.top),
            bottom: view(self.bottom, other.bottom),
            frame: view(self.frame, other.frame),
            art_scale: if t < 0.5 { self.art_scale } else { other.art_scale },
        }
    }
}

/// the top screen alone, as large as the window allows, centred.
fn top_alone(window: (f32, f32), aspect: f32, integer: bool) -> Viewport {
    let (w, h) = window;
    let mut height = h.min(w / aspect);
    if integer && height >= 240.0 {
        height = (height / 240.0).floor() * 240.0;
    }
    let width = height * aspect;
    Viewport { x: ((w - width) / 2.0).floor(), y: ((h - height) / 2.0).floor(), width, height }
}

/// wide: the shape the top screen takes in Panel mode when the aspect is
/// Auto: the widest, from least up to most, that keeps the window's full
/// height with the panel docked beside it at the size asked for. The panel
/// size is the trade: 0.4 (the default) leaves a 3440x1440 window a ~1.76:1
/// top beside an ~816 px panel, while a 16:9 display has no room to widen
/// and keeps 5:3, panel_layout then shrinking the top to fit the panel.
pub fn panel_top_aspect(window: (u32, u32), size: f32, least: f32, most: f32) -> f32 {
    let (w, h) = (window.0.max(1) as f32, window.1.max(1) as f32);
    let m = margin((w, h));
    let size = if size.is_finite() { size.clamp(0.3, 1.0) } else { 0.4 };
    // the panel at the full height's share, framed, as panel_layout makes it
    let screen_h = (size * h).max(BOTTOM_SIZE.1 / 2.0).floor();
    let (border, _) = frame_around(screen_h);
    let frame_w = screen_h * BOTTOM_SIZE.0 / BOTTOM_SIZE.1 + 2.0 * border;
    ((w - 3.0 * m - frame_w) / h).clamp(least, most)
}

/// the frame around a bottom screen of this height, and its art scale.
fn frame_around(screen_height: f32) -> (f32, f32) {
    let scale = (screen_height / BOTTOM_SIZE.1).round().max(1.0);
    (FRAME_INSET * scale, scale)
}

fn framed(x: f32, y: f32, height: f32) -> (Viewport, Viewport, f32) {
    let width = height * BOTTOM_SIZE.0 / BOTTOM_SIZE.1;
    let (border, scale) = frame_around(height);
    let bottom = Viewport { x: x + border, y: y + border, width, height };
    let frame = Viewport { x, y, width: width + 2.0 * border, height: height + 2.0 * border };
    (bottom, frame, scale)
}

/// the gap between things and the window's edge.
fn margin(window: (f32, f32)) -> f32 {
    (window.1 * 0.02).round().max(4.0)
}

/// where the screens go with the bottom one shown.
pub fn panel_layout(window: (u32, u32), params: PanelParams) -> PanelPlaces {
    let window = (window.0.max(1) as f32, window.1.max(1) as f32);
    let (w, h) = window;
    let size = params.size.clamp(0.3, 1.0);
    let aspect = if params.top_aspect.is_finite() && params.top_aspect > 0.1 { params.top_aspect } else { 5.0 / 3.0 };
    let m = margin(window);
    // the frame adds FRAME_INSET art pixels a side, about that many 240ths
    // of the screen's height
    let frame_share = 2.0 * FRAME_INSET / BOTTOM_SIZE.1;
    let panel_width_per_height = BOTTOM_SIZE.0 / BOTTOM_SIZE.1 + frame_share;

    match params.mode {
        PanelMode::Panel => {
            let alone = top_alone(window, aspect, false);
            // the top screen as big as it can be with the panel beside it
            let fit = (w - 2.0 * m) / (aspect + size * panel_width_per_height);
            let mut top_h = alone.height.min(fit).max(alone.height * MIN_TOP_SHARE).min(h);
            if params.integer && top_h >= 240.0 {
                top_h = (top_h / 240.0).floor() * 240.0;
            }
            let top_w = top_h * aspect;
            // the panel gets what is left, up to the size asked for
            let room_w = (w - top_w - 2.0 * m).max(0.0);
            let room_h = h - 2.0 * m;
            let mut screen_h = (size * top_h).min(room_w / panel_width_per_height).min(room_h / (1.0 + frame_share));
            // the frame is a whole number of art pixels, which may not fit
            let (border, _) = frame_around(screen_h);
            screen_h = screen_h.min((room_w - 2.0 * border) * 0.75).min(room_h - 2.0 * border);
            // too small to read: at least half the console's size, over
            // the top screen if it must
            screen_h = screen_h.max(BOTTOM_SIZE.1 / 2.0).floor();
            let (border, _) = frame_around(screen_h);
            let frame_w = screen_h * 4.0 / 3.0 + 2.0 * border;
            let frame_h = screen_h + 2.0 * border;
            let total = top_w + m + frame_w;
            let x0 = ((w - total) / 2.0).max(0.0).floor();
            let (top_x, frame_x) = match params.side {
                PanelSide::Right => (x0, (x0 + top_w + m).min(w - frame_w)),
                PanelSide::Left => ((x0 + frame_w + m).min(w - top_w).max(0.0), x0),
            };
            let top = Viewport { x: top_x.floor(), y: ((h - top_h) / 2.0).floor(), width: top_w, height: top_h };
            let (bottom, frame, art_scale) = framed(frame_x.floor(), ((h - frame_h) / 2.0).floor(), screen_h);
            PanelPlaces { top, bottom, frame, art_scale }
        }
        // bottom2: floating where it was put, over the whole-window top
        PanelMode::Overlay | PanelMode::OnDemand => {
            let top = top_alone(window, aspect, params.integer);
            let (bottom, frame, art_scale) = float_frame((w, h), params.place);
            PanelPlaces { top, bottom, frame, art_scale }
        }
    }
}

/// bottom2: the frame's size for a bottom screen this many pixels high.
pub fn frame_size(screen_h: f32) -> (f32, f32) {
    let (border, _) = frame_around(screen_h);
    (screen_h * BOTTOM_SIZE.0 / BOTTOM_SIZE.1 + 2.0 * border, screen_h + 2.0 * border)
}

/// bottom2: the bottom screen's height in pixels for a placement, as big as
/// asked but never bigger than the window, nor smaller than half the console
/// where the window has room for that.
fn float_height(window: (f32, f32), height: f32) -> f32 {
    let (w, h) = window;
    let asked = if height.is_finite() { height.clamp(FLOAT_LEAST, FLOAT_MOST) } else { Placement::default().height };
    let mut screen_h = (asked * h).max(BOTTOM_SIZE.1 / 2.0);
    // the frame has to fit the window
    for _ in 0..4 {
        let (fw, fh) = frame_size(screen_h);
        if fw <= w && fh <= h {
            break;
        }
        let (border, _) = frame_around(screen_h);
        screen_h = screen_h.min(h - 2.0 * border).min((w - 2.0 * border) * BOTTOM_SIZE.1 / BOTTOM_SIZE.0).floor();
    }
    screen_h.max(16.0).floor()
}

/// bottom2: where a floating frame goes, its top left kept inside the window.
fn float_origin(window: (f32, f32), place: Placement, frame: (f32, f32)) -> (f32, f32) {
    let (w, h) = window;
    let (gx, gy) = place.anchor.grid();
    let along = |g: f32, room: f32, d: f32| {
        let d = if d.is_finite() { d * h } else { 0.0 };
        let at = match g {
            g if g < 0.25 => d,
            g if g > 0.75 => room - d,
            _ => room / 2.0 + d,
        };
        at.round().clamp(0.0, room.max(0.0).floor())
    };
    (along(gx, w - frame.0, place.dx), along(gy, h - frame.1, place.dy))
}

/// bottom2: a floating bottom screen and its frame, from its placement.
pub fn float_frame(window: (f32, f32), place: Placement) -> (Viewport, Viewport, f32) {
    let screen_h = float_height(window, place.height);
    let size = frame_size(screen_h);
    let (x, y) = float_origin(window, place, size);
    framed(x, y, screen_h)
}

/// bottom2: the placement that puts a frame where it is, anchored to the
/// part of the window its middle is in.
pub fn placement_of(window: (u32, u32), frame: Viewport, screen_height: f32) -> Placement {
    let (w, h) = (window.0.max(1) as f32, window.1.max(1) as f32);
    let (room_x, room_y) = (w - frame.width, h - frame.height);
    let (cx, cy) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    let anchor = Anchor::from_grid(cx / w, cy / h);
    let (gx, gy) = anchor.grid();
    let away = |g: f32, at: f32, room: f32| match g {
        g if g < 0.25 => at / h,
        g if g > 0.75 => (room - at) / h,
        _ => (at - room / 2.0) / h,
    };
    Placement { anchor, dx: away(gx, frame.x, room_x), dy: away(gy, frame.y, room_y), height: (screen_height / h).clamp(FLOAT_LEAST, FLOAT_MOST) }
}

/// bottom2: how near a dragged frame has to come to be pulled to an edge.
pub fn snap_reach(window: (u32, u32)) -> f32 {
    (window.1 as f32 * 0.025).max(10.0)
}

/// bottom2: a frame moved to (x, y), held inside the window and pulled to
/// the window's edges (at the margin), its corners and middle lines when
/// near them.
pub fn snap(window: (u32, u32), frame: Viewport, x: f32, y: f32) -> (f32, f32) {
    let (w, h) = (window.0.max(1) as f32, window.1.max(1) as f32);
    let m = margin((w, h));
    let reach = snap_reach(window);
    let pull = |at: f32, size: f32, room: f32| {
        let free = (room - size).max(0.0);
        let low = m.min(free);
        let high = (free - m).max(low);
        let mid = (free / 2.0).floor();
        let at = at.clamp(0.0, free);
        [low, high, mid].into_iter().filter(|t| (at - t).abs() <= reach).min_by(|a, b| (at - a).abs().total_cmp(&(at - b).abs())).unwrap_or(at).round()
    };
    (pull(x, frame.width, w), pull(y, frame.height, h))
}

/// bottom2: the placement after dragging corner `corner` (0 top left, 1 top
/// right, 2 bottom left, 3 bottom right) of a frame to (x, y): the opposite
/// corner stays put and the screen keeps 4:3.
pub fn resize(window: (u32, u32), frame: Viewport, screen_height: f32, corner: u8, x: f32, y: f32) -> Placement {
    let (w, h) = (window.0.max(1) as f32, window.1.max(1) as f32);
    let (border, _) = frame_around(screen_height);
    let right = corner & 1 == 1;
    let lower = corner & 2 == 2;
    let fixed_x = if right { frame.x } else { frame.x + frame.width };
    let fixed_y = if lower { frame.y } else { frame.y + frame.height };
    let room_x = if right { w - fixed_x } else { fixed_x };
    let room_y = if lower { h - fixed_y } else { fixed_y };
    let want_w = (x - fixed_x).abs().min(room_x);
    let want_h = (y - fixed_y).abs().min(room_y);
    let by_w = (want_w - 2.0 * border) * BOTTOM_SIZE.1 / BOTTOM_SIZE.0;
    let by_h = want_h - 2.0 * border;
    let most = ((room_x - 2.0 * border) * BOTTOM_SIZE.1 / BOTTOM_SIZE.0).min(room_y - 2.0 * border).min(FLOAT_MOST * h);
    let least = (FLOAT_LEAST * h).max(BOTTOM_SIZE.1 / 2.0).min(most).max(16.0);
    let mut screen_h = by_w.max(by_h).clamp(least, most.max(least)).floor();
    // a coarser frame at the new size must still fit
    while screen_h > least && {
        let (fw, fh) = frame_size(screen_h);
        fw > room_x || fh > room_y
    } {
        screen_h -= 1.0;
    }
    let (fw, fh) = frame_size(screen_h);
    let fx = if right { fixed_x } else { fixed_x - fw };
    let fy = if lower { fixed_y } else { fixed_y - fh };
    placement_of(window, Viewport { x: fx, y: fy, width: fw, height: fh }, screen_h)
}

/// bottom2: how far a floating frame moves to slide out of the window past
/// its nearest edge.
pub fn slide_out(window: (f32, f32), frame: Viewport) -> (f32, f32) {
    let (w, h) = window;
    let gap = 8.0;
    let left = frame.x;
    let right = w - frame.x - frame.width;
    let top = frame.y;
    let bottom = h - frame.y - frame.height;
    let nearest = left.min(right).min(top).min(bottom);
    if nearest == right {
        (w + gap - frame.x, 0.0)
    } else if nearest == left {
        (-(frame.x + frame.width + gap), 0.0)
    } else if nearest == bottom {
        (0.0, h + gap - frame.y)
    } else {
        (0.0, -(frame.y + frame.height + gap))
    }
}

/// where the screens go with the bottom one put away: the top screen alone,
/// the panel slid out past the window's edge on its side (bottom2: a
/// floating one past the window edge nearest it).
pub fn hidden_layout(window: (u32, u32), params: PanelParams) -> PanelPlaces {
    let shown = panel_layout(window, params);
    let w = window.0.max(1) as f32;
    let top = match params.mode {
        // the docked panel's room goes back to the top screen
        PanelMode::Panel => top_alone((w, window.1.max(1) as f32), params.top_aspect.max(0.1), params.integer),
        _ => shown.top,
    };
    let (shift, drop) = match (params.mode, params.side) {
        // bottom2: a floating one slides out past its nearest edge
        (PanelMode::Overlay | PanelMode::OnDemand, _) => slide_out((w, window.1.max(1) as f32), shown.frame),
        (_, PanelSide::Right) => (w + 8.0 - shown.frame.x, 0.0),
        (_, PanelSide::Left) => (-(shown.frame.x + shown.frame.width + 8.0), 0.0),
    };
    let moved = |v: Viewport| Viewport { x: v.x + shift, y: v.y + drop, ..v };
    PanelPlaces { top, bottom: moved(shown.bottom), frame: moved(shown.frame), art_scale: shown.art_scale }
}

/// the layout part of the way between hidden (0) and shown (1), eased.
pub fn revealed_layout(window: (u32, u32), params: PanelParams, reveal: f32) -> PanelPlaces {
    let t = reveal.clamp(0.0, 1.0);
    let shown = panel_layout(window, params);
    if t >= 1.0 {
        return shown;
    }
    let hidden = hidden_layout(window, params);
    // ease out, quick to start and settling gently
    let eased = 1.0 - (1.0 - t).powi(3);
    hidden.lerp(&shown, eased)
}

/// the bottom-screen pixel a window position touches, none outside it or
/// outside the window's visible part.
pub fn touch_point(bottom: Viewport, window: (u32, u32), x: f32, y: f32) -> Option<(u16, u16)> {
    if x < 0.0 || y < 0.0 || x >= window.0 as f32 || y >= window.1 as f32 {
        return None;
    }
    let (u, v) = ((x - bottom.x) / bottom.width, (y - bottom.y) / bottom.height);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    Some((((u * BOTTOM_SIZE.0) as u16).min(319), ((v * BOTTOM_SIZE.1) as u16).min(239)))
}

/// the window position of a bottom-screen pixel's centre.
pub fn window_point(bottom: Viewport, px: f32, py: f32) -> (f32, f32) {
    (bottom.x + (px + 0.5) / BOTTOM_SIZE.0 * bottom.width, bottom.y + (py + 0.5) / BOTTOM_SIZE.1 * bottom.height)
}

/// the window as the frontend composed it: where the screens go (none for
/// the presenter's own layout) and what is drawn under and over them.
#[derive(Default)]
pub struct Composition {
    /// the top and bottom screens' places, instead of the arrangement's.
    pub screens: Option<(Option<Viewport>, Option<Viewport>)>,
    /// how opaque the bottom screen is, 0 to 1.
    pub bottom_opacity: f32,
    /// drawn first, behind the screens: the wallpaper.
    pub under: Overlay,
    /// drawn over the screens but under the interface: frames, the cursor.
    pub over: Overlay,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(mode: PanelMode, side: PanelSide, aspect: f32) -> PanelParams {
        PanelParams { mode, side, size: 0.6, top_aspect: aspect, integer: false, place: Placement::default() }
    }

    fn inside(inner: Viewport, outer: Viewport) -> bool {
        inner.x >= outer.x - 0.01
            && inner.y >= outer.y - 0.01
            && inner.x + inner.width <= outer.x + outer.width + 0.01
            && inner.y + inner.height <= outer.y + outer.height + 0.01
    }

    fn overlaps(a: Viewport, b: Viewport) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    const SIZES: [(u32, u32); 5] = [(1280, 720), (1920, 1080), (2560, 1440), (3440, 1440), (3840, 2160)];
    const ASPECTS: [f32; 4] = [5.0 / 3.0, 16.0 / 9.0, 2.0, 43.0 / 18.0];

    /// docked, the panel never covers the top screen, both stay in the
    /// window, the bottom screen keeps 4:3 and the top its own shape.
    #[test]
    fn the_panel_docks_beside_the_top_screen() {
        for &(w, h) in &SIZES {
            for &aspect in &ASPECTS {
                for side in [PanelSide::Left, PanelSide::Right] {
                    let places = panel_layout((w, h), params(PanelMode::Panel, side, aspect));
                    let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
                    assert!(inside(places.top, window), "{w}x{h} {aspect} {side:?} {places:?}");
                    assert!(inside(places.frame, window), "{w}x{h} {aspect} {side:?} {places:?}");
                    assert!(inside(places.bottom, places.frame));
                    assert!((places.bottom.width / places.bottom.height - 4.0 / 3.0).abs() < 0.01);
                    assert!((places.top.width / places.top.height - aspect).abs() < 0.01);
                    let alone = top_alone((w as f32, h as f32), aspect, false);
                    assert!(places.top.height >= alone.height * MIN_TOP_SHARE - 1.0);
                    if places.bottom.height > BOTTOM_SIZE.1 / 2.0 {
                        assert!(!overlaps(places.top, places.frame), "{w}x{h} {aspect} {side:?} {places:?}");
                    }
                    match side {
                        PanelSide::Right => assert!(places.frame.x > places.top.x),
                        PanelSide::Left => assert!(places.frame.x < places.top.x),
                    }
                }
            }
        }
    }

    /// on the 3440x1440 ultrawide the 5:3 top screen stays nearly full
    /// height and the panel fills the spare width at the size asked for.
    #[test]
    fn ultrawide_keeps_the_top_screen_big() {
        let places = panel_layout((3440, 1440), params(PanelMode::Panel, PanelSide::Right, 5.0 / 3.0));
        assert!(places.top.height >= 1300.0, "{places:?}");
        assert!(places.bottom.height >= 0.55 * places.top.height, "{places:?}");
        // 16:9 docks beside a smaller top screen, no smaller than the floor
        let places = panel_layout((1920, 1080), params(PanelMode::Panel, PanelSide::Right, 5.0 / 3.0));
        assert!(places.top.height >= 1080.0 * MIN_TOP_SHARE - 1.0, "{places:?}");
        assert!(places.bottom.height >= 240.0, "{places:?}");
    }

    /// bottom2: a floating bottom screen leaves the top screen the whole
    /// window and sits at the right edge's middle by default.
    #[test]
    fn the_floating_screen_leaves_the_top_screen_whole() {
        for &(w, h) in &SIZES {
            for mode in [PanelMode::Overlay, PanelMode::OnDemand] {
                let places = panel_layout((w, h), params(mode, PanelSide::Right, 5.0 / 3.0));
                let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
                assert!(inside(places.frame, window), "{w}x{h} {places:?}");
                let alone = top_alone((w as f32, h as f32), 5.0 / 3.0, false);
                assert_eq!(places.top, alone);
                let m = 0.02 * h as f32;
                assert!((places.frame.x + places.frame.width - (w as f32 - m)).abs() <= 1.0, "{w}x{h} {places:?}");
                assert!((places.frame.y + places.frame.height / 2.0 - h as f32 / 2.0).abs() <= 1.0, "{w}x{h} {places:?}");
            }
        }
    }

    const ODD_SIZES: [(u32, u32); 4] = [(800, 480), (1366, 768), (2560, 1080), (1080, 1920)];
    const HEIGHTS: [f32; 6] = [0.1, 0.15, 0.25, 0.33, 0.5, 0.9];
    const OFFSETS: [f32; 6] = [-0.3, 0.0, 0.02, 0.1, 0.3, 0.8];

    /// bottom2: at any place and size the window keeps the frame whole and
    /// the screen 4:3, touches go round to the same pixel, and the place
    /// read back from where it is gives the same place.
    #[test]
    fn floating_anywhere_stays_inside_and_maps_touches() {
        for &(w, h) in SIZES.iter().chain(&ODD_SIZES) {
            let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
            for anchor in Anchor::ALL {
                for height in HEIGHTS {
                    for dx in OFFSETS {
                        for dy in OFFSETS {
                            let place = Placement { anchor, dx, dy, height };
                            let (bottom, frame, _) = float_frame((w as f32, h as f32), place);
                            let at = format!("{w}x{h} {place:?} {frame:?}");
                            assert!(inside(frame, window), "{at}");
                            assert!(inside(bottom, frame), "{at}");
                            assert!((bottom.width / bottom.height - 4.0 / 3.0).abs() < 0.01, "{at}");
                            assert!(bottom.height >= (BOTTOM_SIZE.1 / 2.0).min(h as f32 / 3.0), "{at}");
                            for py in (0..240).step_by(17).chain([239]) {
                                for px in (0..320).step_by(23).chain([319]) {
                                    let (x, y) = window_point(bottom, px as f32, py as f32);
                                    assert_eq!(touch_point(bottom, (w, h), x, y), Some((px, py)), "{at}");
                                }
                            }
                            // the frame's border touches nothing
                            assert_eq!(touch_point(bottom, (w, h), frame.x + 1.0, frame.y + 1.0), None);
                            // read back, the same place
                            let back = placement_of((w, h), frame, bottom.height);
                            let (_, again, _) = float_frame((w as f32, h as f32), back);
                            assert!((again.x - frame.x).abs() <= 1.0 && (again.y - frame.y).abs() <= 1.0, "{at} came back as {back:?} {again:?}");
                            assert!((again.height - frame.height).abs() <= 1.0, "{at} {again:?}");
                        }
                    }
                }
            }
        }
    }

    /// bottom2: the anchor read from a place keeps it near the same corner
    /// or edge when the window changes size.
    #[test]
    fn a_resized_window_keeps_the_corner() {
        let place = Placement { anchor: Anchor::BottomRight, dx: 0.02, dy: 0.02, height: 0.3 };
        for &(w, h) in SIZES.iter().chain(&ODD_SIZES) {
            let (_, frame, _) = float_frame((w as f32, h as f32), place);
            let gap_right = w as f32 - frame.x - frame.width;
            let gap_bottom = h as f32 - frame.y - frame.height;
            assert!(gap_right <= 0.02 * h as f32 + 1.0 && gap_bottom <= 0.02 * h as f32 + 1.0, "{w}x{h} {frame:?}");
        }
        // dragged to the top middle, it keeps to the top, centred
        let (w, h) = (1920u32, 1080u32);
        let (_, frame, _) = float_frame((1920.0, 1080.0), place);
        let x = ((w as f32 - frame.width) / 2.0).round();
        let back = placement_of((w, h), Viewport { x, y: 20.0, ..frame }, frame.height - 24.0);
        assert_eq!(back.anchor, Anchor::Top);
        let (_, wider, _) = float_frame((3440.0, 1440.0), back);
        assert!(((wider.x + wider.width / 2.0) - 1720.0).abs() <= 2.0, "{wider:?}");
    }

    /// bottom2: dragged near an edge, a corner or the middle line, the frame
    /// is pulled to it; elsewhere it stays where it was put; it never leaves
    /// the window.
    #[test]
    fn dragging_snaps_to_the_edges() {
        let window = (1920u32, 1080u32);
        let m = margin((1920.0, 1080.0));
        let frame = Viewport { x: 500.0, y: 300.0, width: 400.0, height: 300.0 };
        assert_eq!(snap(window, frame, 5.0, 1080.0 - 300.0 - 10.0), (m, 1080.0 - 300.0 - m));
        assert_eq!(snap(window, frame, -200.0, -50.0), (m, m));
        assert_eq!(snap(window, frame, 1920.0, 2000.0), (1920.0 - 400.0 - m, 1080.0 - 300.0 - m));
        assert_eq!(snap(window, frame, 700.0, 480.0), (700.0, 480.0));
        // the middle
        let mid = ((1920.0f32 - 400.0) / 2.0).floor();
        assert_eq!(snap(window, frame, mid + 9.0, 480.0).0, mid);
        // flush to the edge is let be past the margin? no: within reach it is
        // the margin, and the window's edge itself is held
        let reach = snap_reach(window);
        assert!(reach >= 10.0);
        let (x, _) = snap(window, frame, m + reach + 5.0, 480.0);
        assert_eq!(x, m + reach + 5.0);
    }

    /// bottom2: a corner pulled out sizes the screen, keeping 4:3 and the
    /// opposite corner, inside the window and within the sizes allowed.
    #[test]
    fn a_corner_sizes_it() {
        for &(w, h) in SIZES.iter().chain(&ODD_SIZES) {
            let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
            let place = Placement { anchor: Anchor::Left, dx: 0.1, dy: 0.0, height: 0.3 };
            let (bottom, frame, _) = float_frame((w as f32, h as f32), place);
            for corner in 0..4u8 {
                for (px, py) in [(-5000.0, -5000.0), (0.0, 0.0), (w as f32 / 2.0, h as f32 / 2.0), (5000.0, 5000.0), (frame.x + 10.0, frame.y + 10.0)] {
                    let sized = resize((w, h), frame, bottom.height, corner, px, py);
                    let (b, f, _) = float_frame((w as f32, h as f32), sized);
                    let at = format!("{w}x{h} corner {corner} to ({px}, {py}) {sized:?} {f:?}");
                    assert!(inside(f, window), "{at}");
                    assert!((b.width / b.height - 4.0 / 3.0).abs() < 0.01, "{at}");
                    assert!(sized.height >= FLOAT_LEAST - 1e-4 && sized.height <= FLOAT_MOST + 1e-4, "{at}");
                    // the opposite corner stays, unless the window pushed it
                    let (ox, oy) = (if corner & 1 == 1 { f.x - frame.x } else { f.x + f.width - frame.x - frame.width }, if corner & 2 == 2 { f.y - frame.y } else { f.y + f.height - frame.y - frame.height });
                    let pushed = f.x <= 0.5 || f.y <= 0.5 || f.x + f.width >= w as f32 - 0.5 || f.y + f.height >= h as f32 - 0.5;
                    assert!(pushed || (ox.abs() <= 2.0 && oy.abs() <= 2.0), "{at}");
                }
            }
        }
    }

    /// bottom2: Auto slides it out past the edge nearest it, and hidden it
    /// is off the window and touches nothing.
    #[test]
    fn it_slides_out_past_the_nearest_edge() {
        let (w, h) = (1920u32, 1080u32);
        for (anchor, dx, dy, expect) in [
            (Anchor::BottomRight, 0.02, 0.3, "right"),
            (Anchor::BottomRight, 0.5, 0.02, "down"),
            (Anchor::TopLeft, 0.02, 0.2, "left"),
            (Anchor::Top, 0.0, 0.02, "up"),
            (Anchor::Left, 0.02, 0.0, "left"),
        ] {
            let p = PanelParams { place: Placement { anchor, dx, dy, height: 0.3 }, ..params(PanelMode::OnDemand, PanelSide::Right, 5.0 / 3.0) };
            let shown = panel_layout((w, h), p);
            let hidden = hidden_layout((w, h), p);
            let way = match (hidden.frame.x - shown.frame.x, hidden.frame.y - shown.frame.y) {
                (x, _) if x > 0.0 => "right",
                (x, _) if x < 0.0 => "left",
                (_, y) if y > 0.0 => "down",
                _ => "up",
            };
            assert_eq!(way, expect, "{anchor:?} {shown:?} {hidden:?}");
            let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
            assert!(!overlaps(hidden.frame, window));
            let (cx, cy) = window_point(hidden.bottom, 160.0, 120.0);
            assert_eq!(touch_point(hidden.bottom, (w, h), cx, cy), None);
            // half way, what shows still maps
            let half = revealed_layout((w, h), p, 0.5);
            for (px, py) in [(0.0, 0.0), (160.0, 120.0), (319.0, 239.0)] {
                let (x, y) = window_point(half.bottom, px, py);
                let seen = x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32;
                assert_eq!(touch_point(half.bottom, (w, h), x, y).is_some(), seen);
            }
        }
    }

    #[test]
    fn on_demand_slides_out_of_the_window() {
        for &(w, h) in &SIZES {
            for side in [PanelSide::Left, PanelSide::Right] {
                for mode in [PanelMode::OnDemand, PanelMode::Panel, PanelMode::Overlay] {
                    let p = params(mode, side, 5.0 / 3.0);
                    let hidden = revealed_layout((w, h), p, 0.0);
                    let window = Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
                    assert!(!overlaps(hidden.frame, window), "{w}x{h} {side:?} {mode:?} {hidden:?}");
                    assert_eq!(revealed_layout((w, h), p, 1.0), panel_layout((w, h), p));
                    // nothing to touch while put away
                    let centre = window_point(hidden.bottom, 160.0, 120.0);
                    assert_eq!(touch_point(hidden.bottom, (w, h), centre.0, centre.1), None);
                }
            }
        }
    }

    /// every bottom-screen pixel's centre, put in window coordinates for
    /// every layout, maps back to the same pixel.
    #[test]
    fn touches_land_on_the_pixel_under_the_pointer() {
        for &(w, h) in &SIZES {
            for &aspect in &ASPECTS {
                for mode in [PanelMode::Panel, PanelMode::Overlay, PanelMode::OnDemand] {
                    for side in [PanelSide::Left, PanelSide::Right] {
                        for reveal in [0.5, 1.0] {
                            let places = revealed_layout((w, h), params(mode, side, aspect), reveal);
                            for py in (0..240).step_by(7).chain([239]) {
                                for px in (0..320).step_by(9).chain([319]) {
                                    let (x, y) = window_point(places.bottom, px as f32, py as f32);
                                    let visible = x >= 0.0 && x < w as f32 && y >= 0.0 && y < h as f32;
                                    let got = touch_point(places.bottom, (w, h), x, y);
                                    if visible {
                                        assert_eq!(got, Some((px, py)), "{w}x{h} {mode:?} {side:?} {reveal} {places:?}");
                                    } else {
                                        assert_eq!(got, None);
                                    }
                                }
                            }
                            // just outside the screen touches nothing
                            let b = places.bottom;
                            assert_eq!(touch_point(b, (w, h), b.x - 1.0, b.y + 5.0), None);
                            assert_eq!(touch_point(b, (w, h), b.x + 5.0, b.y + b.height + 0.5), None);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn auto_beside_the_panel_widens_on_ultrawide_only() {
        let (least, most) = (5.0 / 3.0, 64.0 / 27.0);
        let a = panel_top_aspect((3440, 1440), 0.4, least, most);
        assert!(a > 1.72 && a < 1.8, "3440x1440 gives {a}");
        // the top keeps the full height there with the panel beside it
        let params = PanelParams { mode: PanelMode::Panel, side: PanelSide::Right, size: 0.4, top_aspect: a, integer: false, place: Placement::default() };
        let places = panel_layout((3440, 1440), params);
        assert!(places.top.height >= 1439.0, "{:?}", places.top);
        assert!(places.frame.x + places.frame.width <= 3440.0);
        assert_eq!(panel_top_aspect((1920, 1080), 0.4, least, most), least);
        assert_eq!(panel_top_aspect((2560, 1440), 0.4, least, most), least);
        assert_eq!(panel_top_aspect((5120, 1440), 0.4, least, most), most);
    }

}
