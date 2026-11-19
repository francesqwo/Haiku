//! pulsefix: the check that a moment of Divine Pulse, or a saved state,
//! came back whole: after the console is put back, everything that makes it
//! is compared with what it was at that moment.
//!
//! - every 4 KiB page of the four memories (FCRAM, VRAM, DSP RAM, AXI
//!   WRAM), by a 64-bit hash of each kept at the moment (a state's file has
//!   the memories themselves);
//! - the rest of the console as `System::save_rest` writes it: the CPU, the
//!   kernel's threads and objects (handles, events, timers, mutexes), the
//!   services, the open files, the code modules, the GPU's registers, the
//!   memory map and the physical allocator, the frame counters;
//! - the native sound layer (its voices, queues, play positions, clock, the
//!   AAC decoder's settings), which `save_rest` leaves out;
//! - the overrides' own variables and the tails waiting (`keep`).
//!
//! It is a test and debug option (FATES_PULSE_VERIFY=1 for the moments,
//! FATES_STATE_VERIFY=1 for saved states): it costs a hash of 135 MiB a
//! moment, and the moment's rest kept whole.

use crate::System;

use super::keep;

pub const PAGE: usize = 4096;
const NAMES: [&str; 4] = ["FCRAM", "VRAM", "DSP RAM", "AXI WRAM"];

fn page_hash(page: &[u8]) -> u64 {
    let mut hash: u64 = 0x9e37_79b9_7f4a_7c15;
    for chunk in page.chunks_exact(8) {
        hash = (hash ^ u64::from_le_bytes(chunk.try_into().unwrap())).wrapping_mul(0x0100_0000_01b3).rotate_left(29);
    }
    hash ^ (hash >> 31)
}

fn memories(system: &System) -> [&[u8]; 4] {
    let phys = &system.memory.phys;
    [&phys.fcram, &phys.vram, &phys.dsp_ram, &phys.axi_wram]
}

/// each page's hash, memory by memory.
pub fn page_hashes(system: &System) -> [Vec<u64>; 4] {
    memories(system).map(|memory| memory.chunks(PAGE).map(page_hash).collect())
}

/// the native sound layer as a saved state's section has it.
fn sound(system: &System) -> Vec<u8> {
    system.services.dsp.native.saved().unwrap_or_default()
}

/// the console at a moment, as the check compares it.
pub struct Picture {
    pages: [Vec<u64>; 4],
    rest: Vec<u8>,
    sound: Vec<u8>,
    host: keep::Kept,
}

impl Picture {
    /// the console now; `rest` is what `save_rest` just wrote.
    pub fn take(system: &System, rest: &[u8]) -> Picture {
        Picture { pages: page_hashes(system), rest: rest.to_vec(), sound: sound(system), host: keep::keep() }
    }
}

/// what the check found.
#[derive(Debug, Default)]
pub struct Report {
    pub pages: usize,
    /// the pages that differ: the memory's name and the page's offset in it
    pub differing: Vec<(&'static str, usize)>,
    /// the rest differs: what of it (each part compared with its maps in
    /// key order; bytes alone can differ by a HashMap's order)
    pub rest: Option<String>,
    pub sound_same: bool,
    /// the overrides' variables (and "tails") that differ
    pub host: Vec<String>,
    pub host_regions: usize,
    pub host_tails: usize,
}

impl Report {
    pub fn whole(&self) -> bool {
        self.differing.is_empty() && self.rest.is_none() && self.sound_same && self.host.is_empty()
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: memory {}/{} pages the same", if self.whole() { "whole" } else { "DIFFERS" }, self.pages - self.differing.len(), self.pages)?;
        if !self.differing.is_empty() {
            let shown: Vec<String> = self.differing.iter().take(8).map(|(name, at)| format!("{name}+0x{at:07X}")).collect();
            write!(f, " (differ: {}{})", shown.join(", "), if self.differing.len() > 8 { ", ..." } else { "" })?;
        }
        match &self.rest {
            None => write!(f, ", the rest (CPU, kernel, services, files, GPU, memory map) the same")?,
            Some(what) => write!(f, ", the rest DIFFERS: {what}")?,
        }
        write!(f, ", sound {}", if self.sound_same { "the same" } else { "DIFFERS" })?;
        if self.host.is_empty() {
            write!(f, ", overrides' state the same ({} variables, {} tails)", self.host_regions, self.host_tails)
        } else {
            write!(f, ", overrides' state DIFFERS: {}", self.host.join(", "))
        }
    }
}

/// where two values first differ, as a path ("kernel.threads[3].status").
fn value_difference(path: &str, a: &serde_json::Value, b: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    if a == b {
        return None;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (key, value) in x {
                match y.get(key) {
                    Some(other) => {
                        if let Some(found) = value_difference(&format!("{path}.{key}"), value, other) {
                            return Some(found);
                        }
                    }
                    None => return Some(format!("{path}.{key} (only in one)")),
                }
            }
            y.keys().find(|key| !x.contains_key(*key)).map(|key| format!("{path}.{key} (only in one)"))
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            x.iter().zip(y).enumerate().find_map(|(i, (v, w))| value_difference(&format!("{path}[{i}]"), v, w))
        }
        _ => {
            let show = |v: &Value| {
                let text = v.to_string();
                if text.len() > 60 { format!("{}...", &text[..60]) } else { text }
            };
            Some(format!("{path}: {} against {}", show(a), show(b)))
        }
    }
}

