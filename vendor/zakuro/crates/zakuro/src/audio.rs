//! the console's sound, played on the host's default output device.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// how much sound to keep waiting, in the console's samples, about 125 ms.
/// more rides out uneven frames, less answers sooner.
const TARGET: usize = 4096;
/// how much a queue that ran dry waits for before it plays again.
const RESUME: usize = TARGET / 4;
/// the queue stretches sound only once it is down to this, about 63 ms.
/// every stretch splices the sound, so smaller shortfalls are left to DRIFT.
const STRETCH_BELOW: usize = TARGET / 2;
/// how much faster or slower than the rates say playback may go to keep
/// the queue near its target, too little to hear.
const DRIFT: f64 = 0.005;

/// the console's samples waiting to be played.
struct Queue {
    samples: VecDeque<[f32; 2]>,
    /// how far the output is between the first two samples.
    fraction: f64,
    /// console samples per output sample.
    step: f64,
    /// ran dry, and waits to fill up before it plays again.
    starved: bool,
    /// the last sample played, which fades out when the queue runs dry
    /// instead of stopping short.
    last: [f32; 2],
    /// times the queue ran dry.
    underruns: u64,
    /// how much a queue that ran dry waits for before it plays again.
    refill: usize,
    /// pulse2: Divine Pulse's time stopping and starting again
    time: TimeFx,
    /// pulse2: FATES_OUTPUT_WAV: the output is pulled by the app, a
    /// presented frame's worth at a time, not by the device
    pulled: bool,
}

/// pulse2: Divine Pulse's sound. When the rewind screen opens, what is
/// queued plays on slowing down to a stop, as a tape stopping (the time
/// stopping, under the picture's wave), and nothing more plays while time
/// is stopped (the game is held). When the screen closes the queue starts
/// over with the sound of the moment time went back to (the music going on
/// from there), fading in.
#[derive(Default)]
struct TimeFx {
    /// winding down: how fast the queued sound plays now (1 to 0), and how
    /// much slower it gets each output sample
    stopping: Option<(f64, f64)>,
    /// stopped: nothing plays and nothing is taken in
    stopped: bool,
    /// fading back in: the gain, and how much it rises each output sample
    fading_in: Option<(f32, f32)>,
}

impl TimeFx {
    fn holds(&self) -> bool {
        self.stopping.is_some() || self.stopped
    }
}

impl Queue {
    /// the next output sample, between two of the console's.
    fn next(&mut self) -> [f32; 2] {
        // pulse2: time winding down, then stopped
        if let Some((rate, slope)) = self.time.stopping {
            if self.samples.len() < 2 || rate <= 0.0 {
                self.time.stopping = None;
                self.time.stopped = true;
            } else {
                let (a, b) = (self.samples[0], self.samples[1]);
                let t = self.fraction as f32;
                let gain = (rate as f32).powf(0.6);
                let out = [(a[0] + (b[0] - a[0]) * t) * gain, (a[1] + (b[1] - a[1]) * t) * gain];
                self.last = out;
                self.fraction += self.step * rate;
                while self.fraction >= 1.0 && self.samples.len() > 1 {
                    self.samples.pop_front();
                    self.fraction -= 1.0;
                }
                self.time.stopping = Some((rate - slope, slope));
                return out;
            }
        }
        if self.time.stopped {
            self.last = self.last.map(|v| v * 0.995);
            return self.last;
        }
        let out = self.next_plain();
        // pulse2: the sound after a rewind fading in
        match self.time.fading_in {
            Some((gain, slope)) if !self.starved => {
                self.time.fading_in = (gain + slope < 1.0).then_some((gain + slope, slope));
                out.map(|v| v * gain)
            }
            _ => out,
        }
    }

    fn next_plain(&mut self) -> [f32; 2] {
        if !self.starved && self.samples.len() < 2 {
            self.starved = true;
            self.underruns += 1;
        }
        if self.starved {
            if self.samples.len() < self.refill {
                self.last = self.last.map(|v| v * 0.995);
                return self.last;
            }
            self.starved = false;
            self.refill = RESUME;
        }
        let (a, b) = (self.samples[0], self.samples[1]);
        let t = self.fraction as f32;
        let out = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        self.last = out;
        // a longer queue plays a little faster, a shorter one slower
        let error = (self.samples.len() as f64 - TARGET as f64) / TARGET as f64;
        self.fraction += self.step * (1.0 + error.clamp(-1.0, 1.0) * DRIFT);
        while self.fraction >= 1.0 && self.samples.len() > 1 {
            self.samples.pop_front();
            self.fraction -= 1.0;
        }
        out
    }

