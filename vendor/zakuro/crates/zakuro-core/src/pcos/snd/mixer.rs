//! audio: the native mixer, the PC's own implementation of what the 3DS's
//! audio DSP does for nn::snd: 24 voices that play wave buffers (PCM8,
//! PCM16, DSP-ADPCM) from guest memory at a rate, through optional
//! one-pole and biquad filters, into three four-channel mixes (main, aux A,
//! aux B), which a final mix brings down to stereo at the master and aux
//! return volumes.
//!
//! It is driven by the nn::snd driver's own calls (Dspsnd::SetChannel*,
//! AssignPCM, AppendChannelNextBuffer, SetMasterVolume, ...), which
//! overrides/snd_native.c forwards here as plain arguments: nothing reads
//! or writes the DSP's shared-memory structures. What a call changes waits
//! in `Pending` until the next sound frame and is applied in the order the
//! DSP firmware applies its dirty flags (reset first, then the queue), so
//! the order the game makes its calls in within a frame never matters,
//! exactly as on the console. The voices report back the same status words
//! the firmware does (enabled, buffer started, sync count, position,
//! current and last buffer id), which nn::snd's own VoiceManager::
//! UpdateStatus turns into wave-buffer states.

use serde::{Deserialize, Serialize};

use crate::memory::Memory;

pub const VOICES: usize = 24;
/// samples in one sound frame, 160 at 32728 Hz, every 4.889 ms.
pub const FRAME_SAMPLES: usize = 160;

pub type Frame = [[i16; 2]; FRAME_SAMPLES];
/// a four-channel mix (front left, front right, rear left, rear right),
/// channel after channel, as nn::snd's AuxBusData lays it out.
pub type QuadFrame = [[i32; FRAME_SAMPLES]; 4];

