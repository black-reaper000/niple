//! Audio Frame Encoding & Header Assembly
//!
//! Encodes raw float audio into either low-latency PCM or Opus packets.

use anyhow::{anyhow, Result};
use protocol::{AudioHeader, CodecMode, AUDIO_HEADER_LEN};
use tracing::warn;

#[cfg(feature = "opus")]
use audiopus::{
    coder::Encoder as OpusEncoder, Application, Channels as OpusChannels,
    SampleRate as OpusSampleRate,
};

pub enum AudioEncoder {
    Pcm,
    #[cfg(feature = "opus")]
    Opus(OpusEncoder),
}

impl AudioEncoder {
    pub fn new(mode: CodecMode) -> Result<Self> {
        match mode {
            CodecMode::Pcm => Ok(AudioEncoder::Pcm),
            CodecMode::Opus => {
                #[cfg(feature = "opus")]
                {
                    let encoder = OpusEncoder::new(
                        OpusSampleRate::Hz48000,
                        OpusChannels::Stereo,
                        Application::LowDelay,
                    )
                    .map_err(|e| anyhow!("Failed to initialize Opus encoder: {:?}", e))?;
                    Ok(AudioEncoder::Opus(encoder))
                }
                #[cfg(not(feature = "opus"))]
                {
                    Err(anyhow!(
                        "Binary was compiled without Opus support. Run with `--codec pcm` or recompile with `--features opus`."
                    ))
                }
            }
        }
    }

    /// Encode an interleaved stereo frame of `f32` samples at 48kHz.
    ///
    /// `samples`: Interleaved stereo samples (L, R, L, R...). Total len = `frame_samples * 2`.
    /// `seq`: Packet sequence number.
    /// `stream_elapsed_ns`: Sender monotonic nanoseconds elapsed since start.
    pub fn encode_frame(
        &mut self,
        samples: &[f32],
        seq: u32,
        stream_elapsed_ns: u64,
        out_buf: &mut Vec<u8>,
    ) -> Result<()> {
        out_buf.clear();
        // Reserve space for fixed 15-byte header
        out_buf.resize(AUDIO_HEADER_LEN, 0);

        let codec = match self {
            AudioEncoder::Pcm => {
                // Convert f32 samples to 16-bit signed integer little-endian
                for &s in samples {
                    let clamped = s.clamp(-1.0, 1.0);
                    let i = (clamped * 32767.0) as i16;
                    out_buf.extend_from_slice(&i.to_le_bytes());
                }
                CodecMode::Pcm
            }
            #[cfg(feature = "opus")]
            AudioEncoder::Opus(encoder) => {
                let start_idx = out_buf.len();
                // Ensure room for max Opus packet
                out_buf.resize(start_idx + 1275, 0);

                match encoder.encode_float(samples, &mut out_buf[start_idx..]) {
                    Ok(encoded_bytes) => {
                        out_buf.truncate(start_idx + encoded_bytes);
                    }
                    Err(err) => {
                        warn!("Opus encode error: {:?}. Dropping frame.", err);
                        out_buf.truncate(start_idx);
                        return Err(anyhow!("Opus encode error: {:?}", err));
                    }
                }
                CodecMode::Opus
            }
        };

        let payload_len = (out_buf.len() - AUDIO_HEADER_LEN) as u16;
        let header = AudioHeader {
            seq,
            timestamp_ns: stream_elapsed_ns,
            payload_len,
            codec,
        };
        header.serialize_into(&mut out_buf[0..AUDIO_HEADER_LEN]);

        Ok(())
    }
}
