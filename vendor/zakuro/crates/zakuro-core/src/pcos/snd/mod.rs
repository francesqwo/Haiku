//! audio: the native sound layer, which replaces the emulated audio DSP
//! under nn::snd.
//!
//! On the console, nn::snd's driver (`nn::snd::CTR::Dspsnd`) writes voice
//! and mixer parameters into structures in DSP memory, tells the DSP
//! firmware about them through a pipe and an event, and the sound thread
//! waits on the DSP's interrupt (an event the dsp::DSP service signals
//! every 160 samples). Zakuro emulated all of that: the dsp::DSP service,
//! its pipes and semaphore, and a high-level model of the firmware that
//! parses those structures (services/dsp*.rs).
//!
//! Here the driver itself is replaced (overrides/snd_native.c): each
//! Dspsnd call (SetChannelTimer, SetChannelMix, AssignPCM, ...) comes
//! straight to [`mixer::Mixer`] with its arguments; WaitPipe sleeps the
//! sound thread until the native mixer's next frame is due on the guest
//! clock (one svcSleepThread, no DSP event); SyncFrameData renders the due
//! frame(s) natively, reading the wave data from guest memory, and hands
//! nn::snd the voice statuses it expects; SendParameter runs the game's aux
//! effects and voice updates and commits them. nn::aacdec (voice clips)
//! decodes on the host ([`aac`]), and the DSP component load and cache
//! flushes are answered without IPC. The emulated route stays behind
//! FATES_NATIVE_AUDIO=0, for A/B comparison.
//!
//! The clock: a frame is due every 160 * 8192 CPU ticks (4.889 ms, 32728
//! Hz), counted on the guest clock from when the driver initialized, so
//! the output stays in step with emulated time exactly as the DSP's
//! interrupt did; the frontend's queue (zakuro/src/audio.rs) absorbs the
//! difference to real time as before. If the sound thread falls behind,
//! the frames it missed are rendered when it comes back (as the DSP would
//! have played them), up to 64 at a time.

pub mod aac;
pub mod mixer;
pub mod wav;

use zakuro_cpu::Bus;

use super::bridge::Call;
use super::stats;
use crate::System;
use mixer::{Buffer, Format, Scratch};

/// call numbers, the same as in overrides/snd_native.c.
pub mod op {
    pub const FIRST: u32 = 0x500;
    pub const LAST: u32 = 0x5FF;

    pub const INITIALIZE: u32 = 0x500;
    pub const ENABLED: u32 = 0x501;
    pub const FINALIZE: u32 = 0x502;
    pub const WAIT: u32 = 0x503;
    pub const SYNC: u32 = 0x504;
    pub const SEND: u32 = 0x505;
    pub const TEST_LABEL: u32 = 0x506;

    pub const CHANNEL_INITIALIZE: u32 = 0x510;
    pub const CHANNEL_PLAY_START: u32 = 0x511;
    pub const CHANNEL_PLAY_STOP: u32 = 0x512;
    pub const CHANNEL_SYNC_COUNT: u32 = 0x513;
    pub const CHANNEL_TIMER: u32 = 0x514;
    pub const CHANNEL_RIM: u32 = 0x515;
    pub const CHANNEL_MIX: u32 = 0x516;
    pub const CHANNEL_ADPCM_PARAM: u32 = 0x517;
    pub const CHANNEL_FILTER_TYPE: u32 = 0x518;
    pub const CHANNEL_FILTER_MONO: u32 = 0x519;
    pub const CHANNEL_FILTER_BIQUAD: u32 = 0x51A;
    pub const CHANNEL_RESET_NEXT: u32 = 0x51B;
    pub const CHANNEL_APPEND_NEXT: u32 = 0x51C;
    pub const CHANNEL_UPDATE_NEXT: u32 = 0x51D;
    pub const CHANNEL_ASSIGN_PCM: u32 = 0x51E;

    pub const MASTER_VOLUME: u32 = 0x530;
    pub const AUX_RETURN_VOLUME: u32 = 0x531;
    pub const ENABLE_AUX_BUS: u32 = 0x532;
    pub const OUTPUT_MODE: u32 = 0x533;
    pub const AUX_FRONT_BYPASS: u32 = 0x534;
    pub const CLIPPING_MODE: u32 = 0x535;
    pub const SURROUND_DEPTH: u32 = 0x536;
    pub const SURROUND_POSITION: u32 = 0x537;
    pub const REAR_RATIO: u32 = 0x538;
    pub const HEADSET: u32 = 0x539;
    pub const OUTPUT_BUFFER_COUNT: u32 = 0x53A;
    pub const SYNC_MODE: u32 = 0x53B;
    pub const DSP_DELAY: u32 = 0x53C;
    pub const DSP_REVERB: u32 = 0x53D;
    pub const DROPPED_FRAMES: u32 = 0x53E;

