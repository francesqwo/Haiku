//! cameras: the player's side of the battle free camera and of walking in My
//! Castle's third-person view (`overrides/cameras.c` is the game's side).
//!
//! - **Battle free camera** (`pc.battle_free_camera`, off by default; the
//!   Free Camera key, G or a controller's Share/Capture/Touchpad, flips it).
//!   While a battle scene is drawn:
//!   - **Fly** (the default, `pc.free_camera_orbit` off): the left stick,
//!     WASD or the arrows move the camera ahead, back and sideways along the
//!     view, the shoulders or triggers (the 3DS R and L: E and Q) up and
//!     down, the right stick or a left-button drag turn it, the wheel or
//!     Camera Zoom In/Out (`=`/`-`) move it ahead in steps, a right- or
//!     middle-button drag slides it. Those inputs do not reach the game
//!     meanwhile.
//!   - **Orbit**: the right stick or a left drag orbits the fight, the wheel
//!     or `=`/`-` zoom, a right or middle drag pans.
//!
//!   Camera Reset (H, R3) gives the game's camera back. The game's camera
//!   goes on underneath; the free one starts where it is when first moved.
//! - **Castle walking** (`pc.castle_walk`, on by default): in the view L
//!   opens in My Castle, the left stick or the D-pad walks Corrin, the
//!   right stick or a left-button drag turns the camera, the wheel zooms.
//! - **My Castle's free camera** (qol5): in My Castle the Free Camera key
//!   takes the camera out while Corrin walks around (the close-up
//!   third-person view L opens, and free roam on the castle map) and puts
//!   it back where the game's camera was. Same styles and controls as the battle
//!   one; Corrin stands meanwhile (the stick, D-pad and L/R are the
//!   camera's). It ends by itself when the walking around does (a
//!   building, a talk, a menu, a scene, a visit), and is not saved: each
//!   visit to My Castle starts on the game's camera.
//!
//! The game calls [`game_hook`] from its own thread once a frame for each of
//! the two, saying which is live; the frontend calls [`poll`] once a frame
//! with what it read. While a context is live and its feature on, the right
//! stick and the mouse over the top screen are the camera's: the bottom
//! screen's touch pointer gives them up ([`owns_right_stick`]).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::controls::PcAction;
use crate::input_api;

/// `HaikuCamFrame` in overrides/cameras.c.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct CamFrame {
    pub size: u32,
    pub context: u32,
    pub enabled: u32,
    pub reset: u32,
    pub orbit: [f32; 2],
    pub pan: [f32; 2],
    pub zoom: f32,
    pub look: [f32; 2],
    pub turn: [f32; 2],
    pub log: u32,
    pub mode: u32,
    pub mv: [f32; 3],
    pub dolly: f32,
}

const CAM_ORBIT: u32 = 0;
const CAM_FLY: u32 = 1;

const CAM_BATTLE: u32 = 1;
const CAM_CASTLE: u32 = 2;
/// qol5: My Castle, walking around: its free camera's input
const CAM_CASTLE_ROAM: u32 = 3;
/// qol5: My Castle, not walking around
const CAM_CASTLE_IDLE: u32 = 4;
/// castlecam: dev, the castle view's camera this tick (FATES_CASTLECAM_LOG)
const CAM_CASTLE_REPORT: u32 = 5;
const CAM_QUERY_BATTLE: u32 = 0x10;
const CAM_QUERY_CASTLE: u32 = 0x20;

/// how long a context counts as live after the game last said so.
const LIVE: Duration = Duration::from_millis(250);
/// qol5: how long without the game saying it is in My Castle before its
/// free camera is put away from this side.
const CASTLE_GONE: Duration = Duration::from_secs(3);
/// radians a second at full stick.
const STICK_ORBIT: f32 = 2.2;
/// zoom steps a second with a zoom button held.
const BUTTON_ZOOM: f32 = 6.0;
/// radians a pixel of drag.
const DRAG_ORBIT: f32 = 0.006;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    Orbit,
    Pan,
}

