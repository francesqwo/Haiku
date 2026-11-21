//! moflexhd: where a movie's time goes (issue #7, higher-resolution MOFLEX
//! movies), for tests: `FATES_MOVIE_STATS=1` has the movie player's stages
//! counted and a headless run end with a `moflexhd:` line.
//!
//! - the game's Mobiclip decoder (VX2DecoderUnpack) and deblocking filter
//!   (MoDeblock), timed in overrides/moflexhd.c by the CPU time of the guest
//!   thread running them (QueryThreadCycleTime: other threads, and time
//!   the thread was not running, are not counted);
//! - the YUV-to-RGB conversion (services::y2r), on the host, and the
//!   writing of its picture to guest memory once the GPU is done with it;
//! - the 2D renderer's reading of a changed movie picture into texels
//!   (source/gfx, `texture_oriented_read`), for textures of 256K texels or
//!   more.
//!
//! Off, nothing is timed and the overrides only test a word.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

/// the stages, the `kind` of `moflexhd_note`.
pub mod kind {
    /// VX2DecoderUnpack, one picture decoded (cycles).
    pub const DECODE: usize = 0;
    /// MoDeblock run by the game's filter (cycles).
    pub const DEBLOCK: usize = 1;
    /// MoDeblock answered for an HD picture without filtering (count).
    pub const UNDEBLOCKED: usize = 2;
    /// y2r: a conversion, the samples gathered and converted (nanoseconds).
    pub const CONVERT: usize = 3;
    /// y2r: the GPU's work on the output range finished and the picture
    /// written there (nanoseconds).
    pub const PLACED: usize = 4;
    /// the 2D renderer: a large texture read into texels (nanoseconds).
    pub const TEXELS: usize = 5;
    /// the 2D renderer: a large changed texture's bytes synced from the
    /// emulated GPU's buffers (nanoseconds).
    pub const TEX_SYNC: usize = 6;
    /// ... read from guest memory and hashed (nanoseconds).
    pub const TEX_READ: usize = 7;
    /// ... looked up in the texture packs (nanoseconds).
    pub const TEX_PACK: usize = 8;
    pub const COUNT: usize = 9;
}

const NAMES: [&str; kind::COUNT] = ["decode", "deblock", "undeblocked", "y2r", "y2r out", "texels", "texture sync", "texture read", "texture packs"];

struct Stage {
    calls: AtomicU64,
    total: AtomicU64,
    most: AtomicU64,
    /// pixels or texels, where the stage has them.
    pixels: AtomicU64,
}

#[allow(clippy::declare_interior_mutable_const)]
const STAGE: Stage = Stage { calls: AtomicU64::new(0), total: AtomicU64::new(0), most: AtomicU64::new(0), pixels: AtomicU64::new(0) };
static STAGES: [Stage; kind::COUNT] = [STAGE; kind::COUNT];

/// nonzero while counting; overrides/moflexhd.c reads it.
#[no_mangle]
pub static moflexhd_counting: AtomicU32 = AtomicU32::new(0);

/// whether FATES_MOVIE_STATS asks for the counts (read once).
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("FATES_MOVIE_STATS").is_some_and(|v| v != "0");
        moflexhd_counting.store(on as u32, Ordering::Relaxed);
        on
    })
}

/// a stage's run: its time (cycles or nanoseconds, by stage) and pixels.
pub fn note(kind: usize, amount: u64, pixels: u64) {
    let Some(stage) = STAGES.get(kind) else { return };
    stage.calls.fetch_add(1, Ordering::Relaxed);
    stage.total.fetch_add(amount, Ordering::Relaxed);
    stage.most.fetch_max(amount, Ordering::Relaxed);
    stage.pixels.fetch_add(pixels, Ordering::Relaxed);
    // FATES_MOVIE_STATS=2: the counts every 60 large textures, for a window
    if kind == kind::TEXELS && stage.calls.load(Ordering::Relaxed) % 60 == 0 && std::env::var_os("FATES_MOVIE_STATS").is_some_and(|v| v == "2") {
        if let Some(line) = summary() {
            eprintln!("{line}");
        }
    }
}