    pub const DSP_LOAD_COMPONENT: u32 = 0x540;
    pub const DSP_UNLOAD_COMPONENT: u32 = 0x541;
    pub const DSP_HEADPHONE_OUT: u32 = 0x542;
    pub const DSP_FLUSH: u32 = 0x543;
    pub const DSP_INVALIDATE: u32 = 0x544;
    pub const DSP_UNREACHED: u32 = 0x545;

    pub const AAC_INITIALIZE: u32 = 0x550;
    pub const AAC_OPEN: u32 = 0x551;
    pub const AAC_DECODE: u32 = 0x552;
    pub const AAC_WAIT: u32 = 0x553;
    pub const AAC_USED_BYTES: u32 = 0x554;
    pub const AAC_RATE_INDEX: u32 = 0x555;
    pub const AAC_CHANNELS: u32 = 0x556;
    pub const AAC_FINALIZE: u32 = 0x557;
    pub const AAC_STATE_BUFFER: u32 = 0x558;
    pub const AAC_SLEEP: u32 = 0x559;
    pub const AAC_WAKE_UP: u32 = 0x55A;
}

/// CPU ticks in one sound frame.
pub const FRAME_TICKS: u64 = crate::CYCLES_PER_AUDIO_FRAME;
/// a frame counts as due this early, so a sleep that wakes a tick short
/// still renders it.
const TOLERANCE: u64 = FRAME_TICKS / 8;
/// the most frames rendered at once for a sound thread that fell behind.
const MOST_CATCH_UP: u64 = 64;
/// the most output kept for a frontend that is not taking it, a second.
const MAX_OUTPUT: usize = 32_768;

/// where the native layer keeps what the game reads back, in DSP memory
/// (plain guest memory nothing else uses once the DSP is not emulated).
const SCRATCH: Scratch = Scratch {
    statuses: zakuro_common::memory_map::DSP_RAM_VADDR,
    final_samples: zakuro_common::memory_map::DSP_RAM_VADDR + 0x200,
    aux: [zakuro_common::memory_map::DSP_RAM_VADDR + 0x800, zakuro_common::memory_map::DSP_RAM_VADDR + 0x1200],
};

/// whether the native sound layer is on: FATES_NATIVE_AUDIO=0 runs the
/// game's own driver against the emulated DSP instead.
pub fn enabled() -> bool {
    // native-final: the pure build has no emulated DSP
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    return from_env();
}

#[cfg(feature = "emulation")]
fn from_env() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("FATES_NATIVE_AUDIO").map_or(true, |value| value != "0");
        log::info!("audio: the native sound layer is {}", if on { "on" } else { "off (FATES_NATIVE_AUDIO=0, emulated DSP)" });
        on
    })
}

#[derive(Default)]
pub struct NativeSnd {
    pub mixer: mixer::Mixer,
    pub aac: aac::Aac,
    pub initialized: bool,
    /// the guest tick the next frame is due at.
    next_due: u64,
    /// frames the sound thread missed and that were rendered late, and
    /// frames dropped because it was too far behind.
    pub late: u64,
    pub dropped: u64,
    /// frames in which a driver write went around the native layer.
    pub bypassed: u64,
    warned: std::collections::BTreeSet<u32>,
}

/// pulse2: what a rewind snapshot keeps of the native layer (which
/// `save_rest` leaves out): the voices as they were, so the music and the
/// sounds playing at the moment go on from it when it comes back, in step
/// with the restored memory their wave buffers are in. statesnd: a saved
/// state's file keeps it too, in a section of its own (`saved`).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    mixer: mixer::Mixer,
    initialized: bool,
    next_due: u64,
}

/// statesnd: what a saved state's sound section holds.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SavedSnd {
    kept: Kept,
    aac: aac::Saved,
}

/// statesnd: the start of the sound section; a new layout gets a new one,
/// and a section this build cannot read is left out (as a state without
/// one: the voices start again with the game's next sound).
const SAVED_MAGIC: &[u8; 8] = b"SNDNAT01";

