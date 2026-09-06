//! Audio Frame Decoding (PCM and Opus)
//!
//! Converts incoming compressed or uncompressed bytes into normalized float samples.

use anyhow::{anyhow, Result};
use protocol::{CodecMode, CHANNELS};
use tracing::warn;

#[cfg(feature = "opus")]
use audiopus::{
    coder::Decoder as OpusDecoder, packet::Packet, Channels as OpusChannels, MutSignals,
    SampleRate as OpusSampleRate,
};

pub enum AudioDecoder {
    Pcm,
    #[cfg(feature = "opus")]
    Opus(OpusDecoder),
}

impl AudioDecoder {
    pub fn new(mode: CodecMode) -> Result<Self> {
        match mode {
            CodecMode::Pcm => Ok(AudioDecoder::Pcm),
            CodecMode::Opus => {
                #[cfg(feature = "opus")]
                {
                    let decoder =
                        OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Stereo)
                            .map_err(|e| anyhow!("Failed to initialize Opus decoder: {:?}", e))?;
                    Ok(AudioDecoder::Opus(decoder))
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

    /// Decode audio payload into float samples.
    ///
    /// If `payload` is `None`, performs Packet Loss Concealment (PLC).
    pub fn decode_frame(
        &mut self,
        payload: Option<&[u8]>,
        out_samples: &mut Vec<f32>,
    ) -> Result<()> {
        out_samples.clear();

        match self {
            AudioDecoder::Pcm => {
                if let Some(bytes) = payload {
                    let num_samples = bytes.len() / 2;
                    out_samples.reserve(num_samples);
                    for chunk in bytes.chunks_exact(2) {
                        let i = i16::from_le_bytes([chunk[0], chunk[1]]);
                        out_samples.push(i as f32 / 32767.0);
                    }
                } else {
                    // PCM Loss concealment: fill with silence
                    out_samples.resize(480 * CHANNELS as usize, 0.0);
                }
                Ok(())
            }
            #[cfg(feature = "opus")]
            AudioDecoder::Opus(decoder) => {
                // Max samples per channel for Opus at 48kHz (120ms max)
                out_samples.resize(5760 * CHANNELS as usize, 0.0);

                let packet = match payload {
                    Some(bytes) => match Packet::try_from(bytes) {
                        Ok(p) => Some(p),
                        Err(err) => {
                            warn!("Invalid Opus packet: {:?}. Dropping frame.", err);
                            out_samples.clear();
                            return Ok(());
                        }
                    },
                    None => None,
                };

                let signals = match MutSignals::try_from(out_samples.as_mut_slice()) {
                    Ok(s) => s,
                    Err(err) => {
                        warn!("Failed to create MutSignals for Opus: {:?}.", err);
                        out_samples.clear();
                        return Ok(());
                    }
                };

                match decoder.decode_float(packet, signals, false) {
                    Ok(samples_per_channel) => {
                        let total_samples = samples_per_channel * CHANNELS as usize;
                        out_samples.truncate(total_samples);
                        Ok(())
                    }
                    Err(err) => {
                        warn!(
                            "Opus decode failure: {:?}. Dropping frame rather than crashing.",
                            err
                        );
                        out_samples.clear();
                        Ok(())
                    }
                }
            }
        }
    }
}
