//! Control and Ping Responder for macOS Receiver
//!
//! Handles periodic ping packets from the Windows sender and applies
//! volume gain adjustments sent from the sender or local dashboard.

use anyhow::{anyhow, Result};
use protocol::{ControlMessage, CONTROL_MAGIC};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tracing::{error, info, warn};

pub struct ControlServer {
    socket: Arc<UdpSocket>,
    volume: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
}

impl ControlServer {
    pub async fn new(listen_addr: SocketAddr, volume: Arc<AtomicU32>) -> Result<Self> {
        let socket = UdpSocket::bind(listen_addr).await.map_err(|e| {
            anyhow!(
                "Failed to bind control UDP socket on {}: {}. Is port already in use?",
                listen_addr,
                e
            )
        })?;

        info!("Control & Ping server listening on: {}", listen_addr);

        Ok(Self {
            socket: Arc::new(socket),
            volume,
            running: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn start(&self, stream_start: Instant) {
        let socket = Arc::clone(&self.socket);
        let volume = Arc::clone(&self.volume);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            let mut buf = [0u8; 512];

            while running.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        if len < 3 || buf[0..2] != CONTROL_MAGIC {
                            continue;
                        }

                        match ControlMessage::deserialize(&buf[..len]) {
                            Ok(ControlMessage::Ping {
                                id,
                                sender_send_ts_ns,
                            }) => {
                                let now_ns = stream_start.elapsed().as_nanos() as u64;
                                let pong = ControlMessage::Pong {
                                    id,
                                    sender_send_ts_ns,
                                    receiver_recv_ts_ns: now_ns,
                                };
                                let reply_bytes = pong.serialize();
                                if let Err(e) = socket.send_to(&reply_bytes, src).await {
                                    warn!("Failed to send Pong to {}: {}", src, e);
                                }
                            }
                            Ok(ControlMessage::SetVolume { volume: new_vol }) => {
                                let clamped = new_vol.clamp(0.0, 1.5);
                                volume.store(clamped.to_bits(), Ordering::Relaxed);
                                info!(
                                    "Receiver volume adjusted to {:.0}% by {}",
                                    clamped * 100.0,
                                    src
                                );
                            }
                            Ok(other) => {
                                tracing::debug!(
                                    "Received control message from {}: {:?}",
                                    src,
                                    other
                                );
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "Failed to parse control message from {}: {}",
                                    src,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!("Control socket receive error: {}", e);
                    }
                }
            }
        });
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}
