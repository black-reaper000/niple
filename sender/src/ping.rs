//! Lightweight Periodic Round-Trip Ping for Latency Estimation
//!
//! Measures round-trip time between sender and receiver and estimates
//! one-way network delay as approximately RTT / 2.

use anyhow::Result;
use protocol::{ControlMessage, LatencyEstimator};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct PingClient {
    control_socket: Arc<UdpSocket>,
    receiver_control_addr: SocketAddr,
    running: Arc<AtomicBool>,
}

impl PingClient {
    pub async fn new(local_bind: &str, receiver_control_addr: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind(local_bind).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to bind UDP control/ping socket on {}: {}. Is port already in use?",
                local_bind,
                e
            )
        })?;

        Ok(Self {
            control_socket: Arc::new(socket),
            receiver_control_addr,
            running: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Spawns background tasks for sending periodic pings and receiving pong responses.
    pub fn start(&self, stream_start: Instant) {
        let socket_send = Arc::clone(&self.control_socket);
        let socket_recv = Arc::clone(&self.control_socket);
        let dest = self.receiver_control_addr;
        let running_send = Arc::clone(&self.running);
        let running_recv = Arc::clone(&self.running);

        let mut estimator = LatencyEstimator::new(0.3);

        // Ping sender task
        tokio::spawn(async move {
            let mut ping_id = 0u64;
            while running_send.load(Ordering::Relaxed) {
                ping_id += 1;
                let now_ns = stream_start.elapsed().as_nanos() as u64;
                let ping = ControlMessage::Ping {
                    id: ping_id,
                    sender_send_ts_ns: now_ns,
                };
                let bytes = ping.serialize();

                if let Err(e) = socket_send.send_to(&bytes, dest).await {
                    warn!("Failed to send ping to {}: {}", dest, e);
                }

                sleep(Duration::from_secs(2)).await;
            }
        });

        // Pong receiver task
        tokio::spawn(async move {
            let mut buf = [0u8; 128];
            let mut consecutive_misses = 0;

            while running_recv.load(Ordering::Relaxed) {
                // Wait up to 3s for incoming control message
                match tokio::time::timeout(Duration::from_secs(3), socket_recv.recv_from(&mut buf))
                    .await
                {
                    Ok(Ok((len, src))) => {
                        if src != dest {
                            continue;
                        }
                        if let Ok(ControlMessage::Pong {
                            sender_send_ts_ns, ..
                        }) = ControlMessage::deserialize(&buf[..len])
                        {
                            let now_ns = stream_start.elapsed().as_nanos() as u64;
                            let (rtt_ms, est_one_way_ms) =
                                estimator.record_ping(sender_send_ts_ns, now_ns);
                            consecutive_misses = 0;

                            info!(
                                "[TELEMETRY] RTT: {:.2} ms | Est. one-way network latency: ~{:.2} ms (estimate = RTT/2)",
                                rtt_ms, est_one_way_ms
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Control socket recv error: {}", e);
                    }
                    Err(_) => {
                        // Timeout on receiving response
                        consecutive_misses += 1;
                        if consecutive_misses >= 3 {
                            warn!(
                                "[DIAGNOSTIC] Receiver at {} not responding after {} attempts. \
                                 Check both devices are on the same LAN and not on a guest/isolated Wi-Fi network.",
                                dest, consecutive_misses
                            );
                        }
                    }
                }
            }
        });
    }
}