/// how far through a frame each sample is, for gains that fade over it.
const PROGRESS: [f32; FRAME_SAMPLES] = {
    let mut progress = [0.0; FRAME_SAMPLES];
    let mut i = 0;
    while i < FRAME_SAMPLES {
        progress[i] = i as f32 / (FRAME_SAMPLES - 1) as f32;
        i += 1;
    }
    progress
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Format {
    Pcm8,
    #[default]
    Pcm16,
    Adpcm,
}

impl Format {
    /// from the low bits of nn::snd's DspsndAudioInfo: bits 2-3.
    pub fn from_info(info: u32) -> Format {
        match (info >> 2) & 3 {
            0 => Format::Pcm8,
            2 => Format::Adpcm,
            _ => Format::Pcm16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Interpolation {
    /// the firmware's polyphase filter, played as linear (as the emulated
    /// route does, for comparable output).
    #[default]
    Polyphase,
    Linear,
    None,
}

/// a wave buffer as nn::snd hands it over, its samples left in guest memory.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Buffer {
    /// guest virtual address of the samples.
    pub address: u32,
    /// in samples.
    pub length: u32,
    pub format: Format,
    pub stereo: bool,
    pub looping: bool,
    pub id: u16,
    /// the ADPCM history (yn1, yn2) to start from, when the game gave one.
    pub adpcm: Option<[i16; 2]>,
    /// buffers from the queue report when they start, the first one does not.
    pub from_queue: bool,
    pub play_position: u32,
    pub has_played: bool,
}

/// y = b0 x + a1 y1, 15 fraction bits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SimpleFilter {
    b0: i32,
    a1: i32,
    y1: [i32; 2],
}

impl Default for SimpleFilter {
    fn default() -> Self {
        SimpleFilter { b0: 1 << 15, a1: 0, y1: [0; 2] }
    }
}

/// the second-order filter, 14 fraction bits, feedback given negated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BiquadFilter {
    b: [i32; 3],
    a: [i32; 2],
    x: [[i32; 2]; 2],
    y: [[i32; 2]; 2],
}

impl Default for BiquadFilter {
    fn default() -> Self {
        BiquadFilter { b: [1 << 14, 0, 0], a: [0; 2], x: [[0; 2]; 2], y: [[0; 2]; 2] }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Filters {
    simple: Option<SimpleFilter>,
    biquad: Option<BiquadFilter>,
}

impl Filters {
    fn process(&mut self, samples: &mut [[i16; 2]]) {
        if self.simple.is_none() && self.biquad.is_none() {
            return;
        }
        for sample in samples {
            for (channel, out) in sample.iter_mut().enumerate() {
                let mut value = *out as i32;
                if let Some(f) = &mut self.simple {
                    value = ((f.b0 * value + f.a1 * f.y1[channel]) >> 15).clamp(-32768, 32767);
                    f.y1[channel] = value;
                }
                if let Some(f) = &mut self.biquad {
                    let x0 = value;
                    value = ((f.b[0] * x0 + f.b[1] * f.x[0][channel] + f.b[2] * f.x[1][channel]
                        + f.a[0] * f.y[0][channel]
                        + f.a[1] * f.y[1][channel])
                        >> 14)
                        .clamp(-32768, 32767);
                    f.x[1][channel] = f.x[0][channel];
                    f.x[0][channel] = x0;
                    f.y[1][channel] = f.y[0][channel];
                    f.y[0][channel] = value;
                }
                *out = value as i16;
            }
        }
    }
}

/// what the game changed on a voice since the last frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pending {
    pub reset: bool,
    pub partial_reset: bool,
    pub enable: Option<bool>,
    pub sync_count: Option<u16>,
    pub rate: Option<f32>,
    pub interpolation: Option<u8>,
    pub coefficients: Option<[i16; 16]>,
    pub gain: Option<[[f32; 4]; 3]>,
    pub filters: Option<u16>,
    pub simple: Option<[i16; 2]>,
    /// b0, b1, b2, a1, a2.
    pub biquad: Option<[i16; 5]>,
    /// UpdateChannelNextBuffer: the playing buffer's new length and loop flag.
    pub partial: Option<(u32, bool)>,
    pub embedded: Option<Buffer>,
    pub queue: [Option<Buffer>; 4],
}

impl Pending {
    fn any(&self) -> bool {
        self.reset
            || self.partial_reset
            || self.enable.is_some()
            || self.sync_count.is_some()
            || self.rate.is_some()
            || self.interpolation.is_some()
            || self.coefficients.is_some()
            || self.gain.is_some()
            || self.filters.is_some()
            || self.simple.is_some()
            || self.biquad.is_some()
            || self.partial.is_some()
            || self.embedded.is_some()
            || self.queue.iter().any(Option::is_some)
    }
}

/// an ADPCM buffer's bytes and what decoding them started from, so a
/// looping buffer is decoded once.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdpcmSource {
    /// statesnd: not in a saved state (the bytes are in guest memory, which
    /// the state holds); `Voice::decode_again` reads them back
    #[serde(skip)]
    data: Vec<u8>,
    count: usize,
    coefficients: [i16; 16],
    start: [i16; 2],
    end: [i16; 2],
}

/// statesnd: a voice is part of a saved state (`NativeSnd::saved`), all
/// but its last frame's samples and its read buffer, which every frame it
/// plays makes again before using them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    pub enabled: bool,
    sync_count: u16,
    rate: f64,
    format: Format,
    stereo: bool,
    interpolation: Interpolation,
    filters: Filters,
    /// how loud the voice is in each channel of each of the three mixes.
    gain: [[f32; 4]; 3],
    /// the gains each mix fades from over this frame, after they changed.
    ramp: [Option<[f32; 4]>; 3],
    coefficients: [i16; 16],
    history: [i16; 2],
    queue: Vec<Buffer>,
    playing: Option<Buffer>,
    /// statesnd: not in a saved state (a whole track can be one ADPCM
    /// buffer, megabytes decoded); `decode_again` makes it again
    #[serde(skip)]
    decoded: Vec<i16>,
    decoded_from: Option<AdpcmSource>,
    remaining: u32,
    length: u32,
    fraction: f64,
    current_buffer_id: u16,
    last_buffer_id: u16,
    buffer_update: bool,
    position: u32,
    #[serde(skip, default = "silent")]
    frame: Frame,
    /// the buffer ran out on the frame's last sample: the next one follows
    /// on from the next frame's first, as on the console.
    ran_out_at_frame_end: bool,
    /// the samples of the buffer being played, read from guest memory once
    /// per frame (PCM) as far as the frame reaches.
    #[serde(skip)]
    scratch: Vec<u8>,
}

/// statesnd: a voice's frame as it comes back from a saved state.
fn silent() -> Frame {
    [[0; 2]; FRAME_SAMPLES]
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            enabled: false,
            sync_count: 0,
            rate: 1.0,
            format: Format::Pcm16,
            stereo: false,
            interpolation: Interpolation::Polyphase,
            filters: Filters::default(),
            gain: [[0.0; 4]; 3],
            ramp: [None; 3],
            coefficients: [0; 16],
            history: [0; 2],
            queue: Vec::new(),
            playing: None,
            decoded: Vec::new(),
            decoded_from: None,
            remaining: 0,
            length: 0,
            fraction: 0.0,
            current_buffer_id: 0,
            last_buffer_id: 0,
            buffer_update: false,
            position: 0,
            frame: [[0; 2]; FRAME_SAMPLES],
            ran_out_at_frame_end: false,
            scratch: Vec::new(),
        }
    }
}

