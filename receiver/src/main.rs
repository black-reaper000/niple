//! LAN Audio Receiver (macOS CoreAudio Playback & Jitter Buffer)

mod audio;
mod control;
mod decoder;
mod discovery;
mod jitter_buffer;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use control::ControlServer;
use decoder::AudioDecoder;
use discovery::ServiceAdvertiser;
use jitter_buffer::JitterBuffer;
use protocol::{AudioPacket, CodecMode, DEFAULT_AUDIO_PORT, DEFAULT_CONTROL_PORT};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "lan-audio-receiver",
    about = "macOS Wireless Speaker Receiver for Windows PC Audio"
)]
struct Args {
    /// Local IP address to listen on
    #[arg(short, long, env = "RECEIVER_LISTEN_IP", default_value = "0.0.0.0")]
    listen_ip: String,

    /// UDP port for audio stream
    #[arg(short, long, env = "AUDIO_PORT", default_value_t = DEFAULT_AUDIO_PORT)]
    port: u16,

    /// UDP port for control & ping
    #[arg(short = 'c', long, env = "CONTROL_PORT", default_value_t = DEFAULT_CONTROL_PORT)]
    control_port: u16,

    /// Adaptive jitter buffer target depth in milliseconds (default: 20ms)
    #[arg(short = 'j', long, env = "JITTER_BUFFER_MS", default_value_t = 20.0)]
    jitter_buffer_ms: f32,

    /// Substring name of macOS output device (defaults to system default output)
    #[arg(short = 'd', long, env = "RECEIVER_DEVICE")]
    device: Option<String>,

    /// List all available audio output devices and exit
    #[arg(long)]
    list_devices: bool,

    /// Disable mDNS Bonjour service advertisement
    #[arg(long)]
    no_mdns: bool,

    /// Enable optional local self-signed HTTPS dashboard
    #[arg(long, env = "DASHBOARD_ENABLED", default_value_t = true)]
    dashboard: bool,