#[derive(Default)]
struct State {
    free_camera: bool,
    castle_walk: bool,
    log: bool,
    /// to the game, taken at its next frame
    orbit: [f32; 2],
    pan: [f32; 2],
    zoom: f32,
    turn: [f32; 2],
    reset: bool,
    look: [f32; 2],
    battle_seen: Option<(Instant, u64)>,
    castle_seen: Option<(Instant, u64)>,
    /// polls so far, the headless clock
    polls: u64,
    drag: Option<(Drag, (f32, f32))>,
    /// the window's height in pixels, for pans
    height: f32,
    /// the free camera key flipped the setting: the frontend saves it
    toggled: Option<bool>,
    last_poll: Option<Instant>,
    /// the headless clock: a frame a poll, not the wall's
    fixed_step: bool,
    /// FATES_CAMERAS_SCRIPT's right stick, held until it changes
    script_stick: [f32; 2],
    /// the free camera flies (else orbits)
    fly: bool,
    /// fly: seconds of full input right, up, ahead; wheel steps ahead
    mv: [f32; 3],
    dolly: f32,
    /// FATES_CAMERAS_SCRIPT's left stick and up/down, held
    script_move: [f32; 3],
    /// the castle view's zoom, wheel steps
    castle_zoom: f32,
    /// qol5: My Castle's free camera is out
    castle_free: bool,
    /// qol5: when the game last said Corrin walks around in My Castle, and
    /// when it was last in My Castle at all
    roam_seen: Option<(Instant, u64)>,
    in_castle_seen: Option<(Instant, u64)>,
    /// qol5: the key changed My Castle's free camera (Some(on)), or was
    /// pressed in My Castle where it cannot come out (Some(None))
    castle_toggled: Option<Option<bool>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(|| State { height: 720.0, log: std::env::var_os("FATES_CAMERAS_LOG").is_some(), ..State::default() }))
}

impl State {
    /// the Free Camera key: qol5: in My Castle it takes the castle's free
    /// camera out and back; elsewhere it switches the battle free camera
    fn free_camera_key(&mut self) {
        if self.live(self.roam_seen) {
            self.castle_free = !self.castle_free;
            self.castle_toggled = Some(Some(self.castle_free));
            log::info!("cameras: My Castle's free camera {}", if self.castle_free { "out" } else { "back" });
        } else if self.live(self.in_castle_seen) {
            self.castle_toggled = Some(None);
        } else {
            self.free_camera = !self.free_camera;
            self.toggled = Some(self.free_camera);
            log::info!("cameras: battle free camera {}", if self.free_camera { "on" } else { "off" });
        }
    }
    /// qol5: whether the game said so within `within` (headless: 4 frames
    /// a 1/15 s)
    fn seen_within(&self, seen: Option<(Instant, u64)>, within: Duration) -> bool {
        match seen {
            Some((_, poll)) if self.fixed_step => self.polls.saturating_sub(poll) <= (within.as_millis() as u64 / 16).max(15),
            Some((at, _)) => at.elapsed() < within,
            None => false,
        }
    }
    /// whether the game said so lately: within LIVE, or 15 frames headless
    fn live(&self, seen: Option<(Instant, u64)>) -> bool {
        match seen {
            Some((_, poll)) if self.fixed_step => self.polls.saturating_sub(poll) <= 15,
            Some((at, _)) => at.elapsed() < LIVE,
            None => false,
        }
    }
    fn battle_live(&self) -> bool {
        self.free_camera && self.live(self.battle_seen)
    }
    fn castle_live(&self) -> bool {
        self.castle_walk && self.live(self.castle_seen)
    }
    fn owns(&self) -> bool {
        self.free_live() || self.castle_live()
    }
    /// qol5: My Castle's free camera is out and Corrin walks around
    fn castle_free_live(&self) -> bool {
        self.castle_free && self.live(self.roam_seen)
    }
    /// qol5: a free camera (the battle's or My Castle's) takes the input
    fn free_live(&self) -> bool {
        self.battle_live() || self.castle_free_live()
    }
    fn fly_live(&self) -> bool {
        self.fly && self.free_live()
    }
}

/// the settings, whenever they change.
pub fn configure(pc: &crate::settings::PcSettings) {
    with(|state| {
        state.free_camera = pc.battle_free_camera;
        state.castle_walk = pc.castle_walk;
        state.fly = !pc.free_camera_orbit;
        if !state.free_camera {
            state.orbit = [0.0; 2];
            state.pan = [0.0; 2];
            state.zoom = 0.0;
        }
    });
}

/// headless runs: each poll is one game frame, not the wall clock's time.
pub fn fixed_step() {
    with(|state| state.fixed_step = true);
}

