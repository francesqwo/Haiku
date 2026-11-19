//! audio: FATES_AUDIO_WAV=<file> records what the console's speakers play,
//! 16-bit stereo at 32728 Hz, from whichever route makes it (the native
//! mixer, or the emulated DSP with FATES_NATIVE_AUDIO=0), in headless runs
//! as in windowed ones. The recording runs on the guest clock from boot:
//! time when no frames are made (before the sound system starts, while it
//! is shut down) is written as silence, so recordings of the two routes
//! line up in game time. The header is brought up to date every second, so
//! the file is a valid WAV even when the process is killed.

use std::io::{Seek, SeekFrom, Write};
use std::sync::{Mutex, OnceLock};

struct Recorder {
    file: std::io::BufWriter<std::fs::File>,
    samples: u64,
    since_header: u64,
    /// the guest tick the next frame is expected at.
    next_tick: u64,
}

const RATE: u32 = 32728;

fn header(samples: u64) -> [u8; 44] {
    let data = (samples * 4).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

fn recorder() -> Option<&'static Mutex<Recorder>> {
    static R: OnceLock<Option<Mutex<Recorder>>> = OnceLock::new();
    R.get_or_init(|| {
        let path = std::env::var_os("FATES_AUDIO_WAV")?;
        let mut file = match std::fs::File::create(&path) {
            Ok(file) => std::io::BufWriter::new(file),
            Err(error) => {
                log::warn!("audio: cannot record to {}: {error}", std::path::Path::new(&path).display());
                return None;
            }
        };
        let _ = file.write_all(&header(0));
        log::info!("audio: recording the output to {}", std::path::Path::new(&path).display());
        Some(Mutex::new(Recorder { file, samples: 0, since_header: 0, next_tick: 0 }))
    })
    .as_ref()
}

/// statesnd: a saved state was loaded: the guest clock jumps (on, or
/// back), and the recording goes straight on with the loaded moment's
/// sound instead of writing the jump as silence.
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// statesnd: see LOADED.
pub fn state_loaded() {
    LOADED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// records one frame as it comes out of the mixer, made for guest time
/// `tick`.
pub fn record(samples: &[[i16; 2]], tick: u64) {
    let Some(recorder) = recorder() else { return };
    let mut r = recorder.lock().unwrap();
    let period = crate::CYCLES_PER_AUDIO_FRAME;
    if LOADED.swap(false, std::sync::atomic::Ordering::Relaxed) && r.samples > 0 {
        r.next_tick = tick;
    }
    // trailer: FATES_AUDIO_WAV_NOLEAD=1 starts the recording at its first
    // frame (a run from a saved state), not with silence from boot
    if r.samples == 0 && r.next_tick == 0 && std::env::var_os("FATES_AUDIO_WAV_NOLEAD").is_some() {
        r.next_tick = tick;
        log::info!("audio: recording starts at guest tick {tick}");
    }
    // silence for the frames nobody made since the last one
    if tick > r.next_tick + period / 2 {
        let missing = ((tick - r.next_tick + period / 2) / period) as usize;
        let silence = vec![0u8; missing * samples.len() * 4];
        let _ = r.file.write_all(&silence);
        r.samples += (missing * samples.len()) as u64;
        r.next_tick += missing as u64 * period;
    }
    r.next_tick = r.next_tick.max(tick.saturating_sub(period / 2)) + period;
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for [l, right] in samples {
        bytes.extend_from_slice(&l.to_le_bytes());
        bytes.extend_from_slice(&right.to_le_bytes());
    }
    let _ = r.file.write_all(&bytes);
    r.samples += samples.len() as u64;
    r.since_header += samples.len() as u64;
    if r.since_header >= RATE as u64 {
        r.since_header = 0;
        let samples = r.samples;
        let file = &mut r.file;
        let _ = file.flush();
        if let Ok(position) = file.get_mut().stream_position() {
            let _ = file.get_mut().seek(SeekFrom::Start(0));
            let _ = file.get_mut().write_all(&header(samples));
            let _ = file.get_mut().seek(SeekFrom::Start(position));
        }
    }
}

/// pulse2, tests: how many samples the recording holds so far (none when
/// not recording), so a log line can say where in the WAV it happened.
pub fn position() -> Option<u64> {
    let recorder = recorder()?;
    let r = recorder.lock().ok()?;
    Some(r.samples)
}