    /// stops it while the game is stopped, without counting that as running
    /// dry, and has it fill all the way up before it plays again, so the
    /// first frames after do not run it dry either.
    fn hold(&mut self) {
        if self.time.holds() {
            return; // pulse2: winding down, or stopped
        }
        self.starved = true;
        self.refill = TARGET;
        self.underruns = 0;
    }

    /// drops what is queued, holding as above, fading out from what played
    /// last.
    fn clear(&mut self) {
        if self.time.holds() {
            return; // pulse2: winding down, or stopped
        }
        self.samples.clear();
        self.fraction = 0.0;
        self.hold();
    }
}

/// the pieces of sound stretching moves around, about 16 ms, and how far
/// apart they go out, half of that.
const GRAIN: usize = 512;
const HOP: usize = GRAIN / 2;
/// how far a piece may shift, about 4 ms, to line up with the one before.
const SEARCH: usize = 128;
/// sound gets at most twice as long.
const MOST_STRETCH: f64 = 2.0;

/// time stretching, sound made longer without its pitch changing, for when
/// the emulation falls behind and the queue would run dry. overlapping
/// pieces of it go out further apart than they came in, each shifted to
/// line up with the one before, which is WSOLA.
struct Stretch {
    /// what came in and has not gone out yet.
    input: Vec<[f32; 2]>,
    /// where in input the next piece is due.
    position: f64,
    /// where the last piece came from, none before the first.
    last: Option<usize>,
    /// the last piece's second half, which the next one's first adds to.
    tail: Vec<[f32; 2]>,
    /// how much longer sound comes out, eased toward what the queue asks.
    factor: f64,
    window: Vec<f32>,
}

impl Stretch {
    fn new() -> Stretch {
        // a periodic Hann window, whose halves add up to one
        let window = (0..GRAIN).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / GRAIN as f32).cos()).collect();
        Stretch { input: Vec::new(), position: 0.0, last: None, tail: vec![[0.0; 2]; HOP], factor: 1.0, window }
    }

    /// forgets what came in, to start over on other sound.
    fn clear(&mut self) {
        self.input.clear();
        self.position = 0.0;
        self.last = None;
        self.tail.fill([0.0; 2]);
        self.factor = 1.0;
    }

    /// stretches samples by about factor, passing out what is ready.
    fn process(&mut self, samples: &[[f32; 2]], factor: f64, out: &mut Vec<[f32; 2]>) {
        self.input.extend_from_slice(samples);
        self.factor += (factor - self.factor) * 0.2;
        // close to not stretching any more, it stops
        if factor == 1.0 && self.factor < 1.001 {
            self.factor = 1.0;
        }
        let mono = |s: [f32; 2]| s[0] + s[1];
        loop {
            // not stretching, each piece follows on from the last, with no
            // search and so no splice
            if let Some(last) = self.last.filter(|_| self.factor == 1.0) {
                self.position = (last + HOP) as f64;
            }
            let due = self.position as usize;
            let natural = self.last.map_or(due, |last| last + HOP);
            if due + SEARCH + GRAIN > self.input.len() || natural + GRAIN > self.input.len() {
                break;
            }
            // the piece around where it is due that most looks like the
            // last one's continuation, which is where it is due when
            // nothing is stretched
            let start = if natural == due {
                due
            } else {
                let template = &self.input[natural..natural + HOP];
                let score = |candidate: usize| {
                    let (mut dot, mut energy) = (0.0f32, 1e-9f32);
                    for (a, &b) in self.input[candidate..candidate + HOP].iter().zip(template) {
                        dot += mono(*a) * mono(b);
                        energy += mono(*a) * mono(*a);
                    }
                    dot / energy.sqrt()
                };
                (due.saturating_sub(SEARCH)..=due + SEARCH)
                    .map(|candidate| (score(candidate), candidate))
                    // a tie, as in silence, goes to the piece nearest the
                    // natural one
                    .max_by(|(a, x), (b, y)| a.total_cmp(b).then(natural.abs_diff(*y).cmp(&natural.abs_diff(*x))))
                    .map_or(due, |(_, candidate)| candidate)
            };
            let piece = &self.input[start..start + GRAIN];
            let (first, second) = piece.split_at(HOP);
            let (rising, falling) = self.window.split_at(HOP);
            for ((tail, sample), w) in self.tail.iter_mut().zip(first).zip(rising) {
                out.push([tail[0] + sample[0] * w, tail[1] + sample[1] * w]);
            }
            for ((tail, sample), w) in self.tail.iter_mut().zip(second).zip(falling) {
                *tail = [sample[0] * w, sample[1] * w];
            }
            self.last = Some(start);
            self.position += HOP as f64 / self.factor;
        }
        // what no piece can reach any more goes
        let used = (self.position as usize).min(self.last.unwrap_or(0)).saturating_sub(SEARCH);
        if used > 0 {
            self.input.drain(..used);
            self.position -= used as f64;
            self.last = self.last.map(|last| last - used);
        }
    }
}