/// once a frame, after the input API has the frame's input: the stick and
/// the buttons.
pub fn poll(window_height: f32) {
    let snapshot = input_api::snapshot();
    let toggle = input_api::take_action(PcAction::FreeCamera);
    let reset = input_api::take_action(PcAction::CameraReset);
    let zoom_in = input_api::action_held(PcAction::CameraZoomIn);
    let zoom_out = input_api::action_held(PcAction::CameraZoomOut);
    with(|state| {
        let now = Instant::now();
        let dt = if state.fixed_step { 1.0 / 60.0 } else { state.last_poll.map_or(0.0, |at| (now - at).as_secs_f32()).min(0.1) };
        state.last_poll = Some(now);
        // castlecam: dev, FATES_CASTLECAM_FAKE_MOUSE=px/s turns the castle view as a mouse would
        if state.castle_live() && !state.castle_free_live() {
            state.turn[0] += crate::castlecam::fake_mouse(DRAG_ORBIT);
        }
        state.polls += 1;
        if window_height > 0.0 {
            state.height = window_height;
        }
        // qol5: in My Castle the key takes the castle's free camera out
        // and back; elsewhere it switches the battle free camera
        if toggle {
            state.free_camera_key();
        }
        // qol5: out of My Castle altogether for a while (the game, which
        // says when the walking around ends, is gone): put it away. A hitch
        // of the game (a capture, a load) is not that: CASTLE_GONE is long.
        if state.castle_free && !state.seen_within(state.in_castle_seen, CASTLE_GONE) {
            state.castle_free = false;
            log::info!("cameras: My Castle's free camera ends: not in My Castle");
        }
        let stick = if state.script_stick != [0.0; 2] { state.script_stick } else { snapshot.right_stick };
        if state.free_live() {
            state.orbit[0] += stick[0] * STICK_ORBIT * dt;
            state.orbit[1] += stick[1] * STICK_ORBIT * dt;
            let zoom = (zoom_in as i32 - zoom_out as i32) as f32 * BUTTON_ZOOM * dt;
            if state.fly {
                // the left stick and the D-pad move, R and L rise and sink
                use zakuro_core::services::hid::PadState;
                let held = |button: PadState| snapshot.buttons.contains(button) as i32 as f32;
                let x = (snapshot.circle[0] + held(PadState::RIGHT) - held(PadState::LEFT)).clamp(-1.0, 1.0);
                let z = (snapshot.circle[1] + held(PadState::UP) - held(PadState::DOWN)).clamp(-1.0, 1.0);
                let y = held(PadState::R) - held(PadState::L);
                let m = state.script_move;
                state.mv[0] += (x + m[0]).clamp(-1.0, 1.0) * dt;
                state.mv[1] += (y + m[1]).clamp(-1.0, 1.0) * dt;
                state.mv[2] += (z + m[2]).clamp(-1.0, 1.0) * dt;
                state.dolly += zoom;
            } else {
                state.zoom += zoom;
            }
            if reset {
                state.reset = true;
            }
        }
        state.look = if state.castle_live() && !state.castle_free_live() { stick } else { [0.0; 2] };
    });
}

/// the free camera key's change since the last call, for the settings file.
pub fn take_toggled() -> Option<bool> {
    with(|state| state.toggled.take())
}

/// qol5: My Castle's free camera came out (Some(true)) or went back
/// (Some(false)) at the key, or the key was pressed in My Castle where it
/// cannot come out (None), since the last call: for the toast.
pub fn take_castle_toggled() -> Option<Option<bool>> {
    with(|state| state.castle_toggled.take())
}

/// whether the right stick (and R3) is the camera's now, not the touch
/// pointer's.
pub fn owns_right_stick() -> bool {
    with(|state| state.owns())
}

/// a mouse button went down on the game, off the bottom screen: whether
/// the camera takes the drag.
pub fn mouse_down(drag: Drag, at: (f32, f32)) -> bool {
    with(|state| {
        let take = match drag {
            Drag::Orbit => state.owns(),
            Drag::Pan => state.free_live(),
        };
        if take {
            state.drag = Some((drag, at));
        }
        take
    })
}

pub fn mouse_up() {
    with(|state| state.drag = None);
}

/// whether a drag is going on, so the button's release is the camera's.
pub fn dragging() -> bool {
    with(|state| state.drag.is_some())
}

pub fn mouse_moved(at: (f32, f32)) {
    with(|state| {
        let Some((drag, from)) = state.drag else { return };
        let (dx, dy) = (at.0 - from.0, at.1 - from.1);
        state.drag = Some((drag, at));
        state.add_drag(drag, dx, dy);
    });
}