    /// Port for local HTTPS dashboard
    #[arg(long, env = "DASHBOARD_PORT", default_value_t = 8443)]
    dashboard_port: u16,
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
        info!("Enumerating available audio output endpoints for playback:");
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
     LAN Audio: macOS Wireless Speaker Receiver
     Playing via CoreAudio with Adaptive Jitter Buffer
  ======================================================
"#
    );

    let stream_start = Instant::now();
    // Default initial volume: 100% (1.0 f32)
    let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));

    // Select audio output device and start CoreAudio playback
    let output_device = audio::select_output_device(args.device.as_deref())?;
    let (audio_tx, audio_rx) = sync_channel::<Vec<f32>>(128);
    let _playback = audio::start_playback(output_device, audio_rx, Arc::clone(&volume))?;

    // Bind UDP audio socket
    let audio_addr: SocketAddr = format!("{}:{}", args.listen_ip, args.port)
        .parse()
        .context("Invalid listen IP or port")?;

    let audio_socket = UdpSocket::bind(audio_addr).await.map_err(|e| {
        anyhow!(
            "Failed to bind audio UDP socket on {}: {}. Is port {} already in use?",
            audio_addr,
            e,
            args.port
        )
    })?;

    info!(
        "Audio UDP socket listening on: {}",
        audio_socket.local_addr()?
    );

    // Bind Control & Ping socket
    let control_addr: SocketAddr = format!("{}:{}", args.listen_ip, args.control_port)
        .parse()
        .context("Invalid control listen IP or port")?;

    let control_server = ControlServer::new(control_addr, Arc::clone(&volume)).await?;
    control_server.start(stream_start);

    // Start mDNS advertisement if enabled
    let _advertiser = if !args.no_mdns {
        match ServiceAdvertiser::new(args.port, args.control_port) {
            Ok(adv) => Some(adv),
            Err(e) => {
                warn!(
                    "mDNS advertisement failed: {}. Continuing in manual IP mode.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    // Start optional HTTPS Dashboard if enabled
    #[cfg(feature = "dashboard")]
    if args.dashboard {
        let dash_port = args.dashboard_port;
        let dash_volume = Arc::clone(&volume);
        tokio::spawn(async move {
            info!(
                "Launching optional self-signed HTTPS dashboard on port {}...",
                dash_port
            );
            if let Err(e) = dashboard::run_dashboard(dash_port, dash_volume).await {
                warn!("Dashboard server exited with error: {}", e);
            }
        });
    }

    info!(
        "Receiver ready. Target jitter buffer delay: {:.1}ms. Waiting for audio from Windows PC...",
        args.jitter_buffer_ms
    );

    let mut jitter_buf = JitterBuffer::new(args.jitter_buffer_ms);
    let mut recv_buf = vec![0u8; 4096];
    let mut last_stats_report = Instant::now();

    // Spawn an asynchronous feeder task to drain the jitter buffer into the audio playback channel
    let (jb_tx, mut jb_rx) = tokio::sync::mpsc::channel::<jitter_buffer::BufferedFrame>(64);
    let playback_tx = audio_tx.clone();

    tokio::spawn(async move {
        let mut pcm_dec = AudioDecoder::new(CodecMode::Pcm).unwrap();
        #[cfg(feature = "opus")]
        let mut opus_dec = AudioDecoder::new(CodecMode::Opus).ok();
        let mut samples = Vec::with_capacity(1920);

        while let Some(frame) = jb_rx.recv().await {
            let res = match frame.codec {
                CodecMode::Pcm => pcm_dec.decode_frame(Some(&frame.payload), &mut samples),
                CodecMode::Opus => {
                    #[cfg(feature = "opus")]
                    {
                        if let Some(dec) = &mut opus_dec {
                            dec.decode_frame(Some(&frame.payload), &mut samples)
                        } else {
                            warn!("Received Opus frame but Opus decoder not available!");
                            continue;
                        }
                    }
                    #[cfg(not(feature = "opus"))]
                    {
                        warn!("Received Opus frame but receiver compiled without Opus!");
                        continue;
                    }
                }
            };

            if res.is_ok() && !samples.is_empty() {
                let _ = playback_tx.send(samples.clone());
            }
        }
    });

    // Main UDP reception loop
    loop {
        match audio_socket.recv_from(&mut recv_buf).await {
            Ok((len, src)) => {
                match AudioPacket::parse(&recv_buf[..len]) {
                    Ok(packet) => {
                        jitter_buf.push(packet);

                        // Pull frame for playout maintaining target buffer depth
                        if let Some(frame) = jitter_buf.pop() {
                            let _ = jb_tx.try_send(frame);
                        }
                    }
                    Err(e) => {
                        warn!("Received malformed audio packet from {}: {}", src, e);
                    }
                }

                // Log stats periodically every 5 seconds
                if last_stats_report.elapsed() >= Duration::from_secs(5) {
                    let s = jitter_buf.stats();
                    let vol = f32::from_bits(volume.load(Ordering::Relaxed));
                    info!(
                        "[RECEIVER STATS] Received: {} pkts | Lost: {} ({:.2}%) | Late dropped: {} | Buffer: {:.1}ms | Volume: {:.0}%",
                        s.packets_received, s.packets_lost, s.loss_rate_pct, s.packets_dropped_late, s.buffer_depth_ms, vol * 100.0
                    );

                    // High packet loss warning
                    if s.loss_rate_pct > 2.0 {
                        warn!(
                            "[WARNING] Packet loss is {:.2}%, exceeding 2.0% threshold. Check Wi-Fi interference or increase jitter buffer.",
                            s.loss_rate_pct
                        );
                    }

                    last_stats_report = Instant::now();
                }
            }
            Err(e) => {
                error!("Audio UDP recv error: {}", e);
            }
        }
    }
}
