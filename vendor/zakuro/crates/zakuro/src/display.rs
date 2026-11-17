//! hfr: the PC display options. Window modes (windowed, borderless
//! fullscreen, exclusive fullscreen at a chosen resolution and refresh rate),
//! the window's size, the monitor, vsync and the resolution the 3D is drawn
//! at, put to use from the settings ([pc] in settings.toml) without a
//! restart, and the displays' modes for the options menu.

use std::sync::Mutex;

use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window};

use crate::settings::{PcSettings, WindowMode};

/// one way a display can be driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    /// the refresh rate in millihertz, 59940 for 59.94 Hz.
    pub refresh_mhz: u32,
    pub bit_depth: u16,
}

impl VideoMode {
    /// the refresh rate in whole hertz, as menus show it.
    pub fn refresh_hz(&self) -> u32 {
        (self.refresh_mhz + 500) / 1000
    }
}

/// a display, as the options menu lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayInfo {
    /// what `pc.monitor` holds to pick it.
    pub index: u32,
    pub name: String,
    /// its desktop resolution and refresh rate.
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    /// whether the window is on it now.
    pub current: bool,
    /// the modes exclusive fullscreen can switch it to, largest and fastest
    /// first, one per resolution and refresh rate.
    pub modes: Vec<VideoMode>,
}

impl DisplayInfo {
    /// the resolutions it can be switched to, largest first.
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut sizes: Vec<(u32, u32)> = self.modes.iter().map(|mode| (mode.width, mode.height)).collect();
        sizes.dedup();
        sizes
    }

    /// the refresh rates it offers at a resolution, in whole hertz, fastest
    /// first.
    pub fn refresh_rates(&self, width: u32, height: u32) -> Vec<u32> {
        let mut rates: Vec<u32> = self.modes.iter().filter(|mode| mode.width == width && mode.height == height).map(VideoMode::refresh_hz).collect();
        rates.dedup();
        rates
    }

    /// the frame rates worth offering on it: the usual ones up to its
    /// fastest refresh rate, and 0 for "match the display".
    pub fn frame_rates(&self) -> Vec<u32> {
        let fastest = self.modes.iter().map(VideoMode::refresh_hz).max().unwrap_or(self.refresh_mhz / 1000).max(60);
        let mut rates: Vec<u32> = [30, 60, 120, 144, 165, 240, 360].into_iter().filter(|&rate| rate <= fastest + 1).collect();
        rates.push(0);
        rates
    }
}

/// the displays as last seen, for the options menu, which runs without the
/// event loop at hand.
static DISPLAYS: Mutex<Vec<DisplayInfo>> = Mutex::new(Vec::new());

/// the displays and the modes each offers, as last looked at (when the
/// window opened, and whenever the display settings are put to use). The
/// "ui" agent's options menu reads this.
pub fn displays() -> Vec<DisplayInfo> {
    DISPLAYS.lock().map(|displays| displays.clone()).unwrap_or_default()
}

/// the displays in the order `pc.monitor` counts them: the primary one
/// first, so the default, 0, is the primary (as the options menu lists them,
/// see fates_ui/app.rs).
pub fn monitors(window: &Window) -> Vec<MonitorHandle> {
    let primary = window.primary_monitor();
    let mut list: Vec<_> = primary.iter().cloned().collect();
    list.extend(window.available_monitors().filter(|monitor| Some(monitor) != primary.as_ref()));
    list
}