impl State {
    fn add_drag(&mut self, drag: Drag, dx: f32, dy: f32) {
        match drag {
            Drag::Orbit if self.free_live() => {
                // right turns right, up looks (or goes) up
                self.orbit[0] += dx * DRAG_ORBIT;
                self.orbit[1] -= dy * DRAG_ORBIT;
            }
            Drag::Orbit if self.castle_live() => self.turn[0] += dx * DRAG_ORBIT,
            Drag::Pan if self.free_live() => {
                let height = self.height.max(1.0);
                self.pan[0] += dx / height;
                self.pan[1] += dy / height;
            }
            _ => {}
        }
    }
}

/// the wheel turned, in lines (up positive): whether the camera took it.
pub fn wheel(lines: f32) -> bool {
    with(|state| {
        if state.fly_live() {
            state.dolly += lines;
            true
        } else if state.free_live() {
            state.zoom += lines;
            true
        } else if state.castle_live() {
            state.castle_zoom += lines;
            true
        } else {
            false
        }
    })
}

/// for headless scripts (input_script.rs): a drag of dx, dy pixels, or the
/// wheel, as the mouse would give them, the game's contexts permitting.
pub fn script(kind: &str, a: f32, b: f32) {
    with(|state| match kind {
        "drag" => state.add_drag(Drag::Orbit, a, b),
        "pan" => state.add_drag(Drag::Pan, a, b),
        "wheel" if state.fly_live() => state.dolly += a,
        "wheel" if state.free_live() => state.zoom += a,
        "wheel" if state.castle_live() => state.castle_zoom += a,
        _ => {}
    });
}

/// cameras: while the fly camera has them, the left stick, the D-pad and
/// R/L move it rather than reach the game. qol5: while My Castle's free
/// camera is out (either style) Corrin's walking gets none of them either.
pub fn mask_game_input(input: &mut zakuro_core::services::hid::InputState) {
    use zakuro_core::services::hid::PadState;
    if with(|state| state.fly_live() || state.castle_free_live()) {
        input.buttons &= !(PadState::UP | PadState::DOWN | PadState::LEFT | PadState::RIGHT | PadState::L | PadState::R);
        input.circle_x = 0.0;
        input.circle_y = 0.0;
    }
}

/// FATES_CAMERAS_SCRIPT: the free camera's input for headless runs, without
/// touching the game's own (a replay keeps driving it). One event a line,
/// `#` comments: `<frame> toggle | reset | drag dx dy | pan dx dy | wheel n
/// | stick x y` (the right stick, held until the next stick line).
pub struct HeadlessScript {
    events: Vec<(u64, String, f32, f32)>,
    next: usize,
}

impl HeadlessScript {
    pub fn open() -> Option<HeadlessScript> {
        let path = std::env::var_os("FATES_CAMERAS_SCRIPT")?;
        let text = std::fs::read_to_string(&path).inspect_err(|error| eprintln!("cameras script: {error}")).ok()?;
        let mut events = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (Some(frame), Some(kind)) = (fields.first().and_then(|f| f.parse::<u64>().ok()), fields.get(1)) else { continue };
            let number = |i: usize| fields.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            events.push((frame, kind.to_string(), number(2), number(3)));
        }
        events.sort_by_key(|event| event.0);
        println!("cameras script: {} events", events.len());
        fixed_step();
        Some(HeadlessScript { events, next: 0 })
    }

    /// the events up to `frame`; then a poll unless the input script did one.
    pub fn frame(&mut self, frame: u64, polled: bool) {
        while let Some((at, kind, a, b)) = self.events.get(self.next) {
            if *at > frame {
                break;
            }
            println!("cameras script frame {frame}: {kind} {a} {b}");
            if std::env::var_os("FATES_CAMERAS_MARKS").is_some() {
                zakuro_gpu::smooth::mark(&format!("cam {frame} {kind} {a} {b}")); // perf3: dev, the timing log's scenes
            }
            match kind.as_str() {
                "toggle" => with(|state| {
                    state.free_camera = !state.free_camera;
                    state.toggled = Some(state.free_camera);
                }),
                "reset" => with(|state| state.reset = true),
                // qol5: the Free Camera key, as pressed (My Castle's in My Castle)
                "key" => with(|state| state.free_camera_key()),
                "stick" => with(|state| state.script_stick = [*a, *b]),
                // fly: the left stick x, y (y ahead), held
                "move" => with(|state| state.script_move = [*a, state.script_move[1], *b]),
                // fly: up (+) or down (-), held
                "rise" => with(|state| state.script_move[1] = *a),
                "orbit" => with(|state| state.fly = false),
                "fly" => with(|state| state.fly = true),
                other => script(other, *a, *b),
            }
            self.next += 1;
        }
        if !polled {
            poll(0.0);
        }
    }
}

