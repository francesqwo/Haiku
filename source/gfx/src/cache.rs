//! gfx-perf: caches that keep the native renderer from redoing work whose
//! inputs did not change.
//!
//! - `TextureWatch`: a texture read, hashed and decoded once stays valid for
//!   as long as guest memory says nothing wrote its bytes since. It uses the
//!   same page watches the emulated GPU's texture cache uses
//!   (`Memory::watch_physical` / `unchanged_physical`): the CPU's writes,
//!   DMA, file reads and the emulated GPU's own write-backs all end a watch.
//!   Before the check the range is synced from the emulated GPU, as a full
//!   read would be, so a picture it rendered there is seen.

use std::collections::HashMap;
use std::sync::Arc;

use zakuro_core::System;

use crate::prim::TexRef;
use crate::render::Decoded;

/// the content key a texture had when it was last read, and the watch that
/// says whether its bytes are still those.
#[derive(Clone, Copy)]
struct Watched {
    physical: u32,
    size: u32,
    watch: u64,
    key: u64,
}

/// gfx-perf: the caches, one field of `Gfx`.
#[derive(Default)]
pub struct Caches {
    pub textures: TextureWatch,
    /// set by a texture read whose texture-pack picture is still loading:
    /// that read is not kept, so the picture is found once it is ready
    pub pack_pending: bool,
    /// per display: the native frame whose picture is in the renderer's
    /// target (its id and scale), which a frame drawn without a clear
    /// starts from (`lazy`)
    pub resident: [Option<(u64, u32)>; 2],
    /// per display: the last native frames' pictures, kept on the GPU in
    /// the renderer's presented images (frame id, image view)
    pub pictures: [std::collections::VecDeque<(u64, ash::vk::ImageView)>; 2],
    /// the emulated GPU's device, which the renderer draws on too
    /// (FATES_GFX_OWN_DEVICE=1: a device of its own, as before)
    pub device: Option<Arc<zakuro_gpu::SharedDevice>>,
    pub device_asked: bool,
}

#[derive(Default)]
pub struct TextureWatch {
    entries: HashMap<(TexRef, bool), Watched>,
    /// gfx-terrain's textures with their smaller sizes, by their count
    mips: HashMap<(TexRef, u32), Watched>,
    pub hits: u64,
    pub misses: u64,
    /// gfx-final: writes the syncs of watched textures brought down from
    /// the emulated GPU since the caller last took them
    pub synced: u64,
}

/// the texture's physical range, when it is in memory the watches cover
/// (VRAM or the linear heap: everything the PICA can sample).
fn range(system: &System, tex: TexRef) -> Option<(u32, u32)> {
    let format = zakuro_gpu::texture::TextureFormat::from_raw(tex.format);
    if matches!(format, zakuro_gpu::texture::TextureFormat::Unknown(_)) || tex.width == 0 || tex.height == 0 {
        return None;
    }
    let size = tex.width * tex.height * format.bits_per_pixel() / 8;
    let physical = zakuro_core::services::gsp::virtual_to_physical(system, tex.addr);
    (physical != tex.addr).then_some((physical, size))
}