impl NativeSnd {
    /// pulse2: the voices and the clock, for a rewind snapshot.
    pub fn keep(&self) -> Kept {
        Kept { mixer: self.mixer.clone(), initialized: self.initialized, next_due: self.next_due }
    }

    /// pulse2: puts back what `keep` kept. The AAC decoder and the counters
    /// stay as they are.
    pub fn put_back(&mut self, kept: &Kept) {
        self.mixer = kept.mixer.clone();
        self.initialized = kept.initialized;
        self.next_due = kept.next_due;
    }

    /// statesnd: the native layer as a saved state's file keeps it (its own
    /// section, after memory: `System::save_state`): the voices, their
    /// wave-buffer queues and play positions (addresses into guest memory,
    /// which the state holds), ADPCM histories, filters, gains and
    /// volumes, what the game changed for the next frame, the clock, and
    /// the AAC decoder's settings.
    pub fn saved(&self) -> Result<Vec<u8>, String> {
        let saved = SavedSnd { kept: self.keep(), aac: self.aac.saved() };
        let out = SAVED_MAGIC.to_vec();
        postcard::to_extend(&saved, out).map_err(|error| format!("the sound could not be written, {error}"))
    }

    /// statesnd: reads what `saved` wrote; nothing changes until
    /// `put_back_saved`.
    pub fn read_saved(bytes: &[u8]) -> Result<SavedSnd, String> {
        let body = bytes.strip_prefix(SAVED_MAGIC.as_slice()).ok_or("it is not the native sound layer's")?;
        postcard::from_bytes(body).map_err(|error| format!("it could not be read, {error}"))
    }

    /// statesnd: the native layer as a saved state had it, after
    /// `load_rest` left it new (and memory is the state's): the voices go
    /// on from the saved moment, their ADPCM decoded again from memory.
    /// The counters stay this run's.
    pub fn put_back_saved(&mut self, saved: SavedSnd, memory: &mut crate::memory::Memory) {
        self.put_back(&saved.kept);
        self.mixer.decode_again(memory);
        self.aac.put_back(saved.aac);
    }

    fn warn_once(&mut self, op: u32, what: &str) {
        if self.warned.insert(op) {
            log::warn!("audio: {what}");
        }
    }
}

fn voice_of(system: &mut System, call: &Call) -> Option<usize> {
    let voice = call.arg(system, 1) as usize & 0xFF;
    (voice < mixer::VOICES).then_some(voice)
}

fn read_wave_buffer(system: &mut System, address: u32, info: Option<u32>) -> Option<Buffer> {
    let memory = &mut system.memory;
    let samples = memory.read32(address);
    let length = memory.read32(address + 4);
    let context = memory.read32(address + 8);
    let looping = memory.read8(address + 0x10) != 0;
    let id = memory.read16(address + 0x12);
    if !memory.is_readable(samples, 1) {
        return None;
    }
    let format = info.map_or(Format::Pcm16, Format::from_info);
    let adpcm = (context != 0 && info.is_none_or(|_| format == Format::Adpcm))
        .then(|| [memory.read16(context + 2) as i16, memory.read16(context + 4) as i16]);
    Some(Buffer {
        address: samples,
        length,
        format,
        stereo: info.is_some_and(|info| info & 3 == 2),
        looping,
        id,
        adpcm,
        from_queue: info.is_none(),
        play_position: 0,
        has_played: false,
    })
}

/// s0, where the hard-float ABI passes a float argument.
fn float_arg(call: &Call) -> f32 {
    // SAFETY: vfp points at the context's 32 VFP words while the code runs
    f32::from_bits(unsafe { *call.ctx.vfp })
}

/// where the driver's pointers into DSP memory lead with the native layer:
/// statuses and final samples to the native mixer's, the rest to a dummy
/// area. Nothing should write there (every Dspsnd method is replaced), but
/// the SDK has code that a compiler folded the driver's writes into; any
/// such write lands harmlessly and is reported (check_dummy).
const DUMMY: u32 = zakuro_common::memory_map::DSP_RAM_VADDR + 0x4000;
const DUMMY_MASTER: u32 = DUMMY + 0x100;
const DUMMY_CHANNELS: u32 = DUMMY + 0x400;
const DUMMY_COEFFICIENTS: u32 = DUMMY + 0x1600;

/// jpn: the Japanese build's Dspsnd (another nn::snd release) has 12 more
/// bytes before its output capture pointer, so every field from +0xC on is
/// 0xC further than the USA build's (see overrides/snd_native.c, `D`).
fn field(offset: u32) -> u32 {
    if offset >= 0xC && crate::region::is_jpn() { offset + 0xC } else { offset }
}