/// the status word a voice reports each frame, as nn::snd reads it
/// (DspsndChannelPlayVars, 12 bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub buffer_update: bool,
    pub sync_count: u16,
    pub position: u32,
    pub current_buffer_id: u16,
    pub last_buffer_id: u16,
}

impl Status {
    pub fn bytes(&self) -> [u8; 12] {
        let mut out = [0u8; 12];
        out[0] = self.enabled as u8;
        out[1] = self.buffer_update as u8;
        out[2..4].copy_from_slice(&self.sync_count.to_le_bytes());
        // the position is a DSP 32-bit word, high half first
        out[4..6].copy_from_slice(&((self.position >> 16) as u16).to_le_bytes());
        out[6..8].copy_from_slice(&(self.position as u16).to_le_bytes());
        out[8..10].copy_from_slice(&self.current_buffer_id.to_le_bytes());
        out[10..12].copy_from_slice(&self.last_buffer_id.to_le_bytes());
        out
    }
}

impl Voice {
    /// applies what the game changed, in the firmware's order.
    fn apply(&mut self, pending: Pending) {
        if pending.reset {
            let scratch = std::mem::take(&mut self.scratch);
            *self = Voice::default();
            self.scratch = scratch;
        }
        if pending.partial_reset {
            self.queue.clear();
            self.ran_out_at_frame_end = false;
        }
        if let Some(enable) = pending.enable {
            self.enabled = enable;
            self.ran_out_at_frame_end = false;
        }
        if let Some(sync) = pending.sync_count {
            self.sync_count = sync;
        }
        if let Some(rate) = pending.rate {
            let rate = rate as f64;
            self.rate = if rate > 0.0 && rate.is_finite() { rate } else { 1.0 };
        }
        if let Some(mode) = pending.interpolation {
            self.interpolation = match mode {
                1 => Interpolation::Linear,
                2 => Interpolation::None,
                _ => Interpolation::Polyphase,
            };
        }
        if let Some(coefficients) = pending.coefficients {
            self.coefficients = coefficients;
        }
        if let Some(gain) = pending.gain {
            for mix in 0..3 {
                self.ramp[mix] = Some(self.gain[mix]);
                self.gain[mix] = gain[mix].map(|g| if g.is_finite() { g } else { 0.0 });
            }
        }
        if let Some(enabled) = pending.filters {
            if enabled & 1 == 0 {
                self.filters.simple = None;
            } else if self.filters.simple.is_none() {
                self.filters.simple = Some(SimpleFilter::default());
            }
            if enabled & 2 == 0 {
                self.filters.biquad = None;
            } else if self.filters.biquad.is_none() {
                self.filters.biquad = Some(BiquadFilter::default());
            }
        }
        if let Some([b0, a1]) = pending.simple {
            let filter = self.filters.simple.get_or_insert_with(SimpleFilter::default);
            filter.b0 = b0 as i32;
            filter.a1 = a1 as i32;
        }
        if let Some([b0, b1, b2, a1, a2]) = pending.biquad {
            let filter = self.filters.biquad.get_or_insert_with(BiquadFilter::default);
            filter.b = [b0 as i32, b1 as i32, b2 as i32];
            filter.a = [a1 as i32, a2 as i32];
        }
        if let Some((length, looping)) = pending.partial {
            let played = self.length - self.remaining;
            self.length = length.max(played);
            self.remaining = self.length - played;
            if let Some(playing) = &mut self.playing {
                playing.length = self.length;
                playing.looping = looping;
            }
        }
        if let Some(buffer) = pending.embedded {
            self.format = buffer.format;
            self.stereo = buffer.stereo;
            self.queue.push(buffer);
        }
        for mut buffer in pending.queue.into_iter().flatten() {
            // queued buffers play in the format the first one set
            buffer.format = self.format;
            buffer.stereo = self.stereo;
            if buffer.length != 0 {
                self.queue.push(buffer);
            }
        }
    }

    fn status(&mut self) -> Status {
        let status = Status {
            enabled: self.enabled,
            buffer_update: self.buffer_update,
            sync_count: self.sync_count,
            position: self.position,
            current_buffer_id: self.current_buffer_id,
            last_buffer_id: self.last_buffer_id,
        };
        self.buffer_update = false;
        status
    }