impl TextureWatch {
    /// the key the texture had when last read, if nothing wrote its bytes
    /// since and its decoded texels are still kept.
    pub fn lookup(&mut self, system: &mut System, tex: TexRef, flip: bool, decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> Option<u64> {
        if crate::perf::var("FATES_GFX_TEXWATCH").is_ok_and(|v| v == "0") {
            return None; // gfx-bottom: dev, every texture read again
        }
        let w = *self.entries.get(&(tex, flip))?;
        // what the emulated GPU drew there comes down first (as a read would
        // bring it), which ends the watch when it changed anything
        // gfx-bottom: synced by its virtual address (see texture_oriented_read)
        let virt = zakuro_core::services::gsp::physical_to_virtual(system, w.physical);
        self.synced += system.sync_gpu_counted(virt, w.size); // gfx-final: counted
        if system.memory.unchanged_physical(w.physical, w.size, w.watch) && decoded.contains_key(&(tex, w.key)) {
            self.hits += 1;
            return Some(w.key);
        }
        self.entries.remove(&(tex, flip));
        None
    }

    /// the texture was just read and gave `key`: watch its bytes from now.
    pub fn store(&mut self, system: &mut System, tex: TexRef, flip: bool, key: u64) {
        self.misses += 1;
        let Some((physical, size)) = range(system, tex) else { return };
        if self.entries.len() > 8192 {
            self.entries.clear();
        }
        let watch = system.memory.watch_physical(physical, size);
        self.entries.insert((tex, flip), Watched { physical, size, watch, key });
    }

    /// `lookup` for a texture with `levels` sizes one after the other.
    pub fn lookup_mip(&mut self, system: &mut System, tex: TexRef, levels: u32, decoded: &HashMap<(TexRef, u64), Arc<Decoded>>) -> Option<u64> {
        let w = *self.mips.get(&(tex, levels))?;
        // gfx-bottom: synced by its virtual address (see texture_oriented_read)
        let virt = zakuro_core::services::gsp::physical_to_virtual(system, w.physical);
        self.synced += system.sync_gpu_counted(virt, w.size); // gfx-final: counted
        if system.memory.unchanged_physical(w.physical, w.size, w.watch) && decoded.contains_key(&(tex, w.key)) {
            self.hits += 1;
            return Some(w.key);
        }
        self.mips.remove(&(tex, levels));
        None
    }

    /// `store` for a texture with `levels` sizes.
    pub fn store_mip(&mut self, system: &mut System, tex: TexRef, levels: u32, key: u64) {
        self.misses += 1;
        let Some((physical, _)) = range(system, tex) else { return };
        let format = zakuro_gpu::texture::TextureFormat::from_raw(tex.format);
        let size: u32 = (0..levels).map(|l| (tex.width >> l) * (tex.height >> l) * format.bits_per_pixel() / 8).sum();
        if self.mips.len() > 8192 {
            self.mips.clear();
        }
        let watch = system.memory.watch_physical(physical, size);
        self.mips.insert((tex, levels), Watched { physical, size, watch, key });
    }

    /// forgets every texture (a texture pack changed, or a state was loaded).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.mips.clear();
    }
}

/// gfx-perf: the renderer's pipelines kept on disk between runs. raster.frag
/// is large, and a pipeline made from it takes 70-100 ms to compile; the
/// emulated renderer keeps its own the same way (zakuro-gpu
/// `pipeline_cache`). The file sits beside zakuro's, in
/// %LOCALAPPDATA%\haiku\pipelines\fates-gfx.bin. FATES_GFX_PIPELINE_CACHE=0
/// starts from nothing and keeps nothing.
///
/// smooth: every renderer on a device shares one VkPipelineCache. Before,
/// each renderer (frames, the lower screen's, the two in-between ones) had
/// a cache of its own, loaded from the same file and written back whole
/// after each pipeline it made, so the last one to write threw away what
/// the others had added and a warm run still compiled. Writing is now off
/// the frame's path: a pipeline made marks the cache changed, and a saver
/// thread writes it a moment later (and the last renderer to go, at once).
pub struct PipelineDisk {
    pub cache: ash::vk::PipelineCache,
    shared: Option<Arc<SharedCache>>,
}

/// the cache all renderers on one device share.
struct SharedCache {
    device: ash::Device,
    path: Option<std::path::PathBuf>,
    /// the cache, null once the last renderer let go; the lock keeps a save
    /// and the destruction apart.
    cache: std::sync::Mutex<ash::vk::PipelineCache>,
    users: std::sync::atomic::AtomicUsize,
    dirty: std::sync::atomic::AtomicBool,
}