/// what a load changes of a state on purpose, made alike on both sides: the
/// saves' host folders (a state from another folder writes into this run's,
/// pcos::fs::rebase_host_paths) and the open files' order (a state from
/// before pulsefix wrote them in a HashMap's order).
fn as_loaded(name: &str, value: &mut serde_json::Value) {
    use serde_json::Value;
    fn paths(value: &mut Value) {
        match value {
            Value::String(text) if text.contains(":\\") || text.starts_with('/') => *text = "(a host path)".to_owned(),
            Value::Array(items) => items.iter_mut().for_each(paths),
            Value::Object(map) => map.values_mut().for_each(paths),
            _ => {}
        }
    }
    match name {
        "files" => {
            paths(value);
            if let Value::Array(items) = value {
                items.sort_by_key(|item| item.get(0).and_then(Value::as_u64).unwrap_or(0));
            }
        }
        "services" => {
            if let Some(fs) = value.get_mut("fs") {
                paths(fs);
            }
        }
        _ => {}
    }
}

/// the parts of the rest that differ (their maps compared in key order),
/// and where in each first; empty when none does.
fn rest_differences(a: &[u8], b: &[u8]) -> Result<Vec<String>, String> {
    let (mut a, mut b) = (super::rest_parts(a)?, super::rest_parts(b)?);
    for (name, value) in a.iter_mut().chain(b.iter_mut()) {
        as_loaded(name, value);
    }
    Ok(a.iter().zip(&b).filter_map(|((name, x), (_, y))| value_difference(name, x, y)).collect())
}

/// the rest compared: None when the same, else what differs.
fn rest_check(now: &[u8], then: &[u8]) -> Option<String> {
    if now == then {
        return None;
    }
    match rest_differences(now, then) {
        Ok(found) if found.is_empty() => None,
        Ok(found) => Some(found.join("; ")),
        Err(error) => Some(error),
    }
}


/// compares the console now with a picture of the moment it was put back
/// to. `save_rest` runs (writing back what the host's GPU drew, which after
/// a load is nothing).
pub fn compare(system: &mut System, picture: &Picture) -> Result<Report, String> {
    let rest = system.save_rest()?;
    let now = page_hashes(system);
    let mut report = Report::default();
    for (index, (a, b)) in now.iter().zip(&picture.pages).enumerate() {
        report.pages += a.len().max(b.len());
        for (page, (x, y)) in a.iter().zip(b).enumerate() {
            if x != y {
                report.differing.push((NAMES[index], page * PAGE));
            }
        }
    }
    report.rest = rest_check(&rest, &picture.rest);
    report.sound_same = sound(system) == picture.sound;
    let host = keep::keep();
    report.host = keep::differences(&host, &picture.host);
    report.host_regions = host.regions.len();
    report.host_tails = picture.host.tails.len();
    Ok(report)
}

/// a saved state's sections, as the check reads them.
pub struct StateParts {
    pub rest: Vec<u8>,
    pub memories: [Vec<u8>; 4],
    pub sound: Option<Vec<u8>>,
    pub host: Option<keep::Kept>,
}

/// compares the console now with a saved state's file, just after it was
/// loaded: the memories byte for byte (page by page), the rest, the sound
/// section and the overrides' section.
pub fn compare_state(system: &mut System, parts: &StateParts) -> Result<Report, String> {
    let rest = system.save_rest()?;
    let mut report = Report::default();
    for (index, (now, saved)) in memories(system).iter().zip(&parts.memories).enumerate() {
        report.pages += now.len().div_ceil(PAGE);
        if now.len() != saved.len() {
            report.differing.push((NAMES[index], 0));
            continue;
        }
        for (page, (x, y)) in now.chunks(PAGE).zip(saved.chunks(PAGE)).enumerate() {
            if x != y {
                report.differing.push((NAMES[index], page * PAGE));
            }
        }
    }
    report.rest = rest_check(&rest, &parts.rest);
    report.sound_same = parts.sound.as_ref().is_none_or(|saved| *saved == sound(system));
    let host = keep::keep();
    let wanted = parts.host.clone().unwrap_or_else(keep::initial);
    report.host = keep::differences(&host, &wanted);
    report.host_regions = host.regions.len();
    report.host_tails = wanted.tails.len();
    Ok(report)
}