    /// starts the next buffer, the lowest id waiting.
    fn dequeue(&mut self, memory: &mut Memory) -> bool {
        let Some(index) = (0..self.queue.len()).min_by_key(|&i| self.queue[i].id) else {
            return false;
        };
        let buffer = self.queue[index];
        if buffer.looping {
            self.queue[index].has_played = true;
        } else {
            self.queue.remove(index);
        }
        let start = if buffer.has_played { 0 } else { buffer.play_position.min(buffer.length) };
        self.length = buffer.length;
        self.remaining = buffer.length - start;
        self.position = start;
        self.fraction = 0.0;
        self.current_buffer_id = buffer.id;
        self.last_buffer_id = 0;
        self.buffer_update = buffer.from_queue && !buffer.has_played;
        if let Some(history) = buffer.adpcm {
            self.history = history;
        }
        if buffer.format == Format::Adpcm {
            let bytes = buffer.length.div_ceil(14) * 8;
            let mut data = vec![0u8; bytes as usize];
            memory.read_bytes(buffer.address, &mut data);
            self.decode(&data, buffer.length as usize);
        } else {
            self.forget_decoded();
        }
        self.playing = Some(buffer);
        true
    }

    fn decode(&mut self, data: &[u8], count: usize) {
        let start = self.history;
        if let Some(source) = &self.decoded_from {
            if source.count == count && source.start == start && source.coefficients == self.coefficients && source.data == data {
                self.history = source.end;
                return;
            }
        }
        decode_adpcm(data, count, &self.coefficients, &mut self.history, &mut self.decoded);
        let mut copy = self.decoded_from.take().map(|source| source.data).unwrap_or_default();
        copy.clear();
        copy.extend_from_slice(data);
        self.decoded_from = Some(AdpcmSource { data: copy, count, coefficients: self.coefficients, start, end: self.history });
    }

    /// statesnd: after a saved state, the ADPCM buffer playing is decoded
    /// again from guest memory, from the history it was decoded from, as
    /// `dequeue` decoded it.
    fn decode_again(&mut self, memory: &mut Memory) {
        let (Some(buffer), Some(source)) = (self.playing, self.decoded_from.as_mut()) else { return };
        if buffer.format != Format::Adpcm || !source.data.is_empty() {
            return;
        }
        source.data.resize((source.count as u32).div_ceil(14) as usize * 8, 0);
        memory.read_bytes(buffer.address, &mut source.data);
        let mut history = source.start;
        decode_adpcm(&source.data, source.count, &source.coefficients, &mut history, &mut self.decoded);
        if history != source.end {
            log::warn!("audio: a voice's ADPCM buffer at {:08X} decodes differently from the saved state's", buffer.address);
        }
    }

    fn forget_decoded(&mut self) {
        self.decoded.clear();
        self.decoded_from = None;
    }

    /// plays one frame's worth of input into self.frame.
    fn play_frame(&mut self, memory: &mut Memory) {
        self.frame = [[0; 2]; FRAME_SAMPLES];
        let follows_on = std::mem::take(&mut self.ran_out_at_frame_end) && !self.queue.is_empty();
        if self.remaining == 0 && !follows_on {
            if self.dequeue(memory) {
                // a buffer starting reports this frame and plays from the next
                return;
            }
            // out of buffers: the voice switches itself off and reports the
            // buffer it finished as the last one, with none current
            self.enabled = false;
            self.buffer_update = true;
            self.last_buffer_id = self.current_buffer_id;
            self.current_buffer_id = 0;
            self.position = 0;
            return;
        }
        self.position = self.length - self.remaining;
        let mut output = 0;
        while output < FRAME_SAMPLES {
            if self.remaining == 0 && !self.dequeue(memory) {
                break;
            }
            output = self.play_buffer(memory, output);
        }
        self.ran_out_at_frame_end = self.remaining == 0 && output == FRAME_SAMPLES;
        self.filters.process(&mut self.frame[..output]);
    }