impl SharedCache {
    /// what the cache holds, to disk, through a file of its own so a run
    /// killed halfway leaves the last one whole.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if *cache == ash::vk::PipelineCache::null() || !self.dirty.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let started = std::time::Instant::now();
        let Ok(data) = (unsafe { self.device.get_pipeline_cache_data(*cache) }) else { return };
        drop(cache);
        let partial = path.with_extension(format!("{}.partial", std::process::id()));
        let ok = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&partial, &data))
            .and_then(|()| std::fs::rename(&partial, path));
        if ok.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        zakuro_gpu::smooth::since("pipesave", started, data.len() as i64, 0.0);
    }
}

/// the shared caches, one per device; the first use starts the saver.
fn shared_caches() -> &'static std::sync::Mutex<Vec<Arc<SharedCache>>> {
    static CACHES: std::sync::OnceLock<std::sync::Mutex<Vec<Arc<SharedCache>>>> = std::sync::OnceLock::new();
    CACHES.get_or_init(|| {
        // the saver: changed caches go to disk within a second of the last
        // pipeline made, so a burst of them is written once
        let _ = std::thread::Builder::new().name("pipeline saver".to_owned()).spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let caches: Vec<Arc<SharedCache>> = shared_caches().lock().unwrap_or_else(|p| p.into_inner()).clone();
            for cache in caches {
                cache.save();
            }
            save_keys();
        });
        std::sync::Mutex::new(Vec::new())
    })
}

impl PipelineDisk {
    fn path() -> Option<std::path::PathBuf> {
        if std::env::var("FATES_GFX_PIPELINE_CACHE").is_ok_and(|v| v == "0") {
            return None;
        }
        let dir = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME")).filter(|v| !v.is_empty())?;
        Some(std::path::PathBuf::from(dir).join("haiku").join("pipelines").join("fates-gfx.bin"))
    }

    pub fn new(device: &ash::Device) -> PipelineDisk {
        use std::sync::atomic::Ordering;
        let mut caches = shared_caches().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(shared) = caches.iter().find(|c| c.device.handle() == device.handle() && c.users.load(Ordering::Acquire) > 0) {
            shared.users.fetch_add(1, Ordering::AcqRel);
            let cache = *shared.cache.lock().unwrap_or_else(|p| p.into_inner());
            return PipelineDisk { cache, shared: Some(shared.clone()) };
        }
        let path = Self::path();
        // a file the driver does not recognise (another GPU or driver) is
        // ignored by it, as Vulkan specifies
        let data = path.as_ref().and_then(|p| std::fs::read(p).ok()).filter(|d| d.len() < (256 << 20)).unwrap_or_default();
        let info = ash::vk::PipelineCacheCreateInfo::default().initial_data(&data);
        let cache = unsafe { device.create_pipeline_cache(&info, None) }
            .or_else(|_| unsafe { device.create_pipeline_cache(&ash::vk::PipelineCacheCreateInfo::default(), None) })
            .unwrap_or(ash::vk::PipelineCache::null());
        log::info!("smooth: the native renderer's pipeline cache starts from {} KB on disk", data.len() / 1024);
        let shared = Arc::new(SharedCache {
            device: device.clone(),
            path,
            cache: std::sync::Mutex::new(cache),
            users: std::sync::atomic::AtomicUsize::new(1),
            dirty: std::sync::atomic::AtomicBool::new(false),
        });
        caches.retain(|c| c.users.load(Ordering::Acquire) > 0);
        caches.push(shared.clone());
        PipelineDisk { cache, shared: Some(shared) }
    }

