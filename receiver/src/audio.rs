//! CoreAudio Playback & Volume Control for macOS (and Windows testing)
//!
//! Plays decoded audio through macOS speakers or headphones with lock-free
//! receiver-side volume scaling.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use tracing::{error, info};

#[allow(dead_code)]
pub struct AudioPlayback {
    _stream: Stream,
    pub device_name: String,
    pub volume: Arc<AtomicU32>,
}

/// Enumerate available audio output devices.
pub fn list_output_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .output_devices()
        .context("Failed to query output devices")?;

    let mut names = Vec::new();
    for dev in devices {
        if let Ok(name) = dev.name() {
            names.push(name);
        }
    }
    Ok(names)
}

/// Select output device by name substring or system default.
pub fn select_output_device(name_filter: Option<&str>) -> Result<Device> {
    let host = cpal::default_host();

    if let Some(query) = name_filter {
        let devices = host
            .output_devices()
            .context("No audio output devices found")?;

        for dev in devices {
            if let Ok(name) = dev.name() {
                if name.to_lowercase().contains(&query.to_lowercase()) {
                    info!("Selected output device for playback: {}", name);
                    return Ok(dev);
                }
            }
        }

        let available = list_output_devices().unwrap_or_default();
        return Err(anyhow!(
            "Device matching '{}' not found. Available devices:\n  - {}",
            query,
            available.join("\n  - ")
        ));
    }

    host.default_output_device().ok_or_else(|| {
        anyhow!(
            "No default audio output device found! Please connect speakers/headphones \
             or select a valid output device in macOS Sound Preferences."
        )
    })
}

/// Start CoreAudio playback stream.
///
/// Samples received on `sample_rx` are multiplied by the receiver-side volume gain
/// and written directly to the audio output device buffer.
pub fn start_playback(
    device: Device,
    sample_rx: Receiver<Vec<f32>>,
    volume: Arc<AtomicU32>,
) -> Result<AudioPlayback> {
    let device_name = device.name().unwrap_or_else(|_| "Unknown Device".into());
    info!("Initializing audio playback on: {}", device_name);

    let default_config = device.default_output_config().map_err(|e| {
        anyhow!(
            "Failed to retrieve default output config for device '{}': {}",
            device_name,
            e
        )
    })?;

    let sample_rate = default_config.sample_rate();
    let channels = default_config.channels();
    let sample_format = default_config.sample_format();

    info!(
        "Output endpoint: {} Hz, {} channels, format: {:?}",
        sample_rate.0, channels, sample_format
    );

    let config = StreamConfig {
        channels: 2,
        sample_rate: cpal::SampleRate(48000),
        buffer_size: cpal::BufferSize::Default,
    };

    let err_fn = move |err: cpal::StreamError| {
        error!(
            "Audio playback stream error: {}. If the output device was unplugged, please restart the receiver.",
            err
        );
    };

    let mut playout_buf: Vec<f32> = Vec::with_capacity(4096);
    let volume_clone = Arc::clone(&volume);

    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // Read current volume multiplier in lock-free manner
                let gain = f32::from_bits(volume_clone.load(Ordering::Relaxed));

                // Pull available samples from sample channel
                while let Ok(chunk) = sample_rx.try_recv() {
                    playout_buf.extend_from_slice(&chunk);
                }

                let needed = output.len();
                if playout_buf.len() >= needed {
                    for (out, &sample) in output.iter_mut().zip(&playout_buf[..needed]) {
                        *out = (sample * gain).clamp(-1.0, 1.0);
                    }
                    playout_buf.drain(..needed);
                } else {
                    // Underrun in audio hardware callback: play what's available and pad with silence
                    let avail = playout_buf.len();
                    for (out, &sample) in output[..avail].iter_mut().zip(&playout_buf[..avail]) {
                        *out = (sample * gain).clamp(-1.0, 1.0);
                    }
                    output[avail..].fill(0.0);
                    playout_buf.clear();
                }
            },
            err_fn,
            None,
        )
        .context("Failed to build output audio stream")?;

    stream
        .play()
        .context("Failed to start CoreAudio playback stream")?;
    info!("CoreAudio playback active and ready for incoming stream.");

    Ok(AudioPlayback {
        _stream: stream,
        device_name,
        volume,
    })
}