pub struct Audio {
    queue: Arc<Mutex<Queue>>,
    /// 0 to 1, applied as samples come in.
    volume: std::cell::Cell<f32>,
    stretch: std::cell::RefCell<Stretch>,
    /// qol: the game speed: the sound comes in this many times as fast and
    /// is time-stretched (made shorter, at its own pitch) to play in time.
    speed: std::cell::Cell<f64>,
    /// pace: the sound's flow over the last stretch of wall time: samples
    /// the game made, samples queued after stretching, since when.
    flow: std::cell::Cell<(u64, u64, Option<std::time::Instant>)>,
    rate: f64,
    /// pulse2: the output's samples a second
    out_rate: f64,
    /// pulse2: FATES_OUTPUT_WAV, what the speakers play, pulled a presented
    /// frame at a time (see record_frame)
    recording: Option<std::cell::RefCell<OutWav>>,
    _stream: cpal::Stream,
}

impl Audio {
    /// opens the default output for sound made at rate samples a second.
    #[allow(dead_code)] // ui: the app opens the chosen device with open_on
    pub fn open(rate: f64) -> Result<Audio, String> {
        Audio::open_on(rate, "")
    }

    /// ui: opens the output device named, as output_devices lists it, or the
    /// default one when name is empty or no device has it.
    pub fn open_on(rate: f64, name: &str) -> Result<Audio, String> {
        let host = cpal::default_host();
        let named = (!name.is_empty())
            .then(|| host.output_devices().ok()?.find(|device| device.name().is_ok_and(|n| n == name)))
            .flatten();
        if !name.is_empty() && named.is_none() {
            log::warn!("the sound device {name} is not there, playing on the default one");
        }
        let device = match named {
            Some(device) => device,
            None => host.default_output_device().ok_or("there is no sound output device")?,
        };
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let queue = Arc::new(Mutex::new(Queue {
            samples: VecDeque::new(),
            fraction: 0.0,
            step: rate / config.sample_rate.0 as f64,
            starved: true,
            last: [0.0; 2],
            underruns: 0,
            refill: RESUME,
            time: TimeFx::default(),
            pulled: false,
        }));
        let stream = match format {
            SampleFormat::F32 => stream::<f32>(&device, &config, queue.clone()),
            SampleFormat::I16 => stream::<i16>(&device, &config, queue.clone()),
            SampleFormat::U16 => stream::<u16>(&device, &config, queue.clone()),
            SampleFormat::I32 => stream::<i32>(&device, &config, queue.clone()),
            other => return Err(format!("the output takes {other} samples, which Zakuro does not make")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        let speed = std::cell::Cell::new(crate::qol::speed()); // qol
        let out_rate = config.sample_rate.0 as f64;
        // pulse2: FATES_OUTPUT_WAV records the output instead of playing it
        let recording = std::env::var_os("FATES_OUTPUT_WAV").and_then(|path| match OutWav::create(std::path::Path::new(&path), out_rate as u32) {
            Ok(wav) => {
                if let Ok(mut queue) = queue.lock() {
                    queue.pulled = true;
                }
                log::info!("pulse2: recording the output to {}", std::path::Path::new(&path).display());
                Some(std::cell::RefCell::new(wav))
            }
            Err(error) => {
                log::warn!("pulse2: cannot record the output, {error}");
                None
            }
        });
        Ok(Audio { queue, volume: std::cell::Cell::new(1.0), stretch: std::cell::RefCell::new(Stretch::new()), speed, flow: std::cell::Cell::new((0, 0, None)), rate, out_rate, recording, _stream: stream })
    }

    /// how many times the sound ran dry since the last call.
    pub fn take_underruns(&self) -> u64 {
        self.queue.lock().map(|mut queue| std::mem::take(&mut queue.underruns)).unwrap_or(0)
    }

    /// stops the sound while the game is stopped, see Queue::hold.
    pub fn hold(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.hold();
        }
    }

    /// drops the sound queued and half stretched, for a game starting or
    /// stopping, so none of the last one's plays at the start of the next.
    pub fn clear(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        self.stretch.borrow_mut().clear();
    }

    /// pulse2: time stops (Divine Pulse opened): what is queued winds down
    /// to a stop in about twice its length (0.15 to 0.5 s), and nothing new
    /// is taken in until time_resume.
    pub fn time_stop(&self) {
        let Ok(mut queue) = self.queue.lock() else { return };
        let console = queue.step * self.out_rate;
        let queued = queue.samples.len() as f64 / console.max(1.0);
        let length = (2.0 * queued).clamp(0.15, 0.5);
        queue.time.stopping = Some((1.0, 1.0 / (length * self.out_rate)));
        queue.time.stopped = false;
        queue.time.fading_in = None;
        queue.starved = false;
    }

    /// pulse2: time goes on (a rewind made or cancelled): the queue starts
    /// over with what comes next, fading in over 0.3 s.
    pub fn time_resume(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.time = TimeFx { fading_in: Some((0.0, (1.0 / (0.3 * self.out_rate)) as f32)), ..TimeFx::default() };
            queue.clear();
        }
        self.stretch.borrow_mut().clear();
    }

