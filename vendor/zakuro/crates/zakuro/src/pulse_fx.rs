//! pulse2: Divine Pulse's effects on the picture, worked out each present
//! from what the rewind did (crate::pulse tells this module) and drawn by
//! the presenter (zakuro-gpu's vulkan_pulse.rs) over the top screen. All of
//! it is presentation: the game is held or has already been put back, and
//! nothing here waits on anything.
//!
//! - **Opening** (about 0.65 s): a brief flash, then a ring of green light
//!   with a clock face's twelve marks spreads from the middle of the top
//!   screen, bending the picture as it passes, and behind it the world
//!   loses its colour toward the rewind screen's green: time stops, as
//!   Three Houses' Divine Pulse washes the screen.
//! - **Moving through the moments** (0.2 s): the picture before fades out
//!   over the moment shown, smeared a little the way time went (back: to
//!   the right, on: to the left). A new step starts a new fade from what
//!   is on screen at that instant, so holding a direction never waits.
//! - **Turning back time** (0.6 s): the old picture smears away to the
//!   right as the restored one comes through it, a ring closes in from the
//!   edges (the opening's wave run backwards) and the colour comes back.
//! - **Cancelling** (0.3 s): the present fades back in, the colour with it.

use std::sync::Mutex;
use std::time::Instant;

use zakuro_gpu::PulseEffect;

#[derive(Clone, Copy)]
enum Change {
    /// a moment shown: -1 back in time, 1 on
    Scrub(f32),
    Confirm,
    Cancel,
}

/// what the sound does (crate::audio's time_stop and time_resume).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cue {
    Stop,
    Resume,
}

struct Fx {
    opened: Option<Instant>,
    closed: Option<(Instant, bool)>,
    change: Option<(Instant, Change)>,
    serial: u32,
    cue: Option<Cue>,
}

static FX: Mutex<Fx> = Mutex::new(Fx { opened: None, closed: None, change: None, serial: 0, cue: None });

static DRAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// whether the presenter draws the effects (the rewind screen's own tint is
/// lighter then, the grade doing most of it).
pub fn set_drawn(drawn: bool) {
    DRAWN.store(drawn, std::sync::atomic::Ordering::Relaxed);
}

