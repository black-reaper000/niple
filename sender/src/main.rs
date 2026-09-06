//! LAN Audio Sender (Windows WASAPI Loopback Capture & Transmission)

mod audio;
mod discovery;
mod encoder;
mod ping;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use encoder::AudioEncoder;
use ping::PingClient;
use protocol::{CodecMode, DEFAULT_AUDIO_PORT, DEFAULT_CONTROL_PORT, SAMPLE_RATE};
use std::net::SocketAddr;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "lan-audio-sender",
    about = "Stream Windows system audio to macOS over LAN with ultra-low latency"
)]
struct Args {
    /// Target IP of macOS receiver ("auto" to search via mDNS Bonjour)
    #[arg(short, long, env = "SENDER_TARGET_IP", default_value = "auto")]
    target_ip: String,

    /// UDP port for audio stream
    #[arg(short, long, env = "AUDIO_PORT", default_value_t = DEFAULT_AUDIO_PORT)]
    port: u16,

    /// UDP port for control & round-trip ping
    #[arg(short = 'c', long, env = "CONTROL_PORT", default_value_t = DEFAULT_CONTROL_PORT)]
    control_port: u16,

    /// Audio codec mode: "pcm" (lowest latency, gigabit LAN) or "opus" (compressed, Wi-Fi friendly)
    #[arg(long, env = "CODEC_MODE", default_value = "opus")]
    codec: String,

    /// Audio frame duration in milliseconds (2.5, 5, 10, or 20)
    #[arg(short = 'f', long, env = "FRAME_SIZE_MS", default_value_t = 10.0)]
    frame_size_ms: f32,

    /// Substring name of Windows audio output device to capture (defaults to system default output)
    #[arg(short = 'd', long, env = "AUDIO_DEVICE")]
    device: Option<String>,

    /// List all available output devices for loopback capture and exit
    #[arg(long)]
    list_devices: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    if args.list_devices {
        info!("Enumerating available audio output endpoints for loopback capture:");
        match audio::list_output_devices() {
            Ok(devs) if !devs.is_empty() => {
                for (i, dev) in devs.iter().enumerate() {
                    println!("  [{}] {}", i + 1, dev);
                }
            }
            Ok(_) => println!("  No output devices found."),
            Err(e) => eprintln!("Error querying devices: {}", e),
        }
        return Ok(());
    }

    println!(
        r#"
  ======================================================
     LAN Audio: Native Low-Latency Windows -> Mac
     Capturing via WASAPI Loopback (Zero-Mic, No WebRTC)
  ======================================================
"#
    );

    let codec_mode = match args.codec.to_lowercase().as_str() {
        "pcm" => CodecMode::Pcm,
        "opus" => CodecMode::Opus,
        other => {
            return Err(anyhow!(
                "Invalid codec '{}'. Choose either 'pcm' or 'opus'.",
                other
            ));
        }
    };

    // Calculate frame size in samples per channel
    let samples_per_channel = (SAMPLE_RATE as f32 * (args.frame_size_ms / 1000.0)).round() as usize;
    let interleaved_samples_per_frame = samples_per_channel * 2; // Stereo

    info!(
        "Configured: Codec={}, FrameSize={:.1}ms ({} samples/ch, {} stereo samples)",
        codec_mode, args.frame_size_ms, samples_per_channel, interleaved_samples_per_frame
    );

    // Resolve target receiver address
    let receiver_audio_addr = if args.target_ip.eq_ignore_ascii_case("auto") {
        info!("Auto-discovery mode enabled. Browsing for macOS receiver via mDNS...");
        match discovery::discover_receiver(Duration::from_secs(5)).await {
            Ok(addr) => addr,
            Err(e) => {
                error!("{}", e);
                return Err(e);
            }
        }
    } else {
        let ip: std::net::IpAddr = args
            .target_ip
            .parse()
            .with_context(|| format!("Invalid target IP address: '{}'", args.target_ip))?;
        SocketAddr::new(ip, args.port)
    };

    let receiver_control_addr = SocketAddr::new(receiver_audio_addr.ip(), args.control_port);
    info!(
        "Target macOS Receiver: Audio={}, Control={}",
        receiver_audio_addr, receiver_control_addr
    );

    // Bind local UDP audio socket
    let local_bind = "0.0.0.0:0";
    let audio_socket = UdpSocket::bind(local_bind)
        .await
        .context("Failed to bind local UDP audio socket")?;

    info!(
        "Local audio UDP socket bound to: {}",
        audio_socket.local_addr()?
    );

    // Initialize Ping Client
    let stream_start = Instant::now();
    let ping_client = PingClient::new("0.0.0.0:0", receiver_control_addr).await?;
    ping_client.start(stream_start);

    // Select audio output device and start WASAPI loopback
    let device = audio::select_output_device(args.device.as_deref())?;
    let (sample_tx, sample_rx) = sync_channel::<Vec<f32>>(128);
    let _capture = audio::start_loopback_capture(device, sample_tx)?;

    // Initialize encoder
    let mut encoder = AudioEncoder::new(codec_mode)?;

    info!(
        "Streaming audio directly to {} over UDP...",
        receiver_audio_addr
    );
    info!("Press Ctrl+C to stop streaming.");

    let mut sequence_number: u32 = 0;
    let mut frame_buffer: Vec<f32> = Vec::with_capacity(interleaved_samples_per_frame * 2);
    let mut packet_buffer: Vec<u8> = Vec::with_capacity(2048);

    // Main capture-encode-send loop
    while let Ok(captured_chunk) = sample_rx.recv() {
        frame_buffer.extend_from_slice(&captured_chunk);

        while frame_buffer.len() >= interleaved_samples_per_frame {
            let frame = &frame_buffer[..interleaved_samples_per_frame];
            let stream_elapsed_ns = stream_start.elapsed().as_nanos() as u64;

            if let Ok(()) = encoder.encode_frame(
                frame,
                sequence_number,
                stream_elapsed_ns,
                &mut packet_buffer,
            ) {
                if let Err(e) = audio_socket
                    .send_to(&packet_buffer, receiver_audio_addr)
                    .await
                {
                    warn!("Failed to send audio packet {}: {}", sequence_number, e);
                }
                sequence_number = sequence_number.wrapping_add(1);
            }

            frame_buffer.drain(..interleaved_samples_per_frame);
        }
    }

    ping_client.stop();
    Ok(())
}