    /// statesnd: a saved state was loaded: what was queued of the moment
    /// before goes, and the loaded moment's sound (the music going on from
    /// it, the state keeping the voices) fades in over 0.3 s, as after a
    /// rewind, rather than starting with a click.
    pub fn state_loaded(&self) {
        self.time_resume();
    }

    /// pulse2: FATES_OUTPUT_WAV: the output of `seconds` (a presented
    /// frame) pulled from the queue and written, as the device would have
    /// played it.
    pub fn record_frame(&self, seconds: f64) {
        let Some(recording) = &self.recording else { return };
        let mut wav = recording.borrow_mut();
        let wanted = self.out_rate * seconds + wav.carry;
        let count = wanted.floor() as usize;
        wav.carry = wanted - count as f64;
        let Ok(mut queue) = self.queue.lock() else { return };
        let samples: Vec<[f32; 2]> = (0..count).map(|_| queue.next()).collect();
        drop(queue);
        wav.write(&samples);
    }

    /// whether the output is recorded (FATES_OUTPUT_WAV).
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    /// the samples recorded so far (FATES_OUTPUT_WAV), none when not.
    pub fn recorded(&self) -> Option<u64> {
        self.recording.as_ref().map(|wav| wav.borrow().samples)
    }

    /// qol: the game speed the sound comes in at.
    pub fn set_speed(&self, speed: f64) {
        self.speed.set(speed.max(1.0));
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume.set(volume.clamp(0.0, 1.0));
    }

    /// queues what the console played, stretched when the queue runs low
    /// so that it plays on rather than stops.
    pub fn push(&self, samples: &[[i16; 2]]) {
        let scale = self.volume.get() / 32768.0;
        let samples: Vec<[f32; 2]> = samples.iter().map(|s| s.map(|v| v as f32 * scale)).collect();
        let Ok(mut queue) = self.queue.lock() else { return };
        if queue.time.holds() {
            return; // pulse2: time stopped
        }
        let before = queue.samples.len();
        enqueue(&mut queue, &mut self.stretch.borrow_mut(), &samples, self.speed.get());
        // pace: what came in and what was queued (trimming aside)
        let added = queue.samples.len().saturating_sub(before) as u64;
        let (fed, queued, since) = self.flow.get();
        self.flow.set((fed + samples.len() as u64, queued + added, since.or_else(|| Some(std::time::Instant::now()))));
    }