pub fn drawn() -> bool {
    DRAWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// the sound's cue since the last call: the app hands it to the output
/// before the frame's sound.
pub fn take_cue() -> Option<Cue> {
    with(|fx| fx.cue.take())
}

fn with<R>(f: impl FnOnce(&mut Fx) -> R) -> R {
    let mut guard = FX.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

const OPEN: f32 = 0.65;
const SCRUB: f32 = 0.2;
const CONFIRM: f32 = 0.6;
const CANCEL: f32 = 0.3;

/// the rewind screen opened.
pub fn opened() {
    with(|fx| {
        fx.opened = Some(now());
        fx.closed = None;
        fx.change = None;
        fx.cue = Some(Cue::Stop);
    });
}

/// a moment is about to be shown (the picture changes after this).
pub fn shown(back: bool) {
    with(|fx| {
        if fx.opened.is_some() {
            fx.serial = fx.serial.wrapping_add(1);
            fx.change = Some((now(), Change::Scrub(if back { -1.0 } else { 1.0 })));
        }
    });
}

/// time turned back (confirm) or the present put back (cancel).
pub fn closed(confirm: bool) {
    with(|fx| {
        if fx.opened.is_some() {
            let now = now();
            fx.serial = fx.serial.wrapping_add(1);
            fx.closed = Some((now, confirm));
            fx.change = Some((now, if confirm { Change::Confirm } else { Change::Cancel }));
            fx.cue = Some(Cue::Resume);
        }
    });
}

/// a saved state loaded, or the game gone: no effect.
pub fn reset() {
    with(|fx| {
        if fx.opened.is_some() && fx.closed.is_none() {
            fx.cue = Some(Cue::Resume);
        }
        fx.opened = None;
        fx.closed = None;
        fx.change = None;
    });
}

/// the effects' clock: the wall clock, or, in a lockstep capture
/// (FATES_LOCKSTEP, dev), a 60th of a second a present, so a capture that
/// takes its time still shows every step of the effects, in step with the
/// sound it records the same way (FATES_OUTPUT_WAV).
fn now() -> Instant {
    if !crate::trailer_dump::lockstep() {
        return Instant::now();
    }
    let (base, presents) = *LOCKSTEP.lock().unwrap_or_else(|e| e.into_inner());
    base.unwrap_or_else(Instant::now) + std::time::Duration::from_secs_f64(presents as f64 / 60.0)
}

static LOCKSTEP: Mutex<(Option<Instant>, u64)> = Mutex::new((None, 0));

/// a present went out (the lockstep clock's tick).
pub fn presented() {
    if crate::trailer_dump::lockstep() {
        let mut clock = LOCKSTEP.lock().unwrap_or_else(|e| e.into_inner());
        clock.0.get_or_insert_with(Instant::now);
        clock.1 += 1;
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// the effect for a present now, none when there is none.
pub fn effect() -> Option<PulseEffect> {
    effect_at(now())
}

fn effect_at(now: Instant) -> Option<PulseEffect> {
    with(|fx| {
        let opened = fx.opened?;
        let since = |at: Instant| now.saturating_duration_since(at).as_secs_f32();
        let t_open = since(opened);
        let mut e = PulseEffect { serial: fx.serial, time: t_open, ..PulseEffect::default() };
        // the colour going back once the screen closed
        let mut colour = 1.0;
        if let Some((at, confirm)) = fx.closed {
            let t = since(at);
            let length = if confirm { CONFIRM } else { CANCEL };
            if t > length + 0.05 {
                fx.opened = None;
                fx.closed = None;
                fx.change = None;
                return None;
            }
            colour = 1.0 - smooth(t / (length * 0.85));
        }
        // opening: the flash, the wave, the grade behind it; then the
        // grade breathing slowly while time is stopped
        let breath = 0.94 + 0.06 * (0.5 + 0.5 * (t_open * 1.6).sin());
        e.grade = ease_out(t_open / 0.5) * colour * breath;
        e.vignette = e.grade;
        if t_open < OPEN + 0.1 && fx.closed.is_none() {
            let k = (t_open / OPEN).min(1.0);
            e.ring = [ease_out(k) * 1.06, 0.05 + 0.07 * k, 0.95 * (1.0 - k).powf(1.3)];
            e.echo = 0.55;
            e.wave_grades = k < 1.0;
            e.grade = colour;
            e.flash = 0.7 * (1.0 - t_open / 0.14).max(0.0);
        }
        // the picture before, fading out over the new one
        if let Some((at, change)) = fx.change {
            let t = since(at);
            match change {
                Change::Scrub(direction) => {
                    // the old picture starts sharp and is swept away as it
                    // fades, so nothing jumps at the start
                    let k = t / SCRUB;
                    e.crossfade = 1.0 - smooth(k);
                    e.streak = -direction * 0.035 * smooth(k / 0.6);
                }
                Change::Confirm => {
                    let k = t / CONFIRM;
                    e.crossfade = 1.0 - smooth(k);
                    e.streak = 0.09 * smooth(k / 0.5);
                    let r = (t / (CONFIRM * 0.9)).min(1.0);
                    e.ring = [1.06 * (1.0 - ease_out(r)), 0.08, 0.75 * (1.0 - r)];
                    e.echo = 0.4;
                    e.wave_grades = false;
                    e.flash = 0.45 * (1.0 - t / 0.16).max(0.0);
                }
                Change::Cancel => {
                    e.crossfade = 1.0 - smooth(t / CANCEL);
                }
            }
            if e.crossfade <= 0.0 && !matches!(change, Change::Confirm) {
                fx.change = None;
            }
        }
        Some(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn opening_and_turning_back_end_with_no_effect() {
        reset();
        opened();
        let start = with(|fx| fx.opened.unwrap());
        let at = |s: f32| start + Duration::from_secs_f32(s);
        let first = effect_at(at(0.01)).unwrap();
        assert!(first.flash > 0.5 && first.ring[2] > 0.8 && first.wave_grades);
        let settled = effect_at(at(1.0)).unwrap();
        assert!(settled.ring[2] == 0.0 && settled.grade > 0.9);
        shown(true);
        let serial = with(|fx| fx.serial);
        let scrub = effect().unwrap();
        assert_eq!(scrub.serial, serial);
        assert!(scrub.crossfade > 0.9 && scrub.streak >= 0.0);
        closed(true);
        let closed_at = with(|fx| fx.closed.unwrap().0);
        assert!(effect_at(closed_at + Duration::from_secs_f32(0.1)).unwrap().crossfade > 0.5);
        assert!(effect_at(closed_at + Duration::from_secs_f32(2.0)).is_none());
        assert!(effect().is_none());
    }
}