    /// smooth: marks the cache changed, for a thread that makes pipelines
    /// through it.
    pub fn marker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let shared = self.shared.clone()?;
        Some(Arc::new(move || shared.dirty.store(true, std::sync::atomic::Ordering::Release)))
    }

    /// a pipeline was made: the saver writes the cache to disk shortly.
    pub fn created(&self, _device: &ash::Device) {
        if let Some(shared) = &self.shared {
            shared.dirty.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    pub fn destroy(&mut self, device: &ash::Device) {
        use std::sync::atomic::Ordering;
        let Some(shared) = self.shared.take() else { return };
        self.cache = ash::vk::PipelineCache::null();
        let _caches = shared_caches().lock().unwrap_or_else(|p| p.into_inner());
        if shared.users.fetch_sub(1, Ordering::AcqRel) == 1 {
            // the last renderer on the device: written now, then gone
            shared.save();
            save_keys();
            let mut cache = shared.cache.lock().unwrap_or_else(|p| p.into_inner());
            if *cache != ash::vk::PipelineCache::null() {
                unsafe { device.destroy_pipeline_cache(*cache, None) };
                *cache = ash::vk::PipelineCache::null();
            }
        }
    }
}

/// smooth: makes a graphics pipeline, noting in the timing log
/// (FATES_SMOOTH_LOG) how long it took and whether the pipeline cache had
/// it (a = 1 hit, 0 compiled, -1 unknown; b = what kind, see the callers).
pub fn make_pipeline(
    device: &ash::Device,
    cache: ash::vk::PipelineCache,
    info: ash::vk::GraphicsPipelineCreateInfo<'_>,
    kind: f64,
) -> Result<ash::vk::Pipeline, ash::vk::Result> {
    use ash::vk;
    let started = std::time::Instant::now();
    let mut feedback = vk::PipelineCreationFeedback::default();
    let mut feedback_info = vk::PipelineCreationFeedbackCreateInfo::default().pipeline_creation_feedback(&mut feedback);
    let info = [info.push_next(&mut feedback_info)];
    // SAFETY: the caller's create info, with the feedback chained
    let made = unsafe { device.create_graphics_pipelines(cache, &info, None) }.map_err(|(_, e)| e).map(|p| p[0]);
    let hit = if !feedback.flags.contains(vk::PipelineCreationFeedbackFlags::VALID) {
        -1
    } else {
        feedback.flags.contains(vk::PipelineCreationFeedbackFlags::APPLICATION_PIPELINE_CACHE_HIT) as i64
    };
    zakuro_gpu::smooth::since("pipe", started, hit, kind);
    made
}

/// a quick content hash of words: four lanes, multiply and rotate. Not for
/// keys anyone picks to collide; every hit is compared in full anyway.
pub fn hash_words(words: &[u32]) -> u64 {
    const K: [u64; 4] = [0x9E37_79B9_7F4A_7C15, 0xC2B2_AE3D_27D4_EB4F, 0x1656_67B1_9E37_79F9, 0xD6E8_FEB8_6659_FD93];
    let mut lanes = [0x243F_6A88_85A3_08D3u64, 0x1319_8A2E_0370_7344, 0xA409_3822_299F_31D0, 0x082E_FA98_EC4E_6C89];
    let (chunks, rest) = words.as_chunks::<8>();
    for c in chunks {
        for l in 0..4 {
            let v = c[2 * l] as u64 | (c[2 * l + 1] as u64) << 32;
            lanes[l] = (lanes[l] ^ v).wrapping_mul(K[l]).rotate_left(29);
        }
    }
    let mut h = lanes[0] ^ lanes[1].rotate_left(17) ^ lanes[2].rotate_left(31) ^ lanes[3].rotate_left(47);
    for &w in rest {
        h = (h ^ w as u64).wrapping_mul(K[0]).rotate_left(23);
    }
    (h ^ words.len() as u64).wrapping_mul(K[1])
}

/// raster.frag's Tables block (light lookup tables and the fog table, as
/// value and step pairs) for a draw's raw table words, built once per
/// content: a battle draws some 150 sub-meshes a frame, each with 61 KB of
/// tables, from a handful of different ones.
pub fn tables(luts: &[u32], fog: &[u32], build: impl FnOnce() -> Vec<f32>) -> Arc<Vec<f32>> {
    static CACHE: std::sync::Mutex<Option<HashMap<u64, Entry>>> = std::sync::Mutex::new(None);
    // the last tables asked for, compared in full: most draws in a row
    // light alike, and comparing costs a third of hashing
    static LAST: std::sync::Mutex<Option<Entry>> = std::sync::Mutex::new(None);
    if let Some(e) = LAST.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        if e.luts == luts && e.fog == fog {
            return e.tables.clone();
        }
    }
    let key = hash_words(luts) ^ hash_words(fog).rotate_left(1);
    let found = tables_keyed(&CACHE, key, luts, fog, build);
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some(Entry { luts: luts.to_vec(), fog: fog.to_vec(), tables: found.clone() });
    found
}