    /// plays the current buffer into the frame from `output` on, until the
    /// frame is full or the buffer ran out; returns where the frame got to.
    fn play_buffer(&mut self, memory: &mut Memory, output: usize) -> usize {
        let Some(buffer) = self.playing else {
            return self.resample(output, 0, |_| [0; 2]);
        };
        let last = buffer.length.saturating_sub(1);
        if buffer.format == Format::Adpcm {
            let decoded = std::mem::take(&mut self.decoded);
            let out = self.resample(output, last, |index| [decoded.get(index as usize).copied().unwrap_or(0); 2]);
            self.decoded = decoded;
            return out;
        }
        // PCM: read the part of the buffer this frame can reach, once
        let size = if buffer.format == Format::Pcm8 { 1 } else { 2 };
        let channels = if buffer.stereo { 2 } else { 1 };
        let at = buffer.length - self.remaining;
        let reach = ((FRAME_SAMPLES - output) as f64 * self.rate + self.fraction).ceil() as u32 + 2;
        let first = at.min(last);
        let end = at.saturating_add(reach).min(buffer.length).max(first + 1);
        let stride = size * channels;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(((end - first) * stride) as usize, 0);
        memory.read_bytes(buffer.address.wrapping_add(first * stride), &mut scratch);
        let sample = |index: u32| -> [i16; 2] {
            let i = (index.clamp(first, end - 1) - first) as usize * stride as usize;
            match (buffer.format, buffer.stereo) {
                (Format::Pcm8, false) => [scratch[i] as i8 as i16 * 256; 2],
                (Format::Pcm8, true) => [scratch[i] as i8 as i16 * 256, scratch[i + 1] as i8 as i16 * 256],
                (_, false) => [i16::from_le_bytes([scratch[i], scratch[i + 1]]); 2],
                (_, true) => [i16::from_le_bytes([scratch[i], scratch[i + 1]]), i16::from_le_bytes([scratch[i + 2], scratch[i + 3]])],
            }
        };
        let out = self.resample(output, last, sample);
        self.scratch = scratch;
        out
    }

    /// steps through the buffer at the voice's rate, reading input samples
    /// by index (clamped to last), with linear interpolation unless the
    /// voice asked for none.
    fn resample(&mut self, mut output: usize, last: u32, mut read: impl FnMut(u32) -> [i16; 2]) -> usize {
        loop {
            let at = self.length - self.remaining;
            let x0 = read(at.min(last));
            self.frame[output] = if self.interpolation == Interpolation::None || self.fraction == 0.0 {
                x0
            } else {
                let x1 = read((at + 1).min(last));
                let t = self.fraction;
                let mix = |a: i16, b: i16| (a as f64 + (b as f64 - a as f64) * t) as i16;
                [mix(x0[0], x1[0]), mix(x0[1], x1[1])]
            };
            output += 1;
            let input = self.fraction + self.rate;
            let consumed = (input as u64).min(self.remaining as u64) as u32;
            // a step past the buffer's end carries over into the next one,
            // as the emulated firmware does
            self.fraction = (input - consumed as f64).max(0.0);
            self.remaining -= consumed;
            if output == FRAME_SAMPLES || self.remaining == 0 {
                return output;
            }
        }
    }

    /// adds what the voice played to the three mixes.
    fn mix_into(&mut self, mixes: &mut [QuadFrame; 3]) {
        let ramps = std::mem::take(&mut self.ramp);
        if !self.enabled {
            return;
        }
        let inputs = [self.frame.map(|[l, _]| l as f32), self.frame.map(|[_, r]| r as f32)];
        for ((mix, gains), from) in mixes.iter_mut().zip(self.gain).zip(ramps) {
            for (channel, out) in mix.iter_mut().enumerate() {
                let (input, gain) = (&inputs[channel & 1], gains[channel]);
                match from.map(|from| from[channel]) {
                    Some(from) if from != gain => {
                        let change = gain - from;
                        for ((out, sample), progress) in out.iter_mut().zip(input).zip(PROGRESS) {
                            *out += ((from + change * progress) * sample) as i32;
                        }
                    }
                    _ if gain == 0.0 => {}
                    _ => {
                        for (out, sample) in out.iter_mut().zip(input) {
                            *out += (gain * sample) as i32;
                        }
                    }
                }
            }
        }
    }
}

/// GC/3DS DSP-ADPCM: frames of 8 bytes, a header (scale, predictor) and 14
/// 4-bit samples, each predicted from the two before.
pub fn decode_adpcm(data: &[u8], count: usize, coefficients: &[i16; 16], history: &mut [i16; 2], samples: &mut Vec<i16>) {
    samples.clear();
    samples.reserve(count);
    let [mut yn1, mut yn2] = history.map(|h| h as i32);
    'frames: for frame in data.chunks(8) {
        let header = frame[0];
        let scale = 1 << (header & 0xF);
        let predictor = ((header >> 4) & 7) as usize;
        let (c1, c2) = (coefficients[predictor * 2] as i32, coefficients[predictor * 2 + 1] as i32);
        for &byte in &frame[1..] {
            for nibble in [byte >> 4, byte & 0xF] {
                if samples.len() == count {
                    break 'frames;
                }
                let xn = (((nibble as i32) << 28) >> 28) * scale;
                let value = (((xn << 11) + 0x400 + c1 * yn1 + c2 * yn2) >> 11).clamp(-32768, 32767);
                yn2 = yn1;
                yn1 = value;
                samples.push(value as i16);
            }
        }
    }
    *history = [yn1 as i16, yn2 as i16];
}