fn point_driver_tables(system: &mut System, driver: u32) {
    let m = &mut system.memory;
    let driver = driver.wrapping_add(field(0x1000) - 0x1000); // jpn: every table below is past +0xC
    for region in 0..2u32 {
        m.write32(driver + 0x1090 + region * 4, DUMMY);
        m.write32(driver + 0x1098 + region * 4, DUMMY_MASTER);
        m.write32(driver + 0x10A0 + region * 4, DUMMY + 0x200);
        m.write32(driver + 0x12F8 + region * 4, SCRATCH.final_samples);
        m.write32(driver + 0x1300 + region * 4, DUMMY + 0x1A00);
        m.write32(driver + 0x1308 + region * 4, DUMMY + 0x1A00);
        for voice in 0..mixer::VOICES as u32 {
            m.write32(driver + 0x10A8 + region * 0x60 + voice * 4, DUMMY_CHANNELS + voice * 0xC0);
            m.write32(driver + 0x1168 + region * 0x60 + voice * 4, SCRATCH.statuses + voice * 12);
            m.write32(driver + 0x1228 + region * 0x60 + voice * 4, DUMMY_COEFFICIENTS + voice * 0x20);
        }
    }
}

/// reports driver writes that went around the native layer.
fn check_dummy(system: &mut System) {
    let mut found = Vec::new();
    for voice in 0..mixer::VOICES as u32 {
        let at = DUMMY_CHANNELS + voice * 0xC0;
        let dirty = system.memory.read32(at);
        if dirty != 0 {
            found.push(format!("voice {voice} {dirty:08X}"));
            system.memory.write32(at, 0);
        }
    }
    let dirty = system.memory.read32(DUMMY_MASTER);
    if dirty != 0 {
        found.push(format!("master {dirty:08X}"));
        system.memory.write32(DUMMY_MASTER, 0);
    }
    if !found.is_empty() {
        let snd = &mut system.services.dsp.native;
        snd.bypassed += 1;
        if snd.bypassed <= 5 {
            log::warn!("audio: driver writes that bypassed the native layer: {}", found.join(", "));
        }
    }
}

/// a driver the native layer did not initialize, as after loading a saved
/// state that has no sound section (one saved before statesnd): it takes it
/// over from here, with no voice playing until the game starts them again.
fn adopt(system: &mut System, now: u64, driver: u32) {
    let snd = &mut system.services.dsp.native;
    if !snd.initialized {
        // the driver's aux buffer pointers, as Initialize sets them
        for (offset, address) in [(0x12E8, SCRATCH.aux[0]), (0x12EC, SCRATCH.aux[1]), (0x12F0, SCRATCH.aux[0]), (0x12F4, SCRATCH.aux[1])] {
            system.memory.write32(driver + field(offset), address); // jpn
        }
        point_driver_tables(system, driver);
        let snd = &mut system.services.dsp.native;
        snd.initialized = true;
        snd.mixer = mixer::Mixer::default();
        // the master volume the game set long ago, until it sets it again
        snd.mixer.pending_master.master_volume = Some(1.0);
        snd.next_due = now;
        log::warn!("audio: the native sound layer took over a running driver (a loaded state?); sounds already playing are lost");
    }
}

/// renders one frame and sends it to the speakers (and the recorder).
fn render(system: &mut System, tick: u64) -> mixer::Frame {
    let frame = system.services.dsp.native.mixer.render(&mut system.memory, SCRATCH);
    wav::record(&frame, tick);
    let output = &mut system.services.dsp.output;
    output.extend_from_slice(&frame);
    if output.len() > MAX_OUTPUT {
        let excess = output.len() - MAX_OUTPUT;
        output.drain(..excess);
    }
    frame
}

