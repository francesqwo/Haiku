//! input: the gilrs controller backend, kept only as the fallback for when
//! SDL3 cannot be loaded or initialised (see pads.rs).

use gilrs::{Axis, Button, EventType, Gilrs};

use crate::controls::{Family, PadButton, PadReading};

pub struct GilrsPads {
    gilrs: Gilrs,
}

impl GilrsPads {
    pub fn new() -> Option<GilrsPads> {
        Gilrs::new()
            .inspect_err(|error| log::warn!("input: gilrs could not open controllers either, {error}"))
            .ok()
            .map(|gilrs| GilrsPads { gilrs })
    }

    /// takes in what happened since the last call; returns the buttons
    /// newly pressed, each with its pad's family.
    pub fn poll(&mut self) -> Vec<(PadButton, Family)> {
        let mut pressed = Vec::new();
        while let Some(event) = self.gilrs.next_event() {
            let name = self.gilrs.gamepad(event.id).name().to_owned();
            let family = Family::from_name(&name).unwrap_or(Family::Generic);
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(button) = from_gilrs(button) {
                        pressed.push((button, family));
                    }
                }
                EventType::Connected => log::info!("input: controller connected (gilrs), {name}, family {family:?}"),
                EventType::Disconnected => log::info!("input: controller disconnected (gilrs), {name}"),
                _ => {}
            }
        }
        pressed
    }

    /// every connected pad's state, with its name and family.
    pub fn readings(&self) -> Vec<(String, Family, PadReading)> {
        self.gilrs
            .gamepads()
            .map(|(_, pad)| {
                let mut reading = PadReading::default();
                for button in [
                    Button::South,
                    Button::East,
                    Button::North,
                    Button::West,
                    Button::LeftTrigger,
                    Button::RightTrigger,
                    Button::LeftTrigger2,
                    Button::RightTrigger2,
                    Button::Select,
                    Button::Start,
                    Button::Mode,
                    Button::LeftThumb,
                    Button::RightThumb,
                    Button::DPadUp,
                    Button::DPadDown,
                    Button::DPadLeft,
                    Button::DPadRight,
                ] {
                    if let Some(ours) = from_gilrs(button) {
                        if pad.is_pressed(button) {
                            reading.set(ours, true);
                        }
                    }
                }
                // the d-pad of some controllers is a pair of axes
                for (button, held) in [
                    (PadButton::DPadRight, pad.value(Axis::DPadX) > 0.5),
                    (PadButton::DPadLeft, pad.value(Axis::DPadX) < -0.5),
                    (PadButton::DPadUp, pad.value(Axis::DPadY) > 0.5),
                    (PadButton::DPadDown, pad.value(Axis::DPadY) < -0.5),
                ] {
                    if held {
                        reading.set(button, true);
                    }
                }
                reading.left = [pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY)];
                reading.right = [pad.value(Axis::RightStickX), pad.value(Axis::RightStickY)];
                let name = pad.name().to_owned();
                let family = Family::from_name(&name).unwrap_or(Family::Generic);
                (name, family, reading)
            })
            .collect()
    }
}

/// gilrs names the shoulders "triggers" and the triggers "triggers 2".
fn from_gilrs(button: Button) -> Option<PadButton> {
    Some(match button {
        Button::South => PadButton::South,
        Button::East => PadButton::East,
        Button::North => PadButton::North,
        Button::West => PadButton::West,
        Button::LeftTrigger => PadButton::LeftShoulder,
        Button::RightTrigger => PadButton::RightShoulder,
        Button::LeftTrigger2 => PadButton::LeftTrigger,
        Button::RightTrigger2 => PadButton::RightTrigger,
        Button::Select => PadButton::Back,
        Button::Start => PadButton::Start,
        Button::Mode => PadButton::Guide,
        Button::LeftThumb => PadButton::LeftStick,
        Button::RightThumb => PadButton::RightStick,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        _ => return None,
    })
}