/// the final mix's settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingMaster {
    pub master_volume: Option<f32>,
    pub aux_return: [Option<f32>; 2],
    pub aux_enable: [Option<bool>; 2],
    pub mono: Option<bool>,
}

/// where the mixer puts what the game reads back, in guest memory.
#[derive(Debug, Clone, Copy)]
pub struct Scratch {
    pub statuses: u32,
    pub final_samples: u32,
    pub aux: [u32; 2],
}

/// bytes of one aux bus's four channels.
pub const AUX_BYTES: u32 = 4 * FRAME_SAMPLES as u32 * 4;

// pulse2: a rewind keeps the voices with each moment; statesnd: and a
// saved state's file has them in a section of its own
#[derive(Clone, Serialize, Deserialize)]
pub struct Mixer {
    pub voices: Vec<Voice>,
    pub pending: Vec<Pending>,
    pub pending_master: PendingMaster,
    /// main, aux A return, aux B return.
    volume: [f32; 3],
    /// whether aux A and B go through the game (its effect callbacks).
    aux: [bool; 2],
    mono: bool,
    /// the game ran its effects on the aux buffers since the last frame.
    pub aux_processed: bool,
    /// the statuses of the frame just rendered.
    pub statuses: [Status; VOICES],
    /// frames rendered, voices that played in the last one.
    pub frames: u64,
    pub active: usize,
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            voices: vec![Voice::default(); VOICES],
            pending: vec![Pending::default(); VOICES],
            pending_master: PendingMaster::default(),
            volume: [0.0; 3],
            aux: [false; 2],
            mono: false,
            aux_processed: false,
            statuses: [Status::default(); VOICES],
            frames: 0,
            active: 0,
        }
    }
}

impl Mixer {
    pub fn pending(&mut self, voice: usize) -> Option<&mut Pending> {
        self.pending.get_mut(voice)
    }

    /// statesnd: the voices from a saved state decode their ADPCM again.
    pub fn decode_again(&mut self, memory: &mut Memory) {
        for voice in &mut self.voices {
            voice.decode_again(memory);
        }
    }

    pub fn voice_buffer_id(&self, voice: usize) -> Option<u16> {
        self.statuses.get(voice).map(|s| s.current_buffer_id)
    }

    /// one sound frame: apply what changed, play every voice, mix, and
    /// return what the speakers play. The aux sends go to the guest's aux
    /// buffers for its effects, and what its effects made of the last
    /// frame's sends comes back.
    pub fn render(&mut self, memory: &mut Memory, scratch: Scratch) -> Frame {
        let master = std::mem::take(&mut self.pending_master);
        if let Some(volume) = master.master_volume {
            self.volume[0] = if volume.is_finite() { volume } else { 0.0 };
        }
        for bus in 0..2 {
            if let Some(volume) = master.aux_return[bus] {
                self.volume[bus + 1] = if volume.is_finite() { volume } else { 0.0 };
            }
            if let Some(on) = master.aux_enable[bus] {
                self.aux[bus] = on;
            }
        }
        if let Some(mono) = master.mono {
            self.mono = mono;
        }

        let mut mixes = [[[0i32; FRAME_SAMPLES]; 4]; 3];
        self.active = 0;
        for (index, voice) in self.voices.iter_mut().enumerate() {
            let pending = std::mem::take(&mut self.pending[index]);
            if pending.any() {
                voice.apply(pending);
            }
            if voice.enabled {
                voice.play_frame(memory);
                self.active += 1;
            }
            voice.mix_into(&mut mixes);
            self.statuses[index] = voice.status();
        }

        // aux buses through the game: mix what it returned, hand it the new
        // sends
        let mut out_mixes = [mixes[0], [[0; FRAME_SAMPLES]; 4], [[0; FRAME_SAMPLES]; 4]];
        for bus in 0..2 {
            if self.aux[bus] {
                if self.aux_processed {
                    read_quad(memory, scratch.aux[bus], &mut out_mixes[bus + 1]);
                }
                write_quad(memory, scratch.aux[bus], &mixes[bus + 1]);
            } else {
                out_mixes[bus + 1] = mixes[bus + 1];
            }
        }
        self.aux_processed = false;

        let mut frame = [[0i16; 2]; FRAME_SAMPLES];
        for (volume, mix) in self.volume.iter().zip(&out_mixes) {
            if *volume != 0.0 {
                downmix(&mut frame, mix, *volume, self.mono);
            }
        }

        let mut statuses = [0u8; VOICES * 12];
        for (out, status) in statuses.chunks_exact_mut(12).zip(&self.statuses) {
            out.copy_from_slice(&status.bytes());
        }
        memory.write_bytes(scratch.statuses, &statuses);
        let mut samples = [0u8; FRAME_SAMPLES * 4];
        for (out, [l, r]) in samples.chunks_exact_mut(4).zip(&frame) {
            out[..2].copy_from_slice(&l.to_le_bytes());
            out[2..].copy_from_slice(&r.to_le_bytes());
        }
        memory.write_bytes(scratch.final_samples, &samples);
        self.frames += 1;
        frame
    }
}