    /// pace: every ten seconds or more, how the sound kept up: wall seconds,
    /// the game's sound seconds made in them (its speed against real time),
    /// and how much longer stretching made it (1.0 is none).
    pub fn take_flow(&self) -> Option<(f64, f64, f64)> {
        let (fed, queued, since) = self.flow.get();
        let wall = since?.elapsed().as_secs_f64();
        if wall < 10.0 || fed == 0 {
            return None;
        }
        self.flow.set((0, 0, None));
        Some((wall, fed as f64 / self.rate, queued as f64 / fed as f64))
    }
}

/// pulse2: FATES_OUTPUT_WAV's file: 16-bit stereo at the output's rate, its
/// header brought up to date every second and at the end.
struct OutWav {
    file: std::io::BufWriter<std::fs::File>,
    rate: u32,
    samples: u64,
    since_header: u64,
    carry: f64,
}

impl OutWav {
    fn create(path: &std::path::Path, rate: u32) -> std::io::Result<OutWav> {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        file.write_all(&wav_header(rate, 0))?;
        Ok(OutWav { file, rate, samples: 0, since_header: 0, carry: 0.0 })
    }

    fn write(&mut self, samples: &[[f32; 2]]) {
        use std::io::{Seek, SeekFrom, Write};
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            for v in s {
                bytes.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
        }
        let _ = self.file.write_all(&bytes);
        self.samples += samples.len() as u64;
        self.since_header += samples.len() as u64;
        if self.since_header >= self.rate as u64 {
            self.since_header = 0;
            let _ = self.file.flush();
            let file = self.file.get_mut();
            if let Ok(at) = file.stream_position() {
                let _ = file.seek(SeekFrom::Start(0));
                let _ = file.write_all(&wav_header(self.rate, self.samples));
                let _ = file.seek(SeekFrom::Start(at));
            }
        }
    }
}

impl Drop for OutWav {
    fn drop(&mut self) {
        use std::io::{Seek, SeekFrom, Write};
        let _ = self.file.flush();
        let file = self.file.get_mut();
        let _ = file.seek(SeekFrom::Start(0));
        let _ = file.write_all(&wav_header(self.rate, self.samples));
    }
}

fn wav_header(rate: u32, samples: u64) -> [u8; 44] {
    let data = (samples * 4).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// ui: the names of the sound output devices, for the options.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host.output_devices().map(|devices| devices.filter_map(|device| device.name().ok()).collect()).unwrap_or_default();
    names.dedup();
    names
}

/// adds what the console played to the queue, stretched by how far the
/// queue has run down.
/// qol: at a game speed above 1 the sound is made that many times shorter.
fn enqueue(queue: &mut Queue, stretch: &mut Stretch, samples: &[[f32; 2]], speed: f64) {
    let short = STRETCH_BELOW.saturating_sub(queue.samples.len()) as f64 / STRETCH_BELOW as f64;
    let mut stretched = Vec::with_capacity(samples.len() * 2);
    stretch.process(samples, (1.0 + short).min(MOST_STRETCH) / speed.max(1.0), &mut stretched);
    queue.samples.extend(stretched);
    // far ahead, after a stall on the output side, it drops the oldest
    // rather than lag behind the picture
    if queue.samples.len() > TARGET * 4 {
        let excess = queue.samples.len() - TARGET;
        queue.samples.drain(..excess);
    }
}