/// the native sound calls, see overrides/snd_native.c. true when handled.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    if number == op::TEST_LABEL {
        return test_label(system, call);
    }
    if !enabled() {
        return false;
    }
    // SAFETY: the context is the one the code is running on
    let now = unsafe { crate::recompiled::ticks_of(call.ctx) };
    let name: &'static str = match number {
        op::ENABLED => return true,
        op::INITIALIZE => {
            point_driver_tables(system, call.ctx.r[0]);
            let snd = &mut system.services.dsp.native;
            let waking = call.ctx.r[1] != 0;
            if !waking {
                snd.mixer = mixer::Mixer::default();
            }
            snd.initialized = true;
            snd.next_due = now + FRAME_TICKS;
            log::info!("audio: the native sound layer took over nn::snd's DSP driver{}", if waking { " (wake up)" } else { "" });
            call.ctx.r[0] = SCRATCH.aux[0];
            call.ctx.r[1] = SCRATCH.aux[1];
            "snd.Initialize"
        }
        op::FINALIZE => {
            system.services.dsp.native.initialized = false;
            call.ret(0);
            "snd.Finalize"
        }
        op::WAIT => {
            adopt(system, now, call.ctx.r[0]);
            let snd = &system.services.dsp.native;
            let ns = if now + TOLERANCE >= snd.next_due {
                0
            } else {
                let ticks = snd.next_due - now;
                (ticks as u128 * 1_000_000_000).div_ceil(crate::kernel::thread::CPU_CLOCK_HZ as u128) as u64 + 1
            };
            call.ctx.r[0] = ns as u32;
            call.ctx.r[1] = (ns >> 32) as u32;
            "snd.WaitPipe"
        }
        op::SYNC => {
            adopt(system, now, call.ctx.r[0]);
            let snd = &mut system.services.dsp.native;
            if now + TOLERANCE < snd.next_due {
                call.ret(0);
            } else {
                let mut due = 1 + now.saturating_sub(snd.next_due) / FRAME_TICKS;
                if due > MOST_CATCH_UP {
                    snd.dropped += due - MOST_CATCH_UP;
                    log::debug!("audio: the sound thread was {due} frames behind, dropping {}", due - MOST_CATCH_UP);
                    due = MOST_CATCH_UP;
                    snd.next_due = now - (MOST_CATCH_UP - 1) * FRAME_TICKS;
                }
                snd.late += due - 1;
                // statuses across the frames rendered: the latest ones, with
                // "a buffer started" kept from any of them
                let mut started = [false; mixer::VOICES];
                check_dummy(system);
                let first = system.services.dsp.native.next_due;
                for i in 0..due {
                    render(system, first + i * FRAME_TICKS);
                    for (flag, status) in started.iter_mut().zip(&system.services.dsp.native.mixer.statuses) {
                        *flag |= status.buffer_update;
                    }
                }
                let snd = &mut system.services.dsp.native;
                snd.next_due += due * FRAME_TICKS;
                if due > 1 {
                    for (index, flag) in started.iter().enumerate() {
                        let mut status = snd.mixer.statuses[index];
                        status.buffer_update = *flag;
                        system.memory.write_bytes(SCRATCH.statuses + index as u32 * 12, &status.bytes());
                    }
                }
                call.ctx.r[0] = due as u32;
                call.ctx.r[1] = SCRATCH.statuses;
                call.ctx.r[2] = SCRATCH.final_samples;
            }
            "snd.SyncFrameData"
        }
        op::SEND => {
            system.services.dsp.native.mixer.aux_processed = true;
            "snd.SendParameter"
        }

        op::CHANNEL_INITIALIZE..=op::CHANNEL_ASSIGN_PCM => {
            let Some(voice) = voice_of(system, call) else {
                call.ret(0);
                return true;
            };
            let (a2, a3) = (call.arg(system, 2), call.arg(system, 3));
            let mut ok = true;
            let name = match number {
                op::CHANNEL_INITIALIZE => {
                    // applied first in the frame, as the firmware's dirty flags are
                    system.services.dsp.native.mixer.pending[voice].reset = true;
                    "snd.InitializeChannelParameters"
                }
                op::CHANNEL_PLAY_START => {
                    system.services.dsp.native.mixer.pending[voice].enable = Some(true);
                    "snd.SetChannelPlayStart"
                }
                op::CHANNEL_PLAY_STOP => {
                    system.services.dsp.native.mixer.pending[voice].enable = Some(false);
                    "snd.SetChannelPlayStop"
                }
                op::CHANNEL_SYNC_COUNT => {
                    system.services.dsp.native.mixer.pending[voice].sync_count = Some(a2 as u16);
                    "snd.SetChannelSyncCount"
                }
                op::CHANNEL_TIMER => {
                    let rate = float_arg(call);
                    system.services.dsp.native.mixer.pending[voice].rate = Some(rate);
                    "snd.SetChannelTimer"
                }
                op::CHANNEL_RIM => {
                    system.services.dsp.native.mixer.pending[voice].interpolation = Some(a2 as u8);
                    "snd.SetChannelRIM"
                }
                op::CHANNEL_MIX => {
                    let mut gain = [[0f32; 4]; 3];
                    for (i, g) in gain.iter_mut().flatten().enumerate() {
                        *g = f32::from_bits(system.memory.read32(a2 + i as u32 * 4));
                    }
                    system.services.dsp.native.mixer.pending[voice].gain = Some(gain);
                    "snd.SetChannelMix"
                }
                op::CHANNEL_ADPCM_PARAM => {
                    let mut coefficients = [0i16; 16];
                    for (i, c) in coefficients.iter_mut().enumerate() {
                        *c = system.memory.read16(a2 + i as u32 * 2) as i16;
                    }
                    system.services.dsp.native.mixer.pending[voice].coefficients = Some(coefficients);
                    "snd.SetChannelAdpcmParam"
                }
                op::CHANNEL_FILTER_TYPE => {
                    system.services.dsp.native.mixer.pending[voice].filters = Some(a2 as u16);
                    "snd.SetChannelIirFilterType"
                }
                op::CHANNEL_FILTER_MONO => {
                    system.services.dsp.native.mixer.pending[voice].simple = Some([a2 as i16, a3 as i16]);
                    "snd.SetChannelIIRFilter_Mono"
                }
                op::CHANNEL_FILTER_BIQUAD => {
                    let (a4, a5, a6) = (call.arg(system, 4), call.arg(system, 5), call.arg(system, 6));
                    system.services.dsp.native.mixer.pending[voice].biquad =
                        Some([a2 as i16, a3 as i16, a4 as i16, a5 as i16, a6 as i16]);
                    "snd.SetChannelIIRFilter_Biquad"
                }
                op::CHANNEL_RESET_NEXT => {
                    // the queue is cleared before this frame's buffers go in
                    system.services.dsp.native.mixer.pending[voice].partial_reset = true;
                    "snd.ResetChannelNextBuffer"
                }
                op::CHANNEL_APPEND_NEXT => match read_wave_buffer(system, a2, None) {
                    Some(buffer) => {
                        // one of four slots, the last write to a slot wins
                        system.services.dsp.native.mixer.pending[voice].queue[a3 as usize & 3] = Some(buffer);
                        "snd.AppendChannelNextBuffer"
                    }
                    None => {
                        ok = false;
                        "snd.AppendChannelNextBuffer"
                    }
                },
                op::CHANNEL_UPDATE_NEXT => {
                    let id = system.memory.read16(a2 + 0x12);
                    if system.services.dsp.native.mixer.voice_buffer_id(voice) == Some(id) {
                        let length = system.memory.read32(a2 + 4);
                        let looping = system.memory.read8(a2 + 0x10) != 0;
                        system.services.dsp.native.mixer.pending[voice].partial = Some((length, looping));
                    }
                    "snd.UpdateChannelNextBuffer"
                }
                op::CHANNEL_ASSIGN_PCM => match read_wave_buffer(system, a2, Some(a3 & 0xFFFF)) {
                    Some(buffer) => {
                        system.services.dsp.native.mixer.pending[voice].embedded = Some(buffer);
                        "snd.AssignPCM"
                    }
                    None => {
                        ok = false;
                        "snd.AssignPCM"
                    }
                },
                _ => unreachable!(),
            };
            call.ret(ok as u32);
            name
        }

        op::MASTER_VOLUME => {
            system.services.dsp.native.mixer.pending_master.master_volume = Some(float_arg(call));
            "snd.SetMasterVolume"
        }
        op::AUX_RETURN_VOLUME => {
            let bus = call.arg(system, 1) as usize;
            if bus < 2 {
                system.services.dsp.native.mixer.pending_master.aux_return[bus] = Some(float_arg(call));
            }
            "snd.SetAuxReturnVolume"
        }
        op::ENABLE_AUX_BUS => {
            let (bus, on) = (call.arg(system, 1) as usize, call.arg(system, 2) & 0xFF != 0);
            if bus < 2 {
                system.services.dsp.native.mixer.pending_master.aux_enable[bus] = Some(on);
            }
            call.ret(1);
            "snd.EnableAuxBus"
        }
        op::OUTPUT_MODE => {
            let mode = call.arg(system, 1) & 0xFFFF;
            // surround comes out as stereo
            system.services.dsp.native.mixer.pending_master.mono = Some(mode == 0);
            call.ret(1);
            "snd.SetSoundOutputMode"
        }
        op::AUX_FRONT_BYPASS..=op::SYNC_MODE => {
            call.ret(1);
            match number {
                op::AUX_FRONT_BYPASS => "snd.SetAuxFrontBypass",
                op::CLIPPING_MODE => "snd.SetClippingMode",
                op::SURROUND_DEPTH => "snd.SetSurroundDepth",
                op::SURROUND_POSITION => "snd.SetSurroundSpeakerPosition",
                op::REAR_RATIO => "snd.SetRearRatio",
                op::HEADSET => "snd.SetIsHeadsetConnected",
                op::OUTPUT_BUFFER_COUNT => "snd.SetOutputBufferCount",
                _ => "snd.SetSyncMode",
            }
        }
        op::DSP_DELAY | op::DSP_REVERB => {
            system.services.dsp.native.warn_once(number, "the game set a DSP-side effect (delay/reverb), which the native mixer does not apply");
            call.ret(1);
            if number == op::DSP_DELAY { "snd.SetDspDelayEffect" } else { "snd.SetDspReverbEffect" }
        }
        op::DROPPED_FRAMES => {
            call.ret(system.services.dsp.native.dropped.min(0xFFFF) as u32);
            "snd.GetDroppedFrameCount"
        }

        op::DSP_LOAD_COMPONENT => {
            // the firmware is not needed: say it loaded
            let loaded = call.arg(system, 5);
            if loaded != 0 {
                system.memory.write8(loaded, 1);
            }
            log::info!("audio: the DSP firmware component is not loaded, the native sound layer plays instead");
            call.wait = Some(crate::pcos::svc::REQUEST_TICKS);
            call.ret(0);
            "dsp.LoadComponent"
        }
        op::DSP_UNLOAD_COMPONENT => {
            call.ret(0);
            "dsp.UnloadComponent"
        }
        op::DSP_HEADPHONE_OUT => {
            call.ret(0);
            "dsp.ForceHeadphoneOut"
        }
        op::DSP_FLUSH | op::DSP_INVALIDATE => {
            // the host's caches are coherent; with console timing the call
            // takes as long as the dsp::DSP request did
            call.wait = Some(crate::pcos::svc::REQUEST_TICKS);
            call.ret(0);
            if number == op::DSP_FLUSH { "dsp.FlushDataCache" } else { "dsp.InvalidateDataCache" }
        }
        op::DSP_UNREACHED => {
            let address = call.ctx.r[12];
            system.services.dsp.native.warn_once(address, &format!("a DSP call the native layer replaces was still reached (0x{address:08X}); emulating it"));
            return false;
        }

        op::AAC_INITIALIZE | op::AAC_OPEN => {
            if number == op::AAC_INITIALIZE {
                let form = call.arg(system, 0);
                system.services.dsp.native.aac.initialize(form);
            }
            call.ret(1);
            if number == op::AAC_INITIALIZE { "aacdec.Initialize" } else { "aacdec.Open" }
        }
        op::AAC_DECODE => {
            let (source, left, right, size) = (call.arg(system, 0), call.arg(system, 1), call.arg(system, 2), call.arg(system, 3));
            let ok = system.services.dsp.native.aac.decode(&mut system.memory, source, left, right, size);
            call.ret(ok as u32);
            "aacdec.Decode"
        }
        op::AAC_WAIT => "aacdec.Wait",
        op::AAC_USED_BYTES => {
            call.ret(system.services.dsp.native.aac.used);
            "aacdec.GetUsedBytes"
        }
        op::AAC_RATE_INDEX => {
            call.ret(system.services.dsp.native.aac.rate_index as u32);
            "aacdec.GetSampleRate"
        }
        op::AAC_CHANNELS => {
            call.ret(system.services.dsp.native.aac.channels);
            "aacdec.GetChannelConfig"
        }
        op::AAC_FINALIZE => {
            system.services.dsp.native.aac.finalize();
            call.ret(0);
            "aacdec.Finalize"
        }
        op::AAC_STATE_BUFFER => "aacdec.SetStateBuffer",
        op::AAC_SLEEP | op::AAC_WAKE_UP => {
            call.ret(1);
            if number == op::AAC_SLEEP { "aacdec.Sleep" } else { "aacdec.WakeUp" }
        }
        _ => return false,
    };
    stats::native(name);
    if trace() {
        let snd = &system.services.dsp.native;
        eprintln!("snd-trace {now} tls={:08X} lr={:08X} {name} r0={:08X} r1={:08X} due={} frames={}", call.ctx.tls, call.ctx.r[14], call.ctx.r[0], call.ctx.r[1], snd.next_due, snd.mixer.frames);
        if number == op::SYNC {
            for (i, st) in snd.mixer.statuses.iter().enumerate() {
                if st.enabled || st.buffer_update {
                    eprintln!("snd-trace   voice {i}: {st:?}");
                }
            }
        }
        if matches!(number, op::CHANNEL_APPEND_NEXT | op::CHANNEL_ASSIGN_PCM | op::CHANNEL_SYNC_COUNT | op::CHANNEL_UPDATE_NEXT) {
            let a2 = call.arg(system, 2);
            let a3 = call.arg(system, 3);
            let (addr, len, id) = (system.memory.read32(a2), system.memory.read32(a2 + 4), system.memory.read16(a2 + 0x12));
            eprintln!("snd-trace   args a2={a2:08X} a3={a3:08X} wb addr={addr:08X} len={len} id={id}");
        }
    }
    true
}