fn read_quad(memory: &mut Memory, address: u32, mix: &mut QuadFrame) {
    let mut bytes = [0u8; AUX_BYTES as usize];
    memory.read_bytes(address, &mut bytes);
    for (channel, chunk) in mix.iter_mut().zip(bytes.chunks_exact(FRAME_SAMPLES * 4)) {
        for (value, word) in channel.iter_mut().zip(chunk.chunks_exact(4)) {
            *value = i32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }
    }
}

fn write_quad(memory: &mut Memory, address: u32, mix: &QuadFrame) {
    let mut bytes = [0u8; AUX_BYTES as usize];
    for (channel, chunk) in mix.iter().zip(bytes.chunks_exact_mut(FRAME_SAMPLES * 4)) {
        for (value, word) in channel.iter().zip(chunk.chunks_exact_mut(4)) {
            word.copy_from_slice(&value.to_le_bytes());
        }
    }
    memory.write_bytes(address, &bytes);
}

/// adds a four-channel mix to the stereo output at a volume.
fn downmix(frame: &mut Frame, mix: &QuadFrame, volume: f32, mono: bool) {
    let clamp = |value: f32| (value as i32).clamp(-32768, 32767);
    for (sample, out) in frame.iter_mut().enumerate() {
        let [a, b, c, d] = std::array::from_fn(|channel| mix[channel][sample] as f32 * volume);
        let (left, right) = if mono {
            let m = clamp((a + b + c + d) / 2.0);
            (m, m)
        } else {
            (clamp(a + c), clamp(b + d))
        };
        out[0] = (out[0] as i32 + left).clamp(-32768, 32767) as i16;
        out[1] = (out[1] as i32 + right).clamp(-32768, 32767) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adpcm_decodes_a_known_frame() {
        // scale 1 (header 0x00), predictor 0 with zero coefficients: the
        // samples are the nibbles << 11 >> 11, i.e. the nibbles themselves
        let mut history = [0; 2];
        let mut out = Vec::new();
        decode_adpcm(&[0x00, 0x12, 0x34, 0x56, 0x70, 0xF0, 0, 0], 10, &[0; 16], &mut history, &mut out);
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 7, 0, -1, 0]);
    }

    use crate::memory::{MemoryState, Permission};
    use zakuro_common::memory_map::{DSP_RAM_PADDR, DSP_RAM_VADDR};

    const SAMPLES: u32 = DSP_RAM_VADDR + 0x10000;
    const SCRATCH: Scratch = Scratch {
        statuses: DSP_RAM_VADDR,
        final_samples: DSP_RAM_VADDR + 0x200,
        aux: [DSP_RAM_VADDR + 0x800, DSP_RAM_VADDR + 0x1200],
    };

    fn memory() -> crate::System {
        let mut system = crate::System::new(crate::Config::default());
        system.memory.map(DSP_RAM_VADDR, DSP_RAM_PADDR, 0x80000, Permission::RW, MemoryState::Private);
        system
    }

    fn pcm(system: &mut crate::System, address: u32, samples: &[i16]) {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        system.memory.write_bytes(address, &bytes);
    }

    fn buffer(address: u32, length: u32, id: u16, from_queue: bool) -> Buffer {
        Buffer { address, length, format: Format::Pcm16, stereo: false, looping: false, id, adpcm: None, from_queue, play_position: 0, has_played: false }
    }

    /// a mono PCM16 voice at rate 1 and gain 1 through the main mix at
    /// master volume 1 comes out as it went in, in both channels, from the
    /// frame after the one it starts in; its statuses follow the firmware's.
    #[test]
    fn a_voice_plays_its_buffers_and_reports_them() {
        let mut system = memory();
        let input: Vec<i16> = (0..400).map(|i| (i * 37 % 2000 - 1000) as i16).collect();
        pcm(&mut system, SAMPLES, &input[..200]);
        pcm(&mut system, SAMPLES + 0x1000, &input[200..]);
        let mut mixer = Mixer::default();
        mixer.pending_master.master_volume = Some(1.0);
        let p = &mut mixer.pending[3];
        p.enable = Some(true);
        p.sync_count = Some(7);
        p.gain = Some([[1.0, 1.0, 0.0, 0.0], [0.0; 4], [0.0; 4]]);
        p.embedded = Some(buffer(SAMPLES, 200, 1, false));
        p.queue[0] = Some(buffer(SAMPLES + 0x1000, 200, 2, true));

        // the first frame starts buffer 1: silent, reported current
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert!(frame.iter().all(|s| *s == [0, 0]));
        assert_eq!(mixer.statuses[3], Status { enabled: true, buffer_update: false, sync_count: 7, position: 0, current_buffer_id: 1, last_buffer_id: 0 });
        // then it plays: 160 of buffer 1
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(frame.iter().map(|s| s[0]).collect::<Vec<_>>(), input[..160]);
        assert!(frame.iter().all(|s| s[0] == s[1]));
        // the rest of buffer 1 and the start of buffer 2, which reports
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(frame.iter().map(|s| s[0]).collect::<Vec<_>>(), input[160..320]);
        assert_eq!(mixer.statuses[3].current_buffer_id, 2);
        assert!(mixer.statuses[3].buffer_update, "a queued buffer starting is reported once");
        // the end of buffer 2, then the voice stops itself
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(frame[..80].iter().map(|s| s[0]).collect::<Vec<_>>(), input[320..]);
        assert!(!mixer.statuses[3].buffer_update);
        mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(mixer.statuses[3], Status { enabled: false, buffer_update: true, sync_count: 7, position: 0, current_buffer_id: 0, last_buffer_id: 2 });
        // the status words in guest memory are what nn::snd reads
        let mut words = [0u8; 12];
        system.memory.read_bytes(SCRATCH.statuses + 3 * 12, &mut words);
        assert_eq!(words, mixer.statuses[3].bytes());
    }

    /// what a frame asked for applies in the firmware's order: a reset
    /// asked after a buffer still keeps the buffer.
    #[test]
    fn a_reset_comes_first_whatever_the_call_order() {
        let mut system = memory();
        pcm(&mut system, SAMPLES, &[1000; 400]);
        let mut mixer = Mixer::default();
        mixer.pending_master.master_volume = Some(1.0);
        mixer.pending[0].enable = Some(true);
        mixer.pending[0].gain = Some([[1.0, 1.0, 0.0, 0.0], [0.0; 4], [0.0; 4]]);
        mixer.pending[0].embedded = Some(buffer(SAMPLES, 400, 1, false));
        mixer.pending[0].reset = true;
        mixer.render(&mut system.memory, SCRATCH);
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(frame[0], [1000, 1000]);
    }

    /// a voice at half rate takes two frames per 160 samples, and linear
    /// interpolation puts the halfway samples between.
    #[test]
    fn rate_and_linear_interpolation() {
        let mut system = memory();
        let input: Vec<i16> = (0..200).map(|i| i * 100).collect();
        pcm(&mut system, SAMPLES, &input);
        let mut mixer = Mixer::default();
        mixer.pending_master.master_volume = Some(1.0);
        let p = &mut mixer.pending[0];
        p.enable = Some(true);
        p.rate = Some(0.5);
        p.interpolation = Some(1);
        p.gain = Some([[1.0, 1.0, 0.0, 0.0], [0.0; 4], [0.0; 4]]);
        p.embedded = Some(buffer(SAMPLES, 200, 1, false));
        mixer.render(&mut system.memory, SCRATCH);
        let frame = mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(&frame[..5].iter().map(|s| s[0]).collect::<Vec<_>>(), &[0, 50, 100, 150, 200]);
        assert_eq!(mixer.statuses[0].position, 0);
        mixer.render(&mut system.memory, SCRATCH);
        assert_eq!(mixer.statuses[0].position, 80);
    }

    #[test]
    fn status_position_is_high_half_first() {
        let status = Status { position: 0x0001_0002, ..Status::default() };
        let bytes = status.bytes();
        assert_eq!(&bytes[4..8], &[1, 0, 2, 0]);
    }
}