/// for overrides/moflexhd.c: `void moflexhd_note(uint32_t kind, uint64_t
/// cycles, uint32_t pixels);`
#[no_mangle]
pub extern "C" fn moflexhd_note(kind: u32, cycles: u64, pixels: u32) {
    note(kind as usize, cycles, pixels as u64);
}

/// thread cycles per nanosecond, measured once.
fn cycles_per_ns() -> f64 {
    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            // QueryThreadCycleTime counts at the time stamp counter's rate
            let (c0, t0) = (unsafe { std::arch::x86_64::_rdtsc() }, std::time::Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(50));
            let (c1, t1) = (unsafe { std::arch::x86_64::_rdtsc() }, std::time::Instant::now());
            (c1 - c0) as f64 / (t1 - t0).as_nanos().max(1) as f64
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            1.0
        }
    })
}

/// the counts for the end of a headless run, when counting.
pub fn summary() -> Option<String> {
    if !on() {
        return None;
    }
    let rate = cycles_per_ns();
    let mut parts = Vec::new();
    for (k, stage) in STAGES.iter().enumerate() {
        let calls = stage.calls.load(Ordering::Relaxed);
        if calls == 0 {
            continue;
        }
        let ns = |v: u64| if k == kind::DECODE || k == kind::DEBLOCK { v as f64 / rate } else { v as f64 };
        let (total, most, pixels) = (stage.total.load(Ordering::Relaxed), stage.most.load(Ordering::Relaxed), stage.pixels.load(Ordering::Relaxed));
        if k == kind::UNDEBLOCKED {
            parts.push(format!("{} {calls}", NAMES[k]));
        } else {
            let per = ns(total) / calls as f64 / 1e6;
            let per_mpix = if pixels > 0 { format!(", {:.2} ms/Mpixel", ns(total) / 1e6 / (pixels as f64 / 1e6)) } else { String::new() };
            parts.push(format!("{} {calls} x {per:.3} ms (most {:.3} ms{per_mpix})", NAMES[k], ns(most) / 1e6));
        }
    }
    Some(format!("moflexhd: {}", if parts.is_empty() { "no movie".to_owned() } else { parts.join("; ") }))
}

/// tests: FATES_Y2R_VARY=1, every converted picture marked with its number
/// (services::y2r), standing in for a movie whose pictures all differ.
pub fn vary() -> bool {
    static VARY: OnceLock<bool> = OnceLock::new();
    *VARY.get_or_init(|| std::env::var_os("FATES_Y2R_VARY").is_some_and(|v| v != "0"))
}

/// the number of the next marked picture.
pub fn next_picture() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// for overrides/moflexhd.c: a movie too large for the game's decoder (more
/// than 1024 wide or high) ends at once instead of hanging on black; said
/// once a size, in the log, for the modder.
#[no_mangle]
pub extern "C" fn moflexhd_unplayable(width: u32, height: u32) {
    static SAID: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if said.contains(&(width, height)) {
        return;
    }
    said.push((width, height));
    log::warn!(
        "moflexhd: a {width}x{height} movie can't be played: the game's movie decoder takes pictures up to 1024 wide and its textures are 1024x1024 at most (the 3DS's limit); the movie is skipped. Up to 1024x576 (16:9) or 960x576 (5:3) plays"
    );
}

/// where the YUV-to-RGB converter wrote its last pictures (guest address
/// and length): the movie player's textures. The renderer keeps only the
/// last few pictures of a texture there (source/gfx moflexhd.rs).
static PICTURES: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
static ANY_PICTURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// services::y2r: a picture went to `len` bytes at `addr`.
pub fn picture_written(addr: u32, len: u32) {
    let mut pictures = PICTURES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if pictures.contains(&(addr, len)) {
        return;
    }
    pictures.push((addr, len));
    if pictures.len() > 8 {
        pictures.remove(0);
    }
    ANY_PICTURE.store(true, Ordering::Relaxed);
}

/// whether a texture at `addr` is one the converter writes pictures into.
pub fn is_picture(addr: u32) -> bool {
    if !ANY_PICTURE.load(Ordering::Relaxed) {
        return false;
    }
    let pictures = PICTURES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    pictures.iter().any(|&(start, len)| addr >= start && addr - start < len.max(1))
}