/// looks at the displays again.
pub fn refresh_displays(window: &Window) -> Vec<DisplayInfo> {
    let current = window.current_monitor();
    let found: Vec<DisplayInfo> = monitors(window)
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let size = monitor.size();
            let mut modes: Vec<VideoMode> = monitor
                .video_modes()
                .map(|mode| VideoMode {
                    width: mode.size().width,
                    height: mode.size().height,
                    refresh_mhz: mode.refresh_rate_millihertz(),
                    bit_depth: mode.bit_depth(),
                })
                .collect();
            // the deepest color of each resolution and rate
            modes.sort_by(|a, b| (b.width * b.height, b.width, b.refresh_mhz, b.bit_depth).cmp(&(a.width * a.height, a.width, a.refresh_mhz, a.bit_depth)));
            modes.dedup_by(|a, b| a.width == b.width && a.height == b.height && a.refresh_mhz == b.refresh_mhz);
            DisplayInfo {
                index: index as u32,
                name: monitor.name().unwrap_or_else(|| format!("Display {}", index + 1)),
                width: size.width,
                height: size.height,
                refresh_mhz: monitor.refresh_rate_millihertz().unwrap_or(60_000),
                current: current.as_ref() == Some(&monitor),
                modes,
            }
        })
        .collect();
    if let Ok(mut displays) = DISPLAYS.lock() {
        displays.clone_from(&found);
    }
    found
}

/// the monitor `pc.monitor` names, else the window's, else the primary one.
fn monitor(window: &Window, index: u32) -> Option<MonitorHandle> {
    monitors(window).into_iter().nth(index as usize).or_else(|| window.current_monitor()).or_else(|| window.primary_monitor())
}

/// the video mode exclusive fullscreen switches to: the size asked for (0
/// for the display's own), at the refresh rate asked for (0 for the
/// display's own), else the nearest it has, in the deepest color.
fn video_mode(monitor: &MonitorHandle, pc: &PcSettings) -> Option<winit::monitor::VideoModeHandle> {
    let size = monitor.size();
    let width = if pc.fullscreen_width > 0 { pc.fullscreen_width } else { size.width };
    let height = if pc.fullscreen_height > 0 { pc.fullscreen_height } else { size.height };
    let refresh = if pc.refresh_rate > 0 { pc.refresh_rate * 1000 } else { monitor.refresh_rate_millihertz().unwrap_or(60_000) };
    let modes: Vec<_> = monitor.video_modes().collect();
    let sized: Vec<_> = modes.iter().filter(|mode| mode.size().width == width && mode.size().height == height).collect();
    let pool: Vec<_> = if sized.is_empty() {
        log::warn!("display: {width}x{height} is not a mode of {}, using its own resolution", monitor.name().unwrap_or_default());
        modes.iter().filter(|mode| mode.size() == size).collect()
    } else {
        sized
    };
    pool.into_iter()
        .min_by_key(|mode| ((mode.refresh_rate_millihertz() as i64 - refresh as i64).abs(), u16::MAX - mode.bit_depth()))
        .cloned()
}

/// what was last put to use, so that only a change does it again.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Applied {
    mode: Option<WindowMode>,
    monitor: u32,
    window: (u32, u32),
    fullscreen: (u32, u32, u32),
    vsync: Option<bool>,
}

impl super::App {
    /// puts the display settings to use: window mode, monitor, size and
    /// vsync, each only when it changed since the last time.
    pub(crate) fn display_apply(&mut self) {
        self.display_apply_with(false);
    }