fn stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<Queue>>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels as usize;
    let mut boosted = false;
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                // smooth: the callback's thread at the priority sound needs,
                // once (cpal 0.16 means to, but passes a thread id where
                // Windows wants a handle, so its own boost does nothing)
                if !boosted {
                    boosted = true;
                    boost_sound_thread();
                }
                let Ok(mut queue) = queue.lock() else { return };
                for frame in out.chunks_mut(channels) {
                    // pulse2: a recording pulls the queue itself
                    let [left, right] = if queue.pulled { [0.0; 2] } else { queue.next() };
                    match frame {
                        [mono] => *mono = T::from_sample((left + right) / 2.0),
                        [l, r, rest @ ..] => {
                            *l = T::from_sample(left);
                            *r = T::from_sample(right);
                            rest.fill(T::EQUILIBRIUM);
                        }
                        [] => {}
                    }
                }
            },
            |error| log::warn!("sound output, {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// smooth: the calling thread (the sound output's) registered with the
/// multimedia class scheduler as "Pro Audio", which keeps it running ahead
/// of busy normal threads, and at time-critical priority besides.
fn boost_sound_thread() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        #[link(name = "avrt")]
        unsafe extern "system" {
            fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> isize;
        }
        let task: Vec<u16> = "Pro Audio".encode_utf16().chain([0]).collect();
        let mut index = 0u32;
        // SAFETY: plain calls on this thread; the task name is a valid
        // NUL-terminated string. The MMCSS registration lasts as long as the
        // thread does.
        let (mmcss, priority) = unsafe { (AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) != 0, SetThreadPriority(GetCurrentThread(), 15) != 0) };
        log::info!("smooth: the sound thread {} MMCSS Pro Audio, {} time-critical priority", if mmcss { "joined" } else { "could not join" }, if priority { "at" } else { "not at" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(samples: usize, step: f64) -> Queue {
        Queue {
            samples: (0..samples).map(|i| [i as f32, -(i as f32)]).collect(),
            fraction: 0.0,
            step,
            starved: false,
            last: [0.0; 2],
            underruns: 0,
            refill: RESUME,
            time: TimeFx::default(),
            pulled: false,
        }
    }

    /// a tone at 440 Hz, the console's rate.
    fn tone(count: usize) -> Vec<[f32; 2]> {
        (0..count)
            .map(|i| {
                let v = (std::f32::consts::TAU * 440.0 * i as f32 / 32728.0).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    /// how many times a sound crosses zero going up, which tells its pitch.
    fn rises(samples: &[[f32; 2]]) -> usize {
        samples.windows(2).filter(|w| w[0][0] < 0.0 && w[1][0] >= 0.0).count()
    }

    #[test]
    fn unstretched_sound_comes_out_as_it_went_in() {
        let input = tone(20_000);
        let mut stretch = Stretch::new();
        let mut out = Vec::new();
        for chunk in input.chunks(546) {
            stretch.process(chunk, 1.0, &mut out);
        }
        // past the first piece fading in, every sample is the one that came in
        assert!(out.len() > 18_000);
        for (a, b) in out[HOP..].iter().zip(&input[HOP..]) {
            assert!((a[0] - b[0]).abs() < 1e-4);
        }
    }

    /// a sound that repeats nowhere, with a stretch of silence in the
    /// middle, so where any piece of it came from is clear.
    fn noise_with_a_pause(count: usize) -> Vec<[f32; 2]> {
        let mut x = 1u32;
        (0..count)
            .map(|i| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let v = if (30_000..33_000).contains(&i) { 0.0 } else { (x >> 8) as f32 / (1 << 24) as f32 - 0.5 };
                [v, v]
            })
            .collect()
    }

    /// qol: at a game speed the sound comes out that many times shorter
    /// (time-stretched, so no faster in pitch), for the queue to stay level.
    #[test]
    fn a_game_speed_shortens_the_sound() {
        for speed in [2.0, 3.0, 4.0] {
            let mut queue = queue(TARGET, 1.0);
            let mut stretch = Stretch::new();
            let input = noise_with_a_pause(60_000);
            let (mut out, mut fed) = (0, 0);
            for (n, chunk) in input.chunks(546).enumerate() {
                let before = queue.samples.len();
                enqueue(&mut queue, &mut stretch, chunk, speed);
                // once the stretch has eased in (a fifth of the way a call)
                if n >= 20 {
                    out += queue.samples.len() - before;
                    fed += chunk.len();
                }
                let extra = queue.samples.len().saturating_sub(TARGET);
                queue.samples.drain(..extra);
            }
            let wanted = fed as f64 / speed;
            assert!((out as f64 - wanted).abs() < wanted * 0.05, "speed {speed}: {out} samples out of {fed}, wanted about {wanted}");
        }
    }

    /// a queue a little short of its target is left to DRIFT, its sound goes
    /// in as it came, with no splice.
    #[test]
    fn a_queue_a_little_short_is_not_stretched() {
        let mut queue = queue(TARGET - 1000, 1.0);
        let mut stretch = Stretch::new();
        let input = noise_with_a_pause(20_000);
        let mut out = Vec::new();
        for chunk in input.chunks(546) {
            let before = queue.samples.len();
            enqueue(&mut queue, &mut stretch, chunk, 1.0);
            out.extend(queue.samples.range(before..).copied());
            // the output plays as much as came in
            queue.samples.drain(..chunk.len());
        }
        assert!(out.len() > 19_000);
        for (a, b) in out[HOP..].iter().zip(&input[HOP..]) {
            assert!((a[0] - b[0]).abs() < 1e-4);
        }
    }

    /// once the queue is full again the stretcher stops, and the sound
    /// follows on as it came in, silence included, with no splice.
    #[test]
    fn stretching_stops_cleanly() {
        let input = noise_with_a_pause(60_000);
        let mut stretch = Stretch::new();
        let mut out = Vec::new();
        for (i, chunk) in input.chunks(546).enumerate() {
            stretch.process(chunk, if i < 20 { 1.5 } else { 1.0 }, &mut out);
        }
        assert_eq!(stretch.factor, 1.0);
        // where the last of it came from, found in noise, which matches in
        // one place only
        let end = &out[out.len() - 10_000..];
        let from = (0..input.len() - end.len())
            .find(|&at| end.iter().zip(&input[at..]).take(64).all(|(a, b)| (a[0] - b[0]).abs() < 1e-4))
            .expect("the end is somewhere in the input");
        // and the input in a row back from there, through the end of the pause
        let tail = &out[out.len() - 28_000..];
        let start = from - 18_000;
        assert!(start < 33_000, "it covers the pause");
        for (a, b) in tail.iter().zip(&input[start..]) {
            assert!((a[0] - b[0]).abs() < 1e-4, "spliced");
        }
    }

    #[test]
    fn stretched_sound_is_longer_at_the_same_pitch() {
        let input = tone(32_728);
        let mut stretch = Stretch::new();
        let mut out = Vec::new();
        for chunk in input.chunks(546) {
            stretch.process(chunk, 1.5, &mut out);
        }
        let ratio = out.len() as f64 / input.len() as f64;
        assert!(ratio > 1.35 && ratio < 1.55, "stretched by {ratio}");
        // the same number of cycles a second
        let pitch = rises(&out) as f64 / out.len() as f64 * 32728.0;
        assert!((pitch - 440.0).abs() < 10.0, "pitch {pitch}");
    }

    #[test]
    fn output_falls_between_the_consoles_samples() {
        let mut queue = queue(TARGET, 0.5);
        assert_eq!(queue.next(), [0.0, 0.0]);
        assert_eq!(queue.next(), [0.5, -0.5]);
        assert_eq!(queue.next(), [1.0, -1.0]);
    }

    #[test]
    fn a_queue_that_ran_dry_fades_and_waits_to_fill_up() {
        let mut queue = queue(1, 1.0);
        queue.last = [0.5, 0.5];
        let faded = queue.next();
        assert!(queue.starved);
        assert_eq!(queue.underruns, 1);
        assert!(faded[0] < 0.5 && faded[0] > 0.4, "it fades out rather than stopping short");
        queue.samples.extend((0..RESUME).map(|_| [1.0, 1.0]));
        assert_eq!(queue.next(), [0.0, -0.0], "it starts again from where it stopped");
    }

    /// stopped while the game is, it does not count running dry, and plays
    /// again only once it is full.
    #[test]
    fn a_held_queue_waits_to_fill_up_and_counts_nothing() {
        let mut queue = queue(0, 1.0);
        queue.last = [0.5, 0.5];
        queue.hold();
        assert!(queue.next()[0] < 0.5);
        assert_eq!(queue.underruns, 0);
        queue.samples.extend((0..RESUME).map(|_| [1.0, 1.0]));
        assert!(queue.next()[0] < 0.5, "what is enough after running dry is not after a hold");
        queue.samples.extend((0..TARGET).map(|_| [1.0, 1.0]));
        assert_eq!(queue.next(), [1.0, 1.0]);
        assert_eq!(queue.underruns, 0);
        // running dry later is counted, and waits for the usual amount
        queue.samples.clear();
        queue.next();
        assert_eq!(queue.underruns, 1);
        queue.samples.extend((0..RESUME).map(|_| [1.0, 1.0]));
        assert_eq!(queue.next(), [1.0, 1.0]);
    }

    /// a game starting or stopping drops what the last one queued, and the
    /// pulse2: time stopping plays what is queued slowing down to silence,
    /// takes nothing in while stopped (clear and hold leave it be), and the
    /// sound after time goes on fades in.
    #[test]
    fn time_winds_down_stays_stopped_and_fades_back_in() {
        let mut queue = queue(0, 32728.0 / 48000.0);
        queue.samples.extend(tone(4096));
        queue.time.stopping = Some((1.0, 1.0 / (0.25 * 48000.0)));
        let wound: Vec<f32> = (0..12500).map(|_| queue.next()[0].abs()).collect();
        let early = wound[..2000].iter().cloned().fold(0.0, f32::max);
        let late = wound[9000..11000].iter().cloned().fold(0.0, f32::max);
        assert!(early > 0.4 && late < early * 0.5, "{early} {late}");
        assert!(queue.time.stopped);
        queue.clear();
        queue.hold();
        assert!(queue.time.stopped);
        let quiet = (0..4000).map(|_| queue.next()[0].abs()).fold(0.0, f32::max);
        assert!(quiet < 0.01);
        // time goes on: a fresh queue, fading in
        queue.time = TimeFx { fading_in: Some((0.0, 1.0 / (0.3 * 48000.0))), ..TimeFx::default() };
        queue.clear();
        queue.samples.extend(tone(TARGET * 3));
        let first = (0..400).map(|_| queue.next()[0].abs()).fold(0.0, f32::max);
        let later: f32 = (0..20000).map(|_| queue.next()[0].abs()).fold(0.0, f32::max);
        assert!(first < 0.05 && later > 0.4, "{first} {later}");
    }

    /// new sound plays once there is enough of it.
    #[test]
    fn a_cleared_queue_plays_none_of_the_old_sound() {
        let mut queue = queue(TARGET, 1.0);
        queue.clear();
        queue.next();
        queue.samples.extend((0..TARGET).map(|_| [-1.0, -1.0]));
        assert_eq!(queue.next(), [-1.0, -1.0]);
        assert_eq!(queue.underruns, 0);
    }

    /// how far behind the picture the sound is, in ms, after each second of
    /// a minute where the console makes exactly as much sound as the output
    /// plays, but a frame now and then comes late and the next ones catch up,
    /// as when the emulation hitches.
    fn latency_with_hitches() -> Vec<f64> {
        const CONSOLE: f64 = 32728.498;
        const OUTPUT: f64 = 48000.0;
        let mut queue = queue(0, CONSOLE / OUTPUT);
        let mut stretch = Stretch::new();
        let mut seconds = Vec::new();
        let tone = tone(40_000);
        let mut at = 0;
        let mut late: Vec<[f32; 2]> = Vec::new();
        for frame in 0..3600usize {
            // a frame's worth of sound, held back every 45th frame for three
            // frames' time
            let count = (CONSOLE * (frame + 1) as f64 / 60.0) as usize - (CONSOLE * frame as f64 / 60.0) as usize;
            let mut chunk: Vec<[f32; 2]> = (0..count).map(|i| tone[(at + i) % tone.len()]).collect();
            at += count;
            if frame % 45 == 0 {
                late.append(&mut chunk);
            } else if frame % 45 == 3 {
                late.append(&mut chunk);
                enqueue(&mut queue, &mut stretch, &late, 1.0);
                late.clear();
            } else if late.is_empty() {
                enqueue(&mut queue, &mut stretch, &chunk, 1.0);
            } else {
                late.append(&mut chunk);
            }
            for _ in 0..(OUTPUT / 60.0) as usize {
                queue.next();
            }
            if frame % 60 == 59 {
                seconds.push(queue.samples.len() as f64 / CONSOLE * 1000.0);
            }
        }
        seconds
    }

    #[test]
    fn hitches_do_not_put_the_sound_further_and_further_behind() {
        let seconds = latency_with_hitches();
        let first = seconds[5];
        let last = *seconds.last().unwrap();
        let most = seconds.iter().cloned().fold(0.0, f64::max);
        eprintln!("latency by second {:?}", seconds.iter().map(|ms| ms.round() as i64).collect::<Vec<_>>());
        assert!(last < first + 30.0, "behind by {first:.0} ms after 5 s and {last:.0} ms after a minute");
        assert!(most < 160.0, "up to {most:.0} ms behind");
    }

    #[test]
    fn a_long_queue_plays_a_little_faster() {
        let mut long = queue(TARGET * 2, 1.0);
        let mut short = queue(TARGET, 1.0);
        for _ in 0..1000 {
            long.next();
            short.next();
        }
        assert!(TARGET * 2 - long.samples.len() > 1000, "the long queue went through more than it played");
        assert!(TARGET - short.samples.len() < 1000, "the short queue went through less than it played");
    }
}
