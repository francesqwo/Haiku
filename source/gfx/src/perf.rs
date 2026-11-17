//! gfx-perf: timing counters for the native renderer.
//!
//! `FATES_GFX_PERF=N` prints, every N game frames, where the native path's
//! time went since the start: per counter the total, the share of the run's
//! wall time, the mean per frame and how often it ran. Counters nest (a
//! mesh's setup is inside the mesh hook), so shares do not add up to 100%.
//! With the variable unset every scope costs one relaxed load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum C {
    /// Model::DrawMesh hook, all of it
    Mesh,
    /// engine objects to Setup (Objects::read, mesh_setup, submesh_setup)
    MeshSetup,
    /// Setup to scene::Shaded (uniform file, Draw block, tables)
    MeshShaded,
    /// texture reads, hashes and decodes for mesh draws
    MeshTextures,
    /// the eft hook (decode and draws)
    Eft,
    /// prim::Context::RenderTarget hook (2D decode)
    Record,
    /// a display transfer completed a frame (texture reads, CPU-path render)
    Complete,
    /// compose (CPU path) / compose_gpu
    Compose,
    /// Renderer::render, all of it
    Render,
    /// buffers, pipelines, textures and descriptor sets before recording
    RenderPrepare,
    /// shaded draws' share of the preparation
    RenderShaded,
    /// texture uploads (each one a submit and wait)
    Upload,
    /// pipelines created
    Pipeline,
    /// command recording (render pass, draws)
    RenderRecord,
    /// post effect blocks run (inside RenderRecord)
    PostRun,
    /// submit and wait for the frame's commands
    Wait,
    /// copying the read-back picture out
    Readback,
    /// the shadow pass (casters' setup, offscreen render, blur, readback)
    Shadow,
    /// the shadow pass's render (inside Shadow)
    ShadowRender,
    /// the shadow map's blur and reordering on the CPU (inside Shadow)
    ShadowBlur,
    /// the emulated GPU running command lists (zakuro-gpu)
    EmuGpu,
    /// gfx-hfr: pictures in between (moved draws and their render)
    Between,
    /// bytes uploaded to the shaded pipeline's buffer (count only)
    ShadedBytes,
    /// perf3: the free camera's haze overlays (freecam.rs), the shadow casters' pass, wide2's
    Haze,
    Caster,
    Wide2,
}

const N: usize = C::Wide2 as usize + 1;
const NAMES: [&str; N] = [
    "mesh hook",
    "  setup",
    "  shaded",
    "  textures",
    "eft hook",
    "2D record",
    "complete",
    "compose",
    "render",
    "  prepare",
    "    shaded",
    "    uploads",
    "    pipelines",
    "  record",
    "    post",
    "  wait",
    "  readback",
    "shadow pass",
    "  render",
    "  blur",
    "emulated GPU",
    "in between",
    "shaded bytes",
    "  haze",
  "  caster",
    "  wide2",
];

static NS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static COUNT: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static START: OnceLock<Instant> = OnceLock::new();

/// how often to report, 0: off.
pub fn every() -> u64 {
    static EVERY: OnceLock<u64> = OnceLock::new();
    *EVERY.get_or_init(|| {
        let n = std::env::var("FATES_GFX_PERF").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        if n > 0 {
            START.get_or_init(Instant::now);
        }
        n
    })
}

pub fn on() -> bool {
    every() > 0
}

/// times its scope into a counter.
pub struct Scope(C, Instant);

impl Drop for Scope {
    fn drop(&mut self) {
        let ns = self.1.elapsed().as_nanos() as u64;
        if on() {
            add(self.0, ns);
        }
        // smooth: a long one goes to the timing log, by its name
        if ns >= 1_000_000 && zakuro_gpu::smooth::enabled() {
            // rtpace: the drawing thread's as gfxw, the frontend's as gfx
            let kind = if crate::rtpace::on_drawing_thread() { "gfxw" } else { "gfx" };
            zakuro_gpu::smooth::span(kind, self.1, self.1 + std::time::Duration::from_nanos(ns), self.0 as i64, 0.0);
        }
    }
}

pub fn scope(c: C) -> Option<Scope> {
    // smooth: the timing log (FATES_SMOOTH_LOG) takes long scopes too
    (on() || zakuro_gpu::smooth::enabled()).then(|| Scope(c, Instant::now()))
}

pub fn add(c: C, ns: u64) {
    NS[c as usize].fetch_add(ns, Ordering::Relaxed);
    COUNT[c as usize].fetch_add(1, Ordering::Relaxed);
}

/// a count with no time (bytes, events).
pub fn count(c: C, n: u64) {
    if on() {
        COUNT[c as usize].fetch_add(n, Ordering::Relaxed);
    }
}

pub fn report(frames: u64) {
    let Some(start) = START.get() else { return };
    let wall = start.elapsed().as_nanos().max(1) as f64;
    let mut out = format!("gfx-perf: {frames} frames, {:.2} s wall, emulated copies skipped {}", wall / 1e9, zakuro_core::native_gfx::skipped());
    for i in 0..N {
        let (ns, n) = (NS[i].load(Ordering::Relaxed), COUNT[i].load(Ordering::Relaxed));
        if n == 0 {
            continue;
        }
        if i == C::ShadedBytes as usize {
            out += &format!("\n  {:<16} {:>10.1} MB, {:.1} KB/frame", NAMES[i], n as f64 / 1e6, n as f64 / 1e3 / frames.max(1) as f64);
            continue;
        }
        out += &format!(
            "\n  {:<16} {:>8.3} s {:>5.1}%  {:>7.3} ms/frame  {:>9} calls  {:>7.1} us/call",
            NAMES[i],
            ns as f64 / 1e9,
            ns as f64 / wall * 100.0,
            ns as f64 / 1e6 / frames.max(1) as f64,
            n,
            ns as f64 / 1e3 / n as f64
        );
    }
    eprintln!("{out}");
}

/// prints a message the first time its text is seen. For "not ported"
/// notes that would otherwise go to stderr on every draw, which costs more
/// than the draw.
pub fn warn_once(message: String) {
    static SEEN: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if seen.get_or_insert_with(Default::default).insert(message.clone()) {
        eprintln!("{message} (shown once)");
    }
}

/// gfx-perf: the process's environment, read once. The renderer asks for
/// its FATES_GFX_* switches on every draw, and on Windows each lookup takes
/// a lock and converts the name (about a microsecond). Nothing sets these
/// variables while the game runs.
fn env_snapshot() -> &'static std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
    static ENV: OnceLock<std::collections::HashMap<std::ffi::OsString, std::ffi::OsString>> = OnceLock::new();
    ENV.get_or_init(|| std::env::vars_os().collect())
}

/// std::env::var_os, from the snapshot.
pub fn var_os(name: &str) -> Option<std::ffi::OsString> {
    env_snapshot().get(std::ffi::OsStr::new(name)).cloned()
}

/// std::env::var, from the snapshot.
pub fn var(name: &str) -> Result<String, std::env::VarError> {
    match env_snapshot().get(std::ffi::OsStr::new(name)) {
        Some(v) => v.clone().into_string().map_err(std::env::VarError::NotUnicode),
        None => Err(std::env::VarError::NotPresent),
    }
}