    /// as display_apply, `again` doing the window's mode, size and monitor
    /// over even when they did not change (the options menu's Display
    /// effect, after the window was moved or resized by hand).
    pub(crate) fn display_apply_with(&mut self, again: bool) {
        let pc = self.settings.pc.clone();
        let Some(window) = &self.window else { return };
        refresh_displays(window);
        let applied = &mut self.hfr.display;
        if applied.vsync != Some(pc.vsync) {
            if let Some(backend) = &mut self.backend {
                backend.set_vsync(pc.vsync);
                log::info!("display: vsync {}", if pc.vsync { "on" } else { "off" });
            }
            applied.vsync = Some(pc.vsync);
        }
        let wanted = (Some(pc.window_mode), pc.monitor, (pc.window_width, pc.window_height), (pc.fullscreen_width, pc.fullscreen_height, pc.refresh_rate));
        if !again && (applied.mode, applied.monitor, applied.window, applied.fullscreen) == wanted {
            return;
        }
        let first = applied.mode.is_none();
        if first {
            for display in displays() {
                log::info!(
                    "display: {} {} {}x{} at {:.2} Hz, {} modes, at its own size {:?} Hz",
                    display.index,
                    display.name,
                    display.width,
                    display.height,
                    display.refresh_mhz as f64 / 1000.0,
                    display.modes.len(),
                    display.refresh_rates(display.width, display.height)
                );
            }
        }
        (applied.mode, applied.monitor, applied.window, applied.fullscreen) = wanted;
        let Some(target) = monitor(window, pc.monitor) else { return };
        match pc.window_mode {
            WindowMode::Windowed => {
                if window.fullscreen().is_some() {
                    window.set_fullscreen(None);
                }
                if pc.window_width > 0 && pc.window_height > 0 {
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(pc.window_width, pc.window_height));
                }
                // onto the display asked for, centred, when it is not there
                if window.current_monitor().as_ref() != Some(&target) || (first && pc.monitor != 0) {
                    let (area, size) = (target.size(), window.outer_size());
                    let position = target.position();
                    let x = position.x + (area.width as i32 - size.width as i32).max(0) / 2;
                    let y = position.y + (area.height as i32 - size.height as i32).max(0) / 2;
                    window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                }
                log::info!("display: windowed on {}", target.name().unwrap_or_default());
            }
            WindowMode::Borderless => {
                window.set_fullscreen(Some(Fullscreen::Borderless(Some(target.clone()))));
                log::info!("display: borderless fullscreen on {} at {}x{}", target.name().unwrap_or_default(), target.size().width, target.size().height);
            }
            WindowMode::Exclusive => match video_mode(&target, &pc) {
                Some(mode) => {
                    log::info!(
                        "display: exclusive fullscreen on {} at {}x{} {:.2} Hz, {}-bit",
                        target.name().unwrap_or_default(),
                        mode.size().width,
                        mode.size().height,
                        mode.refresh_rate_millihertz() as f64 / 1000.0,
                        mode.bit_depth()
                    );
                    window.set_fullscreen(Some(Fullscreen::Exclusive(mode)));
                }
                None => {
                    log::warn!("display: no video mode for exclusive fullscreen, borderless instead");
                    window.set_fullscreen(Some(Fullscreen::Borderless(Some(target.clone()))));
                }
            },
        }
        // the frame rate that follows the display follows the new one
        self.hfr.reset();
    }

    /// draws the 3D at the resolution the settings ask for, while a game
    /// runs, without a restart.
    pub(crate) fn display_apply_resolution(&mut self) {
        let wanted = self.active.resolution.max(1);
        let Some(game) = &mut self.game else { return };
        if !game.system.gpu.can_rescale() || game.system.config.resolution == wanted { // native-final: or the native renderer's
            return;
        }
        match game.system.set_resolution(wanted) {
            Ok(scale) => log::info!("display: the 3D is drawn at {scale}x from now on"),
            Err(error) => log::warn!("display: could not draw at {wanted}x, {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_offers_the_frame_rates_it_can_show() {
        let mode = |width, height, hz: u32| VideoMode { width, height, refresh_mhz: hz * 1000, bit_depth: 32 };
        let display = DisplayInfo {
            index: 0,
            name: "test".into(),
            width: 3440,
            height: 1440,
            refresh_mhz: 165_000,
            current: true,
            modes: vec![mode(3440, 1440, 165), mode(3440, 1440, 144), mode(3440, 1440, 60), mode(2560, 1080, 60)],
        };
        assert_eq!(display.frame_rates(), vec![30, 60, 120, 144, 165, 0]);
        assert_eq!(display.resolutions(), vec![(3440, 1440), (2560, 1080)]);
        assert_eq!(display.refresh_rates(3440, 1440), vec![165, 144, 60]);
    }
}
