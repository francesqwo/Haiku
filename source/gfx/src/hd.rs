//! hdpacks: texture-pack pictures (Citra's custom textures, HD packs) for
//! the native renderer.
//!
//! The pack itself (which picture replaces which texture, by Citra's hash)
//! is zakuro-gpu's `pack::Pack`, the one the mods and the controller glyphs
//! set. This module is how the native renderer draws with it without
//! stalling a frame or keeping every picture it ever drew:
//!
//! - **Found once per texture content.** A texture's bytes are hashed the
//!   way the pack was made (Citra's legacy hash decodes the texture, a few
//!   milliseconds for a large ETC1 one) only the first time those bytes are
//!   seen; the answer is kept by the renderer's own cheap content hash.
//! - **Read in the background.** A picture is read (PNG decode, its smaller
//!   sizes made, laid out the way the native renderer samples it) on worker
//!   threads. Until it is ready the game's own texture is drawn, and the
//!   texture is looked at again next frame.
//! - **Handed out a little at a time.** At most `FATES_HD_FRAME_MB` (64 MB)
//!   of new pictures go to the GPU in one frame (one larger picture always
//!   may), so a scene with many new pictures fills in over a few frames
//!   rather than stopping on one.
//! - **Kept within a budget.** Read pictures stay while they are drawn and
//!   are let go of, least recently drawn first, past `FATES_HD_CPU_MB`
//!   (2048 MB); the GPU's copies go past `FATES_HD_GPU_MB` (4096 MB) in
//!   render.rs `evict`.
//! - **With their smaller sizes.** Each picture comes with its mip chain, so
//!   a picture larger than it is drawn (a 4x pack at a low render scale, or
//!   a map floor seen from afar) is filtered rather than shimmering.
//!
//! `FATES_HD_LOG=1` logs each picture as it is read; `FATES_HD=0` draws
//! the game's own textures although a pack is on.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use zakuro_gpu::pack::{Material, Pack};
use zakuro_gpu::texture::TextureFormat;

use crate::render::Decoded;

/// what `get` found for a texture.
pub enum Got {
    /// the pack has no picture for it (or none is on): drawn as it is.
    None,
    /// the pack has one, still being read: drawn as it is for now.
    Pending,
    /// the picture, ready to draw.
    Ready(Arc<Decoded>),
}

enum Slot {
    Loading,
    Ready { decoded: Arc<Decoded>, bytes: u64, used: u64, handed: bool },
    Broken,
}

struct Job {
    material: Arc<Material>,
    flip: bool,
    key: (u64, bool),
}

struct Done {
    key: (u64, bool),
    result: Result<(Decoded, u128), String>,
    path: String,
}

pub struct Hd {
    /// the pack the answers below are for
    pack: Option<Arc<Pack>>,
    /// by a texture's content hash, format and size: its picture, if any
    found: HashMap<(u64, u32, u32, u32), Option<Arc<Material>>>,
    /// pictures by their hash and orientation
    slots: HashMap<(u64, bool), Slot>,
    jobs: Option<mpsc::Sender<Job>>,
    done: Option<mpsc::Receiver<Done>>,
    /// bytes handed to the GPU this frame, and the frame
    frame_bytes: u64,
    frame: u64,
    kept_bytes: u64,
    pub stats: Stats,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    /// textures looked up in the pack (distinct contents), and found
    pub looked: u64,
    pub found: u64,
    /// pictures read, broken, and handed out later for the frame budget
    pub read: u64,
    pub broken: u64,
    pub deferred: u64,
    pub read_ms: u128,
}

fn env_mb(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default) << 20
}

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_HD").map_or(true, |v| v != "0"))
}

pub fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FATES_HD_LOG").is_some())
}

/// the bytes of textures the native renderer keeps on the GPU before the
/// least recently used go (FATES_HD_GPU_MB, 4096 MB; the game's own
/// textures are a few MB, so in practice the packs' pictures).
pub fn gpu_budget() -> u64 {
    static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| env_mb("FATES_HD_GPU_MB", 4096))
}