struct Entry {
    luts: Vec<u32>,
    fog: Vec<u32>,
    tables: Arc<Vec<f32>>,
}

fn tables_keyed(
    cache: &std::sync::Mutex<Option<HashMap<u64, Entry>>>,
    key: u64,
    luts: &[u32],
    fog: &[u32],
    build: impl FnOnce() -> Vec<f32>,
) -> Arc<Vec<f32>> {
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(e) = cache.get(&key) {
        if e.luts == luts && e.fog == fog {
            return e.tables.clone();
        }
    }
    let tables = Arc::new(build());
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(key, Entry { luts: luts.to_vec(), fog: fog.to_vec(), tables: tables.clone() });
    tables
}

/// gfx-perf: whether fully native frames are drawn on the GPU and read back
/// only when a picture is asked for (the CPU path: headless runs, dumps),
/// instead of read back as each frame completes. FATES_GFX_LAZY=0: as before.
pub fn lazy() -> bool {
    static LAZY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LAZY.get_or_init(|| std::env::var("FATES_GFX_LAZY").map_or(true, |v| v != "0"))
}

impl crate::Gfx {
    /// gfx-perf: a frame drawn natively in full, drawn now and left on the
    /// GPU: in one of the renderer's presented images (read back if it is
    /// asked for), and in the display's target, where the next frame drawn
    /// without a clear starts from.
    pub(crate) fn render_full_lazy(&mut self, frame: &crate::Frame, scale: u32) {
        use crate::render::{Finish, Output, Start};
        let index = frame.disp as usize;
        let (lw, lh) = crate::prim::display_size(frame.disp);
        let k = if frame.disp == 0 { zakuro_gpu::wide::factor().max(1.0) } else { 1.0 };
        let (uw, uh) = ((lh as f32 * scale as f32 * k).round() as u32, lw as u32 * scale);
        let start = match frame.clear {
            Some((rgba, _)) => Start::Clear(rgba),
            None => match self.cache.resident[index] {
                Some((_, s)) if s == scale => Start::Keep,
                // gfx-bottom: the predecessor's picture is gone (a hybrid
                // was composed over the target, the scale changed): from
                // the seed, as an uncleared frame without one starts
                _ => match crate::bottom::seed() {
                    Some(rgba) => {
                        self.stats.bottom.reseeded[index] += 1;
                        Start::Clear(rgba)
                    }
                    None => return,
                },
            },
        };
        self.cache.resident[index] = None;
        self.chain[index] = None;
        self.ensure_renderer();
        let Some(renderer) = self.renderer.as_mut() else { return };
        let depth = frame.clear.map_or(1.0, |c| c.1);
        match renderer.render(index, uw, uh, scale as f32, start, depth, &frame.draws, &frame.texture_keys, &self.decoded, Finish::Present) {
            Ok(Output::Screen(view, _, _)) => {
                self.cache.resident[index] = Some((frame.id, scale));
                let kept = &mut self.cache.pictures[index];
                // the image now holds this frame: older entries for it are gone
                kept.retain(|(_, v)| *v != view);
                kept.push_back((frame.id, view));
                while kept.len() > 3 {
                    kept.pop_front();
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("gfx: render failed, {e}"),
        }
    }

    /// gfx-perf: the picture of a frame `render_full_lazy` drew, read back.
    pub(crate) fn compose_kept(&mut self, index: usize, address: u32, frame: &std::sync::Arc<crate::Frame>, emulated: &zakuro_core::Screen) -> Option<zakuro_core::Screen> {
        let view = self.cache.pictures[index].iter().find(|(id, _)| *id == frame.id)?.1;
        let renderer = self.renderer.as_mut()?;
        let pixels = renderer.read_present(index, view).ok()?;
        let eh = emulated.2;
        if eh == 0 || pixels.len() % (eh as usize * 4) != 0 {
            return None;
        }
        let rw = (pixels.len() / 4) as u32 / eh;
        let picture: zakuro_core::Screen = (Arc::new(pixels), rw, eh);
        self.stats.composed += 1;
        if let Some(dir) = self.dump.clone() {
            self.dump_pair(&dir, index, frame, &picture, emulated);
        }
        self.composed.insert(address, (frame.id, picture.clone()));
        (self.mode == crate::Mode::Native).then_some(picture)
    }
}

/// gfx-perf: whether the post effect being recorded is drawn natively in a
/// frame the native renderer draws in full; then its GPU copies and fills
/// are noted for the emulated GPU to leave out. FATES_GFX_SKIP_COPIES=0
/// keeps them all, =copies leaves out only the texture copies (see
/// dev/reports/gfx-perf.md on hybrid frames that follow).
static COPIES_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn skip_kinds() -> &'static str {
    static KINDS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KINDS.get_or_init(|| match std::env::var("FATES_GFX_SKIP_COPIES").unwrap_or_default().as_str() {
        "0" => String::new(),
        "copies" => "c".into(),
        _ => "tcf".into(),
    })
}

pub fn arm_copies(on: bool) {
    COPIES_ARMED.store(on && !skip_kinds().is_empty(), std::sync::atomic::Ordering::Relaxed);
}

pub fn native_copy(kind: zakuro_core::native_gfx::Copy, input: u32, output: u32) {
    if !COPIES_ARMED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let k = match kind {
        zakuro_core::native_gfx::Copy::Transfer => 't',
        zakuro_core::native_gfx::Copy::TextureCopy => 'c',
        zakuro_core::native_gfx::Copy::Fill => 'f',
    };
    if skip_kinds().contains(k) {
        zakuro_core::native_gfx::note_skip(kind, input, output);
    }
}

/// gfx-perf: shade.vert's Program block (the program words and their
/// operand descriptors) for a setup, shared by every draw with the same
/// program. Replaces building the words and hashing their bytes per draw.
pub fn program(setup: &crate::setup::Setup) -> Arc<Vec<u32>> {
    static CACHE: std::sync::Mutex<Option<HashMap<u64, Vec<Arc<Vec<u32>>>>>> = std::sync::Mutex::new(None);
    static LAST: std::sync::Mutex<Option<Arc<Vec<u32>>>> = std::sync::Mutex::new(None);
    let (p, d) = (&setup.program[..], &setup.descriptors[..]);
    let same = |words: &Vec<u32>| words.len() == p.len() + d.len() && &words[..p.len()] == p && &words[p.len()..] == d;
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(words) = last.as_ref().filter(|w| same(w)) {
        return words.clone();
    }
    let key = hash_words(p) ^ hash_words(d).rotate_left(7);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    let bucket = cache.entry(key).or_default();
    let words = match bucket.iter().find(|w| same(w)) {
        Some(w) => w.clone(),
        None => {
            let w = Arc::new(crate::scene::program(setup));
            bucket.push(w.clone());
            w
        }
    };
    if cache.len() > 1024 {
        cache.clear();
    }
    *last = Some(words.clone());
    words
}

/// gfx-perf: frames drawn without a clear, after a frame that was not
/// native, are drawn natively from black (see prim_target).
/// gfx-bottom: on by default now (FATES_GFX_NOCLEAR_BLACK=0 turns it off);
/// the battle portraits' FaceCallback difference was gfx-terrain's slot fix.
/// See bottom.rs.
pub fn noclear_black() -> bool {
    crate::bottom::seed().is_some()
}

/// gfx-perf: the display transfer into the display buffer at `output`
/// (physical) carries a picture the native renderer draws in full: the
/// emulated GPU leaves it out (FATES_GFX_SKIP_COPIES=0 or =copies keep it).
pub fn native_display(output: u32) {
    if skip_kinds().contains('t') {
        zakuro_core::native_gfx::note_skip(zakuro_core::native_gfx::Copy::Transfer, u32::MAX, output);
    }
}

/// smooth: Vulkan images let go off the frame's path. Freeing device memory
/// takes up to a millisecond an allocation on Windows; evicting a scene's
/// textures at once took 14 ms of a present (timing log, realloc 3). The
/// images handed over are destroyed by a thread of their own, in order.
/// The caller makes sure the GPU no longer uses them.
pub fn reap(device: &ash::Device, images: Vec<(ash::vk::ImageView, ash::vk::Image, ash::vk::DeviceMemory)>) {
    if images.is_empty() {
        return;
    }
    let reaper = reaper();
    reaper.1.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    if reaper.0.lock().unwrap_or_else(|p| p.into_inner()).send((device.clone(), images)).is_err() {
        reaper.1.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// smooth: waits until every image handed to `reap` is gone, before the
/// device can go.
pub fn reap_flush() {
    let Some(reaper) = REAPER.get() else { return };
    while reaper.1.load(std::sync::atomic::Ordering::Acquire) > 0 {
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

type Reaped = (ash::Device, Vec<(ash::vk::ImageView, ash::vk::Image, ash::vk::DeviceMemory)>);
static REAPER: std::sync::OnceLock<(std::sync::Mutex<std::sync::mpsc::Sender<Reaped>>, Arc<std::sync::atomic::AtomicUsize>)> = std::sync::OnceLock::new();

fn reaper() -> &'static (std::sync::Mutex<std::sync::mpsc::Sender<Reaped>>, Arc<std::sync::atomic::AtomicUsize>) {
    REAPER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Reaped>();
        let left = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let theirs = left.clone();
        let _ = std::thread::Builder::new().name("gfx reaper".to_owned()).spawn(move || {
            for (device, images) in rx {
                let started = std::time::Instant::now();
                for (view, image, memory) in images {
                    // SAFETY: handed over unused, each once
                    unsafe {
                        device.destroy_image_view(view, None);
                        device.destroy_image(image, None);
                        device.free_memory(memory, None);
                    }
                    // a few at a time: freeing takes a driver lock that a
                    // present waits for too (measured: a 10 ms present)
                    std::thread::sleep(std::time::Duration::from_micros(500));
                }
                zakuro_gpu::smooth::since("reaped", started, 0, 0.0);
                theirs.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            }
        });
        (std::sync::Mutex::new(tx), left)
    })
}

// ---------------------------------------------------------------------------
// smooth: the shaded pipelines made ahead, off the frame's path
// ---------------------------------------------------------------------------
//
// A raster.frag pipeline the driver has never compiled takes 70-100 ms
// (gfx-perf), a frame and more of hitch the first time a material state is
// drawn. The keys of every shaded pipeline made are kept in
// fates-gfx-keys.txt beside the pipeline cache, and a list played through
// the replays ships in assets/pipelines/fates-gfx-keys.txt; when the first
// shaded draw sets the pipelines up, a thread of low priority makes each
// listed pipeline once through the shared pipeline cache and lets it go, so
// the frame that first needs it finds it in the cache (a millisecond or
// less) instead of compiling it. FATES_GFX_WARM=0 turns it off.

/// a shaded pipeline's key, as kept in the lists (render/shaded.rs encodes).
pub type KeyBytes = [u8; 16];

struct Keys {
    seen: std::collections::BTreeSet<KeyBytes>,
    dirty: bool,
}

fn keys() -> &'static std::sync::Mutex<Keys> {
    static KEYS: std::sync::OnceLock<std::sync::Mutex<Keys>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| std::sync::Mutex::new(Keys { seen: read_keys(&keys_path()), dirty: false }))
}

