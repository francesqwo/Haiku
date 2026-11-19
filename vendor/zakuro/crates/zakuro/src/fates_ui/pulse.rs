//! pulse: Divine Pulse's rewind screen (crate::pulse keeps the moments),
//! drawn with the game's own art and font over the stopped game:
//!
//! - the game tinted the green of Sothis's time stopping, the tint breathing
//!   slowly while the screen is up;
//! - the game's ornate bar along the top of the top screen (its ends as
//!   they are, only the band between them longer at wider shapes), with
//!   "Divine Pulse", the moment's turn and phase, and the charges left;
//! - the timeline along the bottom in the game's dark prompt pill: one
//!   pennant a moment (blue your phase, red the enemy's, green another
//!   force's), the turn written where a phase starts, the name of who acts
//!   next under each, "Now" last; the hand points at the one chosen;
//! - the game's own Yes/No dialog to confirm.
//!
//! Left and right (keys, D-pad, stick) move through the moments, L and R
//! (Q and E) to the start of the phase before or after; A (Enter) asks to
//! turn back time there, B (Esc) puts everything back as it was. The mouse
//! clicks a moment, clicks it again to choose it, scrolls with the wheel,
//! and right-clicks to go back.

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_GOLD};
use super::{draw_prompts, prompts_width, Nav, Prompt, Prompts};
use crate::pulse::{self, phase_name};

const CELL: f32 = 46.0;
const STRIP_H: f32 = 44.0;

fn tag_colour(force: u8) -> u8 {
    match force {
        0 => 1, // blue
        1 => 0, // red
        _ => 4, // green
    }
}

/// the moment's line: "Turn 3 · Player Phase".
fn moment_line(view: &pulse::View, index: usize) -> String {
    match view.moments.get(index) {
        Some((turn, phase, _, _)) => format!("Turn {turn} \u{b7} {} Phase", phase_name(*phase)),
        None => format!("Now \u{b7} Turn {} \u{b7} {} Phase", view.turn_now.0, phase_name(view.turn_now.1)),
    }
}

fn charges_text(view: &pulse::View) -> String {
    match (view.left, view.limit) {
        (Some(left), Some(limit)) => format!("Charges {left}/{limit}"),
        _ => "Charges \u{221e}".to_owned(),
    }
}

/// where things are, in the top screen's units.
struct Layout {
    width: f32,
    strip_x: f32,
    strip_y: f32,
    strip_w: f32,
    first: usize,
    shown: usize,
}

fn layout(view: &pulse::View, width: f32) -> Layout {
    let strip_x = 14.0;
    let strip_w = width - 28.0;
    let strip_y = 240.0 - STRIP_H - 26.0;
    let room = ((strip_w - 28.0) / CELL).floor().max(1.0) as usize;
    let count = view.moments.len() + 1;
    let shown = room.min(count);
    // the chosen one in view, as far right as it can be
    let first = (view.selected + 2).saturating_sub(shown).min(count - shown);
    Layout { width, strip_x, strip_y, strip_w, first, shown }
}

fn cell_x(l: &Layout, slot: usize) -> f32 {
    let used = l.shown as f32 * CELL;
    l.strip_x + (l.strip_w - used) / 2.0 + slot as f32 * CELL
}

fn prompt_entries(view: &pulse::View) -> Vec<(Prompt, &'static str)> {
    if view.asking.is_some() {
        return vec![(Prompt::Confirm, "Select"), (Prompt::Back, "Back")];
    }
    vec![
        (Prompt::PrevTab, ""),
        (Prompt::NextTab, "Phase"),
        (Prompt::Confirm, "Turn Back"),
        (Prompt::Back, if view.game_over { "Accept Defeat" } else { "Cancel" }),
    ]
}