/// FATES_CAMERAS_SCRIPT in a windowed run (tests): the events by the game's
/// frame, on top of the window's own input, which the frontend polled.
pub fn windowed_script(frame: u64) {
    static SCRIPT: Mutex<Option<Option<HeadlessScript>>> = Mutex::new(None);
    let mut guard = SCRIPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = guard.get_or_insert_with(|| {
        let script = HeadlessScript::open();
        // a window keeps the wall clock
        with(|state| state.fixed_step = false);
        script
    });
    if let Some(script) = script {
        script.frame(frame, true);
    }
}

/// `void (*fates_cameras_hook)(HaikuCamFrame *)`, from the game's thread.
///
/// # Safety
/// `frame` points at a `HaikuCamFrame` the game owns for the call.
pub unsafe extern "C" fn game_hook(frame: *mut CamFrame) {
    let Some(frame) = (unsafe { frame.as_mut() }) else { return };
    if frame.size as usize != std::mem::size_of::<CamFrame>() {
        return;
    }
    // castlecam: dev, the castle view's camera (the frame's floats, in order)
    if frame.context == CAM_CASTLE_REPORT {
        let f = &*frame;
        let mut v = Vec::with_capacity(13);
        v.extend_from_slice(&f.orbit);
        v.extend_from_slice(&f.pan);
        v.push(f.zoom);
        v.extend_from_slice(&f.look);
        v.extend_from_slice(&f.turn);
        v.extend_from_slice(&f.mv);
        v.push(f.dolly);
        crate::castlecam::report_part(f.mode, &v);
        return;
    }
    with(|state| {
        frame.log = state.log as u32;
        match frame.context {
            CAM_QUERY_BATTLE => frame.enabled = state.free_camera as u32,
            CAM_QUERY_CASTLE => frame.enabled = state.castle_walk as u32,
            CAM_BATTLE => {
                frame.enabled = state.free_camera as u32;
                if !state.live(state.battle_seen) && state.log {
                    eprintln!("[cameras] a battle scene: the free camera {}", if state.free_camera { "is on" } else { "is off" });
                }
                state.battle_seen = Some((Instant::now(), state.polls));
                frame.mode = if state.fly { CAM_FLY } else { CAM_ORBIT };
                if state.free_camera {
                    frame.orbit = std::mem::take(&mut state.orbit);
                    frame.pan = std::mem::take(&mut state.pan);
                    frame.zoom = std::mem::take(&mut state.zoom);
                    frame.reset = std::mem::take(&mut state.reset) as u32;
                    frame.mv = std::mem::take(&mut state.mv);
                    frame.dolly = std::mem::take(&mut state.dolly);
                }
            }
            CAM_CASTLE_ROAM | CAM_CASTLE_IDLE => {
                let now = (Instant::now(), state.polls);
                if !state.live(state.in_castle_seen) && state.log {
                    eprintln!("[qol5] My Castle: the free camera key is the castle's");
                }
                state.in_castle_seen = Some(now);
                if frame.context == CAM_CASTLE_ROAM {
                    state.roam_seen = Some(now);
                } else if state.castle_free {
                    // the walking around is over (a building, a talk, a menu,
                    // a scene): the game eases back into its camera
                    state.castle_free = false;
                    log::info!("cameras: My Castle's free camera ends with the walking around");
                }
                frame.enabled = (state.castle_free && frame.context == CAM_CASTLE_ROAM) as u32;
                frame.mode = if state.fly { CAM_FLY } else { CAM_ORBIT };
                if frame.enabled != 0 {
                    frame.orbit = std::mem::take(&mut state.orbit);
                    frame.pan = std::mem::take(&mut state.pan);
                    frame.zoom = std::mem::take(&mut state.zoom);
                    frame.reset = std::mem::take(&mut state.reset) as u32;
                    frame.mv = std::mem::take(&mut state.mv);
                    frame.dolly = std::mem::take(&mut state.dolly);
                }
            }
            CAM_CASTLE => {
                frame.enabled = state.castle_walk as u32;
                if !state.live(state.castle_seen) && state.log {
                    eprintln!("[cameras] My Castle's view: walking {}", if state.castle_walk { "is on" } else { "is off" });
                }
                state.castle_seen = Some((Instant::now(), state.polls));
                if state.castle_walk {
                    frame.look = state.look;
                    frame.turn = std::mem::take(&mut state.turn);
                    frame.zoom = std::mem::take(&mut state.castle_zoom);
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_reaches_the_game_only_while_its_context_is_live() {
        let mut pc = crate::settings::PcSettings::default();
        assert!(!pc.battle_free_camera, "off by default");
        assert!(pc.castle_walk, "walking on by default");
        pc.battle_free_camera = true;
        configure(&pc);
        fixed_step();
        // no battle yet: a drag is not the camera's
        assert!(!mouse_down(Drag::Orbit, (0.0, 0.0)));
        let mut frame = CamFrame { size: std::mem::size_of::<CamFrame>() as u32, context: CAM_BATTLE, ..CamFrame::default() };
        unsafe { game_hook(&mut frame) };
        assert_eq!(frame.enabled, 1);
        assert!(owns_right_stick());
        assert!(mouse_down(Drag::Orbit, (0.0, 0.0)));
        mouse_moved((100.0, -50.0));
        mouse_up();
        assert!(wheel(2.0));
        let mut frame = CamFrame { size: std::mem::size_of::<CamFrame>() as u32, context: CAM_BATTLE, ..CamFrame::default() };
        unsafe { game_hook(&mut frame) };
        assert!((frame.orbit[0] - 0.6).abs() < 1e-4 && (frame.orbit[1] - 0.3).abs() < 1e-4, "{frame:?}");
        // the fly camera (the default style) takes the wheel as steps ahead
        assert_eq!(frame.mode, CAM_FLY);
        assert_eq!(frame.dolly, 2.0);
        // taken once
        let mut again = CamFrame { size: std::mem::size_of::<CamFrame>() as u32, context: CAM_BATTLE, ..CamFrame::default() };
        unsafe { game_hook(&mut again) };
        assert_eq!(again.orbit, [0.0; 2]);
        pc.battle_free_camera = false;
        configure(&pc);
        let mut off = CamFrame { size: std::mem::size_of::<CamFrame>() as u32, context: CAM_BATTLE, ..CamFrame::default() };
        unsafe { game_hook(&mut off) };
        assert_eq!(off.enabled, 0);
        assert!(!wheel(1.0));

        // qol5: My Castle's free camera. Walking around (roam), the key
        // takes it out (not the battle setting), masks Corrin's movement
        // and gets the fly input; it ends when the roaming stops
        let roam = |context| {
            let mut frame = CamFrame { size: std::mem::size_of::<CamFrame>() as u32, context, ..CamFrame::default() };
            unsafe { game_hook(&mut frame) };
            frame
        };
        assert_eq!(roam(CAM_CASTLE_ROAM).enabled, 0, "not out until the key");
        with(|state| state.free_camera_key());
        assert!(!with(|state| state.free_camera), "the battle setting stays as it was");
        assert_eq!(take_castle_toggled(), Some(Some(true)));
        assert_eq!(roam(CAM_CASTLE_ROAM).enabled, 1);
        assert!(owns_right_stick());
        let mut input = zakuro_core::services::hid::InputState { circle_x: 1.0, buttons: zakuro_core::services::hid::PadState::UP, ..Default::default() };
        mask_game_input(&mut input);
        assert_eq!((input.circle_x, input.buttons.bits()), (0.0, 0), "Corrin's walk is masked");
        assert!(wheel(1.0));
        assert_eq!(roam(CAM_CASTLE_ROAM).dolly, 1.0);
        // not walking around: the camera ends, and the key says so
        assert_eq!(roam(CAM_CASTLE_IDLE).enabled, 0);
        assert!(!with(|state| state.castle_free), "ends with the walking around");
        for _ in 0..20 {
            poll(0.0);
        }
        roam(CAM_CASTLE_IDLE);
        with(|state| state.free_camera_key());
        assert_eq!(take_castle_toggled(), Some(None));
        assert!(!with(|state| state.free_camera), "still not the battle setting in My Castle");
    }
}