fn keys_path() -> Option<std::path::PathBuf> {
    PipelineDisk::path().map(|p| p.with_file_name("fates-gfx-keys.txt"))
}

fn shipped_keys_path() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("assets").join("pipelines").join("fates-gfx-keys.txt"))
}

fn read_keys(path: &Option<std::path::PathBuf>) -> std::collections::BTreeSet<KeyBytes> {
    let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else { return Default::default() };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.len() != 32 {
                return None;
            }
            let mut key = [0u8; 16];
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&line[i * 2..i * 2 + 2], 16).ok()?;
            }
            Some(key)
        })
        .collect()
}

/// a shaded pipeline was made: its key goes on the list (written by the
/// pipeline saver).
pub fn note_shaded_key(key: KeyBytes) {
    let mut keys = keys().lock().unwrap_or_else(|p| p.into_inner());
    if keys.seen.insert(key) {
        keys.dirty = true;
    }
}

/// the list to disk, when it changed (called by the pipeline saver).
fn save_keys() {
    let mut keys = keys().lock().unwrap_or_else(|p| p.into_inner());
    if !keys.dirty {
        return;
    }
    keys.dirty = false;
    let text: String = keys.seen.iter().map(|k| k.iter().map(|b| format!("{b:02x}")).collect::<String>() + "\n").collect();
    drop(keys);
    if let Some(path) = keys_path() {
        let partial = path.with_extension(format!("{}.partial", std::process::id()));
        if path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&partial, text)).and_then(|()| std::fs::rename(&partial, &path)).is_err() {
            let _ = std::fs::remove_file(&partial);
        }
    }
}