/// FATES_AUDIO_TRACE=1 logs every native sound call (debugging).
fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_AUDIO_TRACE").is_ok_and(|v| v == "1"))
}

/// FATES_AUDIO_TEST=label@frame,...: the label of a sound to play now, for
/// reaching sounds no replay reaches (the AAC voice clips); in either route.
fn test_label(system: &mut System, call: &mut Call) -> bool {
    static TESTS: std::sync::Mutex<Option<Vec<(String, u64)>>> = std::sync::Mutex::new(None);
    let mut tests = TESTS.lock().unwrap();
    let tests = tests.get_or_insert_with(|| {
        std::env::var("FATES_AUDIO_TEST").unwrap_or_default().split(',').filter_map(|item| {
            let (label, frame) = item.trim().rsplit_once('@')?;
            Some((label.to_owned(), frame.parse().ok()?))
        }).collect()
    });
    let Some(index) = tests.iter().position(|(_, frame)| *frame <= system.frames) else {
        call.ctx.r[0] = 0;
        return true;
    };
    let (label, _) = tests.remove(index);
    let address = zakuro_common::memory_map::DSP_RAM_VADDR + 0x6000;
    let mut bytes = label.clone().into_bytes();
    bytes.push(0);
    system.memory.write_bytes(address, &bytes);
    log::info!("audio: test: playing {label} at frame {}", system.frames);
    call.ctx.r[0] = address;
    true
}