/// the screen for a frame: input, then drawing. `top` is the top screen in
/// points (the whole window when unknown).
pub fn frame(base: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, screen: egui::Rect, top: Option<egui::Rect>) {
    let Some(view) = pulse::view() else { return };
    let top = top.filter(|t| t.height() > 1.0).unwrap_or(screen);
    let unit = top.height() / 240.0;
    let canvas = Canvas { origin: top.min, unit, ..base.clone() };
    let width = top.width() / unit;
    let l = layout(&view, width);
    let now = input.time;

    // ---- input -------------------------------------------------------------
    let count = view.moments.len();
    let pointer = input.pointer.hover_pos().map(|p| canvas.unit_of(p));
    let clicked = input.pointer.primary_clicked();
    if let Some(yes) = view.asking {
        let mut yes = yes;
        let rows = dialog_rows(&canvas, &view, width);
        let mut answer = None;
        if nav.up || nav.down || nav.left || nav.right {
            yes = !yes;
        }
        if nav.back || input.pointer.secondary_clicked() {
            answer = Some(false);
        } else if nav.confirm {
            answer = Some(yes);
        } else if let Some((px, py)) = pointer {
            for (index, (x, y, w)) in rows.iter().enumerate() {
                if (*x..x + w).contains(&px) && (*y..y + 19.0).contains(&py) {
                    if input.pointer.delta() != egui::Vec2::ZERO {
                        yes = index == 0;
                    }
                    if clicked {
                        answer = Some(index == 0);
                    }
                }
            }
        }
        match answer {
            Some(true) => {
                pulse::set_asking(None);
                let selected = view.selected;
                pulse::with(|p| {
                    if selected >= p.moments.len() {
                        // Accept Defeat: back to the present, the Game Over goes on
                        p.cancel();
                    } else if let Err(error) = p.confirm() {
                        p.toast = Some(error);
                    }
                });
            }
            Some(false) => pulse::set_asking(None),
            None => pulse::set_asking(Some(yes)),
        }
    } else {
        let mut selected = view.selected;
        let mut ask = false;
        if nav.left {
            selected = selected.saturating_sub(1);
        }
        if nav.right {
            selected = (selected + 1).min(count);
        }
        if nav.prev_tab {
            // the start of this phase, or of the one before
            let mut at = selected.min(count.saturating_sub(1));
            if at > 0 && view.moments.get(at).is_some_and(|m| m.2) {
                at -= 1;
            }
            while at > 0 && !view.moments[at].2 {
                at -= 1;
            }
            selected = at;
        }
        if nav.next_tab {
            let mut at = selected + 1;
            while at < count && !view.moments[at].2 {
                at += 1;
            }
            selected = at.min(count);
        }
        let wheel: f32 = input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel { delta, unit, .. } => Some(match unit {
                    egui::MouseWheelUnit::Point => delta.y / 40.0,
                    _ => delta.y,
                }),
                _ => None,
            })
            .sum();
        if wheel.abs() >= 0.5 {
            let by = wheel.round() as i64;
            selected = (selected as i64 - by).clamp(0, count as i64) as usize;
        }
        let mut back = nav.back || input.pointer.secondary_clicked();
        if let Some((px, py)) = pointer {
            // a moment of the strip
            if clicked && (l.strip_y..l.strip_y + STRIP_H).contains(&py) {
                for slot in 0..l.shown {
                    let x = cell_x(&l, slot);
                    if (x..x + CELL).contains(&px) {
                        let index = l.first + slot;
                        if index == selected {
                            ask = true;
                        }
                        selected = index;
                    }
                }
                // the arrows at the ends
                if (l.strip_x..l.strip_x + 14.0).contains(&px) {
                    selected = selected.saturating_sub(1);
                }
                if (l.strip_x + l.strip_w - 14.0..l.strip_x + l.strip_w).contains(&px) {
                    selected = (selected + 1).min(count);
                }
            }
            // the prompts under it
            let entries = prompt_entries(&view);
            let pw = prompts_width(&canvas, prompts, &entries) + 12.0;
            let px0 = (width - pw) / 2.0;
            let py0 = l.strip_y + STRIP_H + 4.0;
            if clicked && (py0..py0 + 18.0).contains(&py) && (px0..px0 + pw).contains(&px) {
                // which prompt: each starts where the ones before it end
                let at = |i: usize| px0 + 6.0 + prompts_width(&canvas, prompts, &entries[..i]) + if i > 0 { 6.0 } else { 0.0 };
                if px >= at(3) {
                    back = true;
                } else if px >= at(2) {
                    ask = true;
                } else if px >= at(1) {
                    let mut i = selected + 1;
                    while i < count && !view.moments[i].2 {
                        i += 1;
                    }
                    selected = i.min(count);
                } else {
                    let mut i = selected.min(count.saturating_sub(1));
                    if i > 0 && view.moments.get(i).is_some_and(|m| m.2) {
                        i -= 1;
                    }
                    while i > 0 && !view.moments[i].2 {
                        i -= 1;
                    }
                    selected = i;
                }
            }
        }
        if nav.confirm {
            ask = true;
        }
        if selected != view.selected {
            pulse::with(|p| p.select(selected));
        }
        if back {
            if view.game_over {
                // Accept Defeat is asked, with the present chosen
                pulse::with(|p| p.select(p.moments.len()));
                pulse::set_asking(Some(false));
            } else {
                pulse::with(|p| p.cancel());
            }
        } else if ask {
            if selected >= count {
                pulse::notice("Choose a moment to go back to.");
            } else if view.left == Some(0) {
                pulse::notice("No charges of Divine Pulse are left on this map.");
            } else {
                pulse::set_asking(Some(true));
            }
        }
    }

    // ---- drawing -----------------------------------------------------------
    let Some(view) = pulse::view() else { return };
    let l = layout(&view, width);
    let white = Color32::WHITE;
    // the time stopped: the whole window tinted, breathing slowly (pulse2:
    // where the presenter draws Divine Pulse's effects, it grades the
    // picture itself, behind its wave, and fades the grade out after the
    // screen closes, so the tint here would only pop)
    let since = view.opened.elapsed().as_secs_f32();
    if !crate::pulse_fx::drawn() {
        let fade = (since / 0.25).min(1.0);
        let breath = 0.5 + 0.5 * (since * 1.6).sin();
        let tint = Color32::from_rgba_unmultiplied(40, 150, 120, ((46.0 + 18.0 * breath) * fade) as u8);
        canvas.painter.rect_filled(screen, 0.0, tint);
        let shade = Color32::from_rgba_unmultiplied(0, 20, 16, (70.0 * fade) as u8);
        canvas.painter.rect_filled(screen, 0.0, shade);
    }

    // the bar along the top
    canvas.three(Art::Header, 0.0, 0.0, width, 30.0, [60.0, 60.0], Color32::from_rgb(150, 200, 170));
    canvas.text(14.0, 7.0, "Divine Pulse", 1.0, TEXT_GOLD);
    let line = moment_line(&view, view.selected);
    canvas.text_centered(width / 2.0, 8.0, &line, 0.85, TEXT_CREAM);
    let charges = charges_text(&view);
    let cw = canvas.kit.measure(&charges, 0.82) + 16.0;
    canvas.nine(Art::PillPrompt, width - 10.0 - cw, 6.0, cw, 17.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
    canvas.text(width - 10.0 - cw + 8.0, 7.5, &charges, 0.82, if view.left == Some(0) { Color32::from_rgb(240, 140, 120) } else { TEXT_CREAM });
    if view.game_over && view.asking.is_none() {
        let text = "Your army is about to fall. Turn back time?";
        let w = canvas.kit.measure(text, 0.85) + 20.0;
        canvas.nine(Art::PillPrompt, (width - w) / 2.0, 34.0, w, 18.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
        canvas.text_centered(width / 2.0, 35.5, text, 0.85, TEXT_CREAM);
    }

    // the timeline
    canvas.nine(Art::PillPrompt, l.strip_x, l.strip_y, l.strip_w, STRIP_H, [7.0, 7.0, 7.0, 7.0], 1.0, white);
    let bob = ((now * 6.0).sin() * 1.5) as f32;
    for slot in 0..l.shown {
        let index = l.first + slot;
        let x = cell_x(&l, slot);
        let chosen = index == view.selected;
        let (turn, force, start, actor) = match view.moments.get(index) {
            Some((turn, phase, start, actor)) => (*turn, *phase, *start, actor.clone()),
            None => (view.turn_now.0, view.turn_now.1, false, None),
        };
        let is_now = index >= view.moments.len();
        if chosen {
            canvas.three(Art::Highlight, x + 1.0, l.strip_y + 3.0, CELL - 2.0, STRIP_H - 6.0, [8.0, 8.0], white);
        }
        // the pennant: the phase's colour, the turn where a phase starts
        let tag = if is_now { Art::PillCream } else { Art::Tag(tag_colour(force)) };
        let tint = if chosen || is_now { white } else { Color32::from_rgb(190, 180, 170) };
        if is_now {
            canvas.nine(tag, x + 7.0, l.strip_y + 6.0, CELL - 14.0, 15.0, [9.0, 9.0, 9.0, 9.0], 0.55, white);
            canvas.text_centered(x + CELL / 2.0, l.strip_y + 6.5, "Now", 0.75, TEXT_DARK);
        } else {
            canvas.nine(tag, x + 5.0, l.strip_y + 6.0, CELL - 10.0, 15.0, [7.0, 6.0, 12.0, 7.0], 0.6, tint);
            let label = if start { format!("T{turn}") } else { "\u{b7}".to_owned() };
            canvas.text_centered(x + CELL / 2.0 - 2.0, l.strip_y + 6.5, &label, 0.72, TEXT_CREAM);
        }
        // who acts next from here
        let name = if is_now { "Present".to_owned() } else { actor.map(|(name, _)| name).unwrap_or_else(|| "\u{2014}".to_owned()) };
        let mut scale = 0.62;
        while canvas.kit.measure(&name, scale) > CELL - 4.0 && scale > 0.45 {
            scale -= 0.04;
        }
        canvas.text_centered(x + CELL / 2.0, l.strip_y + 24.0, &name, scale, if chosen { TEXT_CREAM } else { Color32::from_rgb(220, 206, 176) });
        // a phase begins: a thin rule before it
        if start && slot > 0 {
            canvas.fill(x - 0.5, l.strip_y + 5.0, 1.0, STRIP_H - 10.0, Color32::from_rgba_unmultiplied(255, 230, 170, 120));
        }
        if chosen {
            // the game's pointing hand, at the chosen one's pennant
            canvas.image(Art::Hand, x - 9.0 + bob, l.strip_y + 9.0, 16.0, 9.3, white);
        }
    }
    if l.first > 0 {
        canvas.image(Art::ArrowLeft, l.strip_x + 3.0, l.strip_y + STRIP_H / 2.0 - 5.0, 6.3, 10.0, white);
    }
    if l.first + l.shown < view.moments.len() + 1 {
        canvas.image(Art::ArrowRight, l.strip_x + l.strip_w - 9.3, l.strip_y + STRIP_H / 2.0 - 5.0, 6.3, 10.0, white);
    }

    // the prompts under it
    let entries = prompt_entries(&view);
    let pw = prompts_width(&canvas, prompts, &entries) + 12.0;
    let px0 = (width - pw) / 2.0;
    let py0 = l.strip_y + STRIP_H + 4.0;
    canvas.nine(Art::PillPrompt, px0, py0, pw, 18.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
    draw_prompts(&canvas, prompts, px0 + 6.0, py0 + 0.5, &entries);

    if let Some(text) = &view.notice {
        let w = canvas.kit.measure(text, 0.85) + 20.0;
        canvas.nine(Art::PillRed, (width - w) / 2.0, l.strip_y - 34.0, w, 18.0, [9.0, 9.0, 9.0, 9.0], 0.62, white);
        canvas.text_centered(width / 2.0, l.strip_y - 32.5, text, 0.85, TEXT_CREAM);
    }

    // the question
    if let Some(yes) = view.asking {
        let (x, y, w, h, lines) = dialog(&canvas, &view, width);
        canvas.nine(Art::Dialog, x, y, w, h, [22.0, 22.0, 22.0, 22.0], 0.5, white);
        for (i, line) in lines.iter().enumerate() {
            canvas.text_centered(x + w / 2.0, y + 11.0 + i as f32 * 15.0, line, 0.9, TEXT_DARK);
        }
        for (index, (rx, ry, rw)) in dialog_rows(&canvas, &view, width).iter().enumerate() {
            let chosen = (index == 0) == yes;
            if chosen {
                canvas.three(Art::Highlight, *rx, ry - 1.0, *rw, 20.0, [10.0, 10.0], white);
                canvas.sprite(Art::Hand, rx - 20.0 + bob, ry + 2.5, white);
            }
            canvas.text_centered(rx + rw / 2.0, ry + 2.0, ["Yes", "No"][index], 0.95, if chosen { TEXT_CREAM } else { TEXT_DARK });
        }
    }
    let _ = (l.width, Prompt::Start);
}

fn question(view: &pulse::View) -> Vec<String> {
    if view.selected >= view.moments.len() {
        return vec!["Accept defeat?".to_owned(), "The battle will be lost.".to_owned()];
    }
    let left = match view.left {
        Some(left) => format!("{} {} will be left.", left.saturating_sub(1), if left == 2 { "charge" } else { "charges" }),
        None => "Divine Pulse has no limit here.".to_owned(),
    };
    vec!["Turn back time to this moment?".to_owned(), moment_line(view, view.selected), left]
}

fn dialog(canvas: &Canvas, view: &pulse::View, width: f32) -> (f32, f32, f32, f32, Vec<String>) {
    let lines = question(view);
    let w = lines.iter().map(|line| canvas.kit.measure(line, 0.9)).fold(200.0, f32::max) + 40.0;
    let h = lines.len() as f32 * 15.0 + 22.0 + 2.0 * 19.0 + 8.0;
    let x = (width - w) / 2.0;
    let y = (240.0 - STRIP_H - 30.0 - h) / 2.0 + 16.0;
    (x, y, w, h, lines)
}

fn dialog_rows(canvas: &Canvas, view: &pulse::View, width: f32) -> [(f32, f32, f32); 2] {
    let (x, y, w, _, lines) = dialog(canvas, view, width);
    let top = y + 12.0 + lines.len() as f32 * 15.0 + 6.0;
    [(x + 60.0, top, w - 120.0), (x + 60.0, top + 19.0, w - 120.0)]
}

// ---- the Gameplay tab's rows ------------------------------------------------------

use super::options::{Effect, Item};
use crate::settings::{PcSettings, PulseCharges, Settings};

/// the option's mode, for which rows show: 0 off, 1 story, 2 fixed, 3 unlimited.
pub fn mode(pc: &PcSettings) -> u8 {
    match (pc.divine_pulse, pc.divine_pulse_charges) {
        (false, _) => 0,
        (true, PulseCharges::Story) => 1,
        (true, PulseCharges::Fixed) => 2,
        (true, PulseCharges::Unlimited) => 3,
    }
}

/// the charges' rows under Divine Pulse while it is on.
pub fn gameplay_rows(list: &mut Vec<Item>, mode: u8) {
    let Some(at) = list.iter().position(|item| *item == Item::DivinePulse) else { return };
    if mode != 0 {
        list.insert(at + 1, Item::PulseCharges);
    }
    if mode == 2 {
        list.insert(at + 2, Item::PulseFixed);
    }
}

pub fn row(item: Item, settings: &Settings) -> (String, Option<String>, String) {
    let pc = &settings.pc;
    let key = {
        let mut keys = settings.keys.clone();
        keys.all_mut().into_iter().find(|(name, _)| *name == "Divine Pulse").map(|(_, binds)| binds.label()).unwrap_or_default()
    };
    let key = if key.trim().is_empty() || key.trim() == "-" { "its key".to_owned() } else { key };
    match item {
        Item::DivinePulse => (
            "Divine Pulse".to_owned(),
            Some(if pc.divine_pulse { "On" } else { "Off" }.to_owned()),
            format!("Turn back time on a battle map to undo actions, as in Three Houses and Engage. Open it from the map's menu or with {key}; it also offers itself before a defeat."),
        ),
        Item::PulseCharges => (
            "Divine Pulse Charges".to_owned(),
            Some(match pc.divine_pulse_charges {
                PulseCharges::Story => "Story",
                PulseCharges::Fixed => "Fixed",
                PulseCharges::Unlimited => "Unlimited",
            }
            .to_owned()),
            match pc.divine_pulse_charges {
                PulseCharges::Story => "As in Three Houses: 3 at first and more as the story goes on, up to 13 (10 on Lunatic). They refill every map.",
                PulseCharges::Fixed => "The same number every map, as Engage's 10 on Hard and Maddening. They refill every map.",
                PulseCharges::Unlimited => "As Engage's Normal: turn back time as often as you like.",
            }
            .to_owned(),
        ),
        _ => ("Charges per Map".to_owned(), Some(pc.divine_pulse_fixed.clamp(1, 99).to_string()), "How many times you can turn back time on each map.".to_owned()),
    }
}

pub fn adjust(item: Item, d: i32, pc: &mut PcSettings) -> Option<Effect> {
    match item {
        Item::DivinePulse => pc.divine_pulse = !pc.divine_pulse,
        Item::PulseCharges => {
            const ALL: [PulseCharges; 3] = [PulseCharges::Story, PulseCharges::Fixed, PulseCharges::Unlimited];
            let at = ALL.iter().position(|c| *c == pc.divine_pulse_charges).unwrap_or(0) as i32;
            pc.divine_pulse_charges = ALL[(at + d).rem_euclid(3) as usize];
        }
        _ => pc.divine_pulse_fixed = (pc.divine_pulse_fixed.clamp(1, 99) as i32 + d).clamp(1, 99) as u32,
    }
    Some(Effect::Apply)
}