static WARM: std::sync::Mutex<Option<(Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>)>> = std::sync::Mutex::new(None);
static WARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// starts making the listed shaded pipelines on a thread of their own, once
/// a process; `make` makes one (and lets it go) from its key, false to stop.
pub fn warm_start(make: impl Fn(&KeyBytes) -> bool + Send + 'static) {
    if std::env::var("FATES_GFX_WARM").is_ok_and(|v| v == "0") || WARMED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let mut list = keys().lock().unwrap_or_else(|p| p.into_inner()).seen.clone();
    list.extend(read_keys(&shipped_keys_path()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let theirs = stop.clone();
    let thread = std::thread::Builder::new().name("pipeline warm".to_owned()).spawn(move || {
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThread() -> isize;
                fn SetThreadPriority(thread: isize, priority: i32) -> i32;
            }
            // THREAD_PRIORITY_BELOW_NORMAL: it gives way to the game
            unsafe { SetThreadPriority(GetCurrentThread(), -1) };
        }
        let started = std::time::Instant::now();
        let mut made = 0;
        for key in &list {
            if theirs.load(std::sync::atomic::Ordering::Acquire) || !make(key) {
                break;
            }
            made += 1;
        }
        zakuro_gpu::smooth::since("warm", started, made, list.len() as f64);
        log::info!("smooth: {made} of {} listed shaded pipelines made ahead in {:.0} ms", list.len(), started.elapsed().as_secs_f64() * 1e3);
    });
    if let Ok(thread) = thread {
        *WARM.lock().unwrap_or_else(|p| p.into_inner()) = Some((stop, thread));
    }
}

/// stops the warm thread and waits for it, before anything it uses goes.
pub fn warm_join() {
    let warm = WARM.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some((stop, thread)) = warm {
        stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = thread.join();
    }
}