/// a summary line for the end of a run (with FATES_AUDIO_TRACE, the guest
/// threads too).
pub fn summary(system: &System) -> String {
    let snd = &system.services.dsp.native;
    let mut out = format!(
        "audio: native frames {}, late {}, dropped {}, bypassing writes {}, AAC frames {} ({} failed), next frame due at {}, clock {}",
        snd.mixer.frames, snd.late, snd.dropped, snd.bypassed, snd.aac.frames, snd.aac.failures, snd.next_due, system.cpu.cycles
    );
    if trace() {
        for t in &system.kernel.threads {
            out += &format!(
                "
  thread {} '{}' prio {} {:?} pc {:08X} lr {:08X} tls {:08X} wake {:?} addr {:?} {:?}",
                t.id, t.name, t.priority, t.status, t.context.regs[15], t.context.regs[14], t.tls, t.wakeup_at, t.wait_address, t.wait_syscall
            );
        }
    }
    out
}

/// native-final: a saved state taken while the emulated DSP ran (states from
/// before native audio, `services.dsp.running`) has the game's sound thread
/// waiting on the DSP's end-of-frame interrupt. The pure build has no
/// emulated DSP, so the native layer raises that interrupt itself, at the
/// same audio-frame boundaries, as the DSP firmware did (the requested
/// semaphore mask reported back, the semaphore and interrupt events
/// signalled): the thread wakes, its driver calls reach this layer, which
/// adopts it (`adopt`). Nothing is mixed here; sound comes from the native
/// mixer as after any loaded state.
pub fn legacy_dsp_interrupt(system: &mut System) {
    static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !NOTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        log::info!("audio: the loaded state has the emulated DSP running; its interrupt is raised natively until the native layer takes the driver over");
    }
    let dsp = &mut system.services.dsp;
    dsp.semaphore_value = dsp.semaphore_mask;
    let mut events: Vec<_> = dsp.semaphore_event_object.into_iter().collect();
    events.extend(dsp.interrupt_events.iter().copied());
    for object in events {
        crate::pcos::sched::signal_event(system, object);
    }
}
