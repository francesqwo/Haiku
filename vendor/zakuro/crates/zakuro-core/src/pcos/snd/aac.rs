//! audio: nn::aacdec natively. The game's voice clips (sound/stream/*.aac,
//! 1,026 files) are ADTS AAC streams that nw::snd's AacDecoder hands to
//! nn::aacdec, which on the console sends each frame to the DSP's AAC
//! decoder through the binary pipe and waits on the DSP's interrupt. Here
//! Decode decodes the frame at once on the host (symphonia) and writes the
//! PCM where the game asked; Wait has nothing left to wait for.

use symphonia_codec_aac::AacDecoder;
use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

use crate::memory::Memory;

/// the sample rates in the order nn::aacdec numbers them (its GetSampleRate
/// table): 48000 is 0, 44100 is 1, ...
const RATES: [u32; 9] = [48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000];
const ADTS_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

#[derive(Default)]
pub struct Aac {
    decoder: Option<(AacDecoder, (u32, u32))>,
    pub initialized: bool,
    /// ADTS (1) or raw.
    pub form: u32,
    /// what the last frame left: bytes used, channels, rate index.
    pub used: u32,
    pub channels: u32,
    pub rate_index: i32,
    pub frames: u64,
    pub failures: u64,
}

impl Aac {
    pub fn initialize(&mut self, form: u32) {
        self.decoder = None;
        self.initialized = true;
        self.form = form;
        // what the DSP answers for a frame that does not decode (Citra's
        // values, which keep titles going): 48 kHz, two channels
        self.used = 0;
        self.channels = 2;
        self.rate_index = 0;
    }

    pub fn finalize(&mut self) {
        self.decoder = None;
        self.initialized = false;
    }

    /// nn::aacdec::Decode(source, left, right, size): decodes one ADTS frame
    /// (whose size comes from its header when size is 0) into 1024 samples
    /// per channel at left and right. false when the frame is not one the
    /// SDK accepts, as the SDK's own checks answer.
    pub fn decode(&mut self, memory: &mut Memory, source: u32, left: u32, right: u32, size: u32) -> bool {
        if !self.initialized {
            return false;
        }
        let mut head = [0u8; 9];
        memory.read_bytes(source, &mut head);
        let mut size = size;
        if self.form == 1 {
            if head[0] != 0xFF || head[1] & 0xF0 != 0xF0 {
                return false;
            }
            size = (((head[3] & 3) as u32) << 11) | ((head[4] as u32) << 3) | ((head[5] as u32) >> 5);
        }
        if size == 0 || size > 0x600 {
            return false;
        }
        let mut frame = vec![0u8; size as usize];
        memory.read_bytes(source, &mut frame);
        // whatever happens below, the SDK goes on with the bytes used
        self.used = size;
        self.frames += 1;
        let protection_absent = frame[1] & 1 != 0;
        let rate_index = ((frame[2] >> 2) & 0xF) as usize;
        let layout = (((frame[2] & 1) << 2) | (frame[3] >> 6)) as u32;
        let header = if protection_absent { 7 } else { 9 };
        let Some(&rate) = ADTS_RATES.get(rate_index) else {
            self.failures += 1;
            return true;
        };
        if frame.len() <= header {
            self.failures += 1;
            return true;
        }
        let key = (rate_index as u32, layout);
        if self.decoder.as_ref().is_none_or(|(_, made)| *made != key) {
            let config = (2u16 << 11) | ((rate_index as u16) << 7) | ((layout as u16) << 3);
            let mut params = AudioCodecParameters::new();
            params.for_codec(CODEC_ID_AAC).with_extra_data(config.to_be_bytes().to_vec().into_boxed_slice());
            self.decoder = match AacDecoder::try_new(&params, &AudioDecoderOptions::default()) {
                Ok(decoder) => Some((decoder, key)),
                Err(error) => {
                    log::warn!("audio: no AAC decoder for rate {rate} layout {layout}: {error}");
                    None
                }
            };
        }
        let Some((decoder, _)) = self.decoder.as_mut() else {
            self.failures += 1;
            return true;
        };
        let packet = PacketRef::new(0, Timestamp::ZERO, Duration::new(1024), &frame[header..]);
        let mut planes: Vec<Vec<i16>> = Vec::new();
        match decoder.decode_ref(&packet) {
            Ok(decoded) => decoded.copy_to_vecs_planar::<i16>(&mut planes),
            Err(error) => {
                self.failures += 1;
                log::debug!("audio: an AAC frame did not decode: {error}");
                return true;
            }
        }
        if planes.is_empty() {
            self.failures += 1;
            return true;
        }
        for (plane, target) in planes.iter().zip([left, right]) {
            if target == 0 {
                continue;
            }
            let count = plane.len().min(1024);
            let bytes: Vec<u8> = plane[..count].iter().flat_map(|s| s.to_le_bytes()).collect();
            memory.write_bytes(target, &bytes);
        }
        self.channels = planes.len() as u32;
        self.rate_index = RATES.iter().position(|&hz| hz == rate).map_or(-1, |i| i as i32);
        true
    }
}