/// the largest picture side kept: larger ones are halved down to it.
fn kept_size() -> u32 {
    std::env::var("FATES_HD_MAX_SIZE").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(4096).clamp(64, 8192)
}

impl Default for Hd {
    fn default() -> Self {
        Hd {
            pack: None,
            found: HashMap::new(),
            slots: HashMap::new(),
            jobs: None,
            done: None,
            frame_bytes: 0,
            frame: 0,
            kept_bytes: 0,
            stats: Stats::default(),
        }
    }
}

impl Hd {
    /// a new gfx frame: what the workers finished comes in, the frame's
    /// budget starts again, and pictures past the memory budget go.
    pub fn frame(&mut self, frame: u64) {
        self.frame = frame;
        self.frame_bytes = 0;
        // the pack changed (mods, or the glyph set merged over them)
        let current = if enabled() { zakuro_gpu::pack::current() } else { None };
        let same = match (&self.pack, &current) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.found.clear();
            self.pack = current;
            // pictures already read stay: they are the same files by hash
        }
        if let Some(done) = &self.done {
            while let Ok(done) = done.try_recv() {
                let slot = match done.result {
                    Ok((decoded, ms)) => {
                        let bytes = decoded.rgba.len() as u64;
                        self.stats.read += 1;
                        self.stats.read_ms += ms;
                        if log_on() {
                            log::info!("hdpacks: read {} ({}x{}, {} sizes, {} KB) in {ms} ms", done.path, decoded.width, decoded.height, decoded.levels, bytes >> 10);
                        }
                        self.kept_bytes += bytes;
                        Slot::Ready { decoded: Arc::new(decoded), bytes, used: frame, handed: false }
                    }
                    Err(error) => {
                        self.stats.broken += 1;
                        log::warn!("hdpacks: the texture pack's {} can't be read, {error}; the game's own texture is drawn", done.path);
                        Slot::Broken
                    }
                };
                self.slots.insert(done.key, slot);
            }
        }
        self.trim();
        if log_on() && frame % 300 == 0 && self.stats.looked > 0 {
            let s = self.stats;
            log::info!(
                "hdpacks: frame {frame}: {} textures looked up, {} in the pack, {} pictures read ({} ms in all), {} broken, {} handed out a frame later, {} MB kept",
                s.looked, s.found, s.read, s.read_ms, s.broken, s.deferred, self.kept_bytes >> 20
            );
        }
    }

    /// the least recently drawn pictures go while more than the budget is
    /// kept, never one drawn in the last frames. A picture handed out long
    /// ago counts as new again (its GPU copy may have gone).
    fn trim(&mut self) {
        let frame = self.frame;
        for slot in self.slots.values_mut() {
            if let Slot::Ready { used, handed, .. } = slot {
                if frame.saturating_sub(*used) > 300 {
                    *handed = false;
                }
            }
        }
        let budget = {
            static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            *BUDGET.get_or_init(|| env_mb("FATES_HD_CPU_MB", 2048))
        };
        if self.kept_bytes <= budget {
            return;
        }
        let mut old: Vec<((u64, bool), u64, u64)> = self
            .slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                Slot::Ready { used, bytes, .. } if frame.saturating_sub(*used) > 2 => Some((*key, *used, *bytes)),
                _ => None,
            })
            .collect();
        old.sort_by_key(|(_, used, _)| *used);
        for (key, _, bytes) in old {
            if self.kept_bytes <= budget * 3 / 4 {
                break;
            }
            self.slots.remove(&key);
            self.kept_bytes -= bytes;
        }
    }

    /// whether a pack is on, so that a texture is worth looking up.
    pub fn active(&self) -> bool {
        self.pack.is_some()
    }

    /// the picture for a texture of these bytes (`content` the renderer's
    /// hash of them), drawn with row 0 at t = 0 (`flip`) or t = 1.
    pub fn get(&mut self, raw: &[u8], content: u64, format: TextureFormat, width: u32, height: u32, flip: bool) -> Got {
        let Some(pack) = self.pack.clone() else { return Got::None };
        let number = format_number(format);
        let material = self
            .found
            .entry((content, number, width, height))
            .or_insert_with(|| {
                let mut rows = Vec::new();
                let found = pack.find(raw, format, width, height, &mut rows);
                self.stats.looked += 1;
                if found.is_some() {
                    self.stats.found += 1;
                }
                found
            })
            .clone();
        if self.found.len() > 65536 {
            self.found.clear();
        }
        let Some(material) = material else { return Got::None };
        let key = (material.hash(), flip);
        match self.slots.get_mut(&key) {
            None => {
                self.slots.insert(key, Slot::Loading);
                self.spawn(Job { material, flip, key });
                Got::Pending
            }
            Some(Slot::Loading) => Got::Pending,
            Some(Slot::Broken) => Got::None,
            Some(Slot::Ready { decoded, bytes, used, handed }) => {
                *used = self.frame;
                if !*handed {
                    let budget = {
                        static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
                        *BUDGET.get_or_init(|| env_mb("FATES_HD_FRAME_MB", 64))
                    };
                    // one picture a frame always goes, however large
                    if self.frame_bytes > 0 && self.frame_bytes + *bytes > budget {
                        self.stats.deferred += 1;
                        return Got::Pending;
                    }
                    self.frame_bytes += *bytes;
                    *handed = true;
                }
                Got::Ready(decoded.clone())
            }
        }
    }

    fn spawn(&mut self, job: Job) {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<Job>();
            let (finished, done) = mpsc::channel::<Done>();
            let queue = Arc::new(Mutex::new(queue));
            let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
            let workers = std::env::var("FATES_HD_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or((cores / 8).clamp(1, 4));
            for index in 0..workers {
                let queue = queue.clone();
                let finished = finished.clone();
                let _ = std::thread::Builder::new().name(format!("hd pictures {index}")).spawn(move || loop {
                    let job = match queue.lock() {
                        Ok(queue) => queue.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else { return };
                    let started = std::time::Instant::now();
                    let path = job.material.path().display().to_string();
                    let result = job.material.read_now(kept_size()).map(|picture| (laid_out(picture, job.flip), started.elapsed().as_millis()));
                    if finished.send(Done { key: job.key, result, path }).is_err() {
                        return;
                    }
                });
            }
            self.jobs = Some(jobs);
            self.done = Some(done);
        }
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

/// a picture (rows from the top, then its smaller sizes) as a Decoded the
/// native renderer samples: as it is for row 0 at t = 1 (raster.frag, the
/// 3D), each size turned over for row 0 at t = 0 (the 2D layer).
fn laid_out(picture: zakuro_gpu::pack::Picture, flip: bool) -> Decoded {
    let mut rgba = picture.texels;
    if flip {
        for level in &picture.levels {
            let row = level.width as usize * 4;
            let rows = level.height as usize;
            let bytes = &mut rgba[level.offset..level.offset + row * rows];
            for y in 0..rows / 2 {
                let (top, bottom) = bytes.split_at_mut((rows - 1 - y) * row);
                top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
            }
        }
    }
    let levels = picture.levels.len().max(1) as u32;
    Decoded { width: picture.width, height: picture.height, rgba, levels }
}

/// a picture from its smaller size that is width x height on (that size
/// and the ones below it), none when it has none (it is that size, or has
/// no smaller sizes).
pub fn at_size(d: &Decoded, width: u32, height: u32) -> Option<Decoded> {
    if d.width <= width || d.levels <= 1 {
        return None;
    }
    let mut offset = 0usize;
    for level in 0..d.levels {
        let (w, h) = ((d.width >> level).max(1), (d.height >> level).max(1));
        if w == width && h == height {
            let rgba = d.rgba.get(offset..)?.to_vec();
            return Some(Decoded { width: w, height: h, rgba, levels: d.levels - level });
        }
        offset += (w * h * 4) as usize;
    }
    None
}

/// a format's PICA number.
fn format_number(format: TextureFormat) -> u32 {
    match format {
        TextureFormat::Unknown(number) => number,
        _ => (0..14).find(|&number| TextureFormat::from_raw(number) == format).unwrap_or(0),
    }
}

/// how many times larger than the game's texture a picture is, as a power
/// of two (a 4x pack's: 2), none when it is not larger.
pub fn shift(picture_width: u32, texture_width: u32) -> Option<u8> {
    if texture_width == 0 || picture_width <= texture_width {
        return None;
    }
    Some(((picture_width as f32 / texture_width as f32).log2().round() as u8).max(1))
}

/// the Mip a shaded draw samples a picture with: the game's own range of
/// sizes, moved down by the picture's extra sizes, so that up close the
/// picture's own detail shows and from afar it is no finer than the game's
/// smallest size; a texture with one size gets the picture's sizes down to
/// the game's.
pub fn mip(game: crate::scene::Mip, sampling: crate::prim::Sampling, levels: u32, shift: u8) -> crate::scene::Mip {
    let top = levels.saturating_sub(1).min(255) as u8;
    if game.levels > 1 {
        crate::scene::Mip {
            levels: levels.min(255) as u8,
            min: if game.min == 0 { 0 } else { (game.min + shift).min(top) },
            max: (game.max + shift).min(top),
            ..game
        }
    } else {
        crate::scene::Mip {
            levels: levels.min(255) as u8,
            min: 0,
            max: shift.min(top),
            bias: 0,
            linear: sampling.linear_mag,
            between: sampling.linear_mag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laid_out_turns_each_size_over() {
        // 2x2 then 1x1
        let texels = vec![1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 9, 9, 9, 9];
        let levels = vec![
            zakuro_gpu::pack::Level { offset: 0, width: 2, height: 2 },
            zakuro_gpu::pack::Level { offset: 16, width: 1, height: 1 },
        ];
        let picture = zakuro_gpu::pack::Picture { width: 2, height: 2, texels: texels.clone(), levels: levels.clone() };
        let same = laid_out(picture, false);
        assert_eq!(same.rgba, texels);
        assert_eq!(same.levels, 2);
        let picture = zakuro_gpu::pack::Picture { width: 2, height: 2, texels, levels };
        let over = laid_out(picture, true);
        assert_eq!(over.rgba, vec![3, 3, 3, 3, 4, 4, 4, 4, 1, 1, 1, 1, 2, 2, 2, 2, 9, 9, 9, 9]);
    }

    #[test]
    fn at_size_keeps_the_smaller_sizes() {
        // 4x4, 2x2, 1x1
        let rgba: Vec<u8> = (0..(16 + 4 + 1) * 4).map(|i| i as u8).collect();
        let d = Decoded { width: 4, height: 4, rgba: rgba.clone(), levels: 3 };
        let two = at_size(&d, 2, 2).unwrap();
        assert_eq!((two.width, two.height, two.levels), (2, 2, 2));
        assert_eq!(two.rgba, rgba[64..].to_vec());
        assert!(at_size(&d, 4, 4).is_none());
        assert!(at_size(&d, 3, 3).is_none());
    }

    #[test]
    fn shift_and_mip() {
        assert_eq!(shift(512, 128), Some(2));
        assert_eq!(shift(128, 128), None);
        assert_eq!(shift(4096, 1024), Some(2));
        let sampling = crate::prim::Sampling { wrap: 0, wrap_t: 0, linear_mag: true, linear_min: true, sharp: false };
        let game = crate::scene::Mip { levels: 4, min: 0, max: 3, bias: 0, linear: true, between: true };
        let m = mip(game, sampling, 13, 2);
        assert_eq!((m.levels, m.min, m.max), (13, 0, 5));
        let one = mip(crate::scene::Mip { levels: 1, ..Default::default() }, sampling, 10, 2);
        assert_eq!((one.levels, one.min, one.max, one.linear, one.between), (10, 0, 2, true, true));
    }
}
