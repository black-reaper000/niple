//! WASAPI Loopback System Audio Capture for Windows
//!
//! Captures system audio played by any Windows application directly from
//! the audio endpoint without microphone delay or screen-sharing overhead.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, SupportedStreamConfig};
use std::sync::mpsc::SyncSender;
use tracing::{error, info, warn};

/// Container for the active capture stream and device metadata.
#[allow(dead_code)]
pub struct AudioCapture {
    _stream: Stream,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Enumerate and list all available audio output devices (endpoints for WASAPI loopback).
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

/// Select an output device by name substring, or use system default.
pub fn select_output_device(name_filter: Option<&str>) -> Result<Device> {
    let host = cpal::default_host();

    if let Some(query) = name_filter {
        let devices = host
            .output_devices()
            .context("No audio output devices found")?;

        for dev in devices {
            if let Ok(name) = dev.name() {
                if name.to_lowercase().contains(&query.to_lowercase()) {
                    info!("Selected requested output device for loopback: {}", name);
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
            "No default audio output device found! WASAPI loopback requires an active \
             playback device (e.g. Speakers, HDMI output, or VB-Audio Virtual Cable). \
             Please enable an audio endpoint in Windows Sound settings."
        )
    })
}

/// Start WASAPI loopback capture stream on the chosen output device.
///
/// Captured interleaved float samples (normalized to [-1.0, 1.0]) are dispatched
/// to `sample_tx`.
pub fn start_loopback_capture(
    device: Device,
    sample_tx: SyncSender<Vec<f32>>,
) -> Result<AudioCapture> {
    let device_name = device.name().unwrap_or_else(|_| "Unknown Device".into());
    info!("Initializing WASAPI loopback capture on: {}", device_name);

    let default_config = device.default_output_config().map_err(|e| {
        anyhow!(
            "Failed to retrieve default output config for device '{}': {}. \
             Ensure the device is not exclusively locked by another program.",
            device_name,
            e
        )
    })?;

    let sample_rate = default_config.sample_rate().0;
    let channels = default_config.channels();
    let sample_format = default_config.sample_format();

    info!(
        "Device native format: {} Hz, {} channels, format: {:?}",
        sample_rate, channels, sample_format
    );

    let err_fn = move |err: cpal::StreamError| {
        error!("Audio loopback stream error: {}", err);
    };

    let stream = match sample_format {
        SampleFormat::F32 => {
            build_loopback_stream::<f32>(&device, &default_config, sample_tx, err_fn)?
        }
        SampleFormat::I16 => {
            build_loopback_stream::<i16>(&device, &default_config, sample_tx, err_fn)?
        }
        SampleFormat::U16 => {
            build_loopback_stream::<u16>(&device, &default_config, sample_tx, err_fn)?
        }
        other => {
            return Err(anyhow!(
                "Unsupported native audio sample format {:?}. Only F32 and I16 formats are supported.",
                other
            ));
        }
    };

    stream
        .play()
        .context("Failed to start WASAPI loopback stream")?;
    info!("WASAPI loopback capture active and streaming.");

    Ok(AudioCapture {
        _stream: stream,
        device_name,
        sample_rate,
        channels,
    })
}

fn build_loopback_stream<T>(
    device: &Device,
    config: &SupportedStreamConfig,
    sample_tx: SyncSender<Vec<f32>>,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: cpal::Sample + cpal::SizedSample + Into<f32>,
{
    let stream_config = config.config();
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if data.is_empty() {
                    return;
                }
                let mut float_buf = Vec::with_capacity(data.len());
                for &s in data {
                    float_buf.push(s.into());
                }
                if let Err(e) = sample_tx.try_send(float_buf) {
                    // Buffer full; drop frame rather than blocking real-time audio thread
                    warn!("Audio sample queue full, dropped capture buffer: {}", e);
                }
            },
            err_fn,
            None,
        )
        .context("Failed to build WASAPI loopback input stream")?;

    Ok(stream)
}
