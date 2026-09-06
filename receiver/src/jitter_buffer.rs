//! Adaptive Jitter Buffer
//!
//! Absorbs network transmission jitter, reorders packets arriving out of order,
//! detects packet loss, and supplies steady continuous audio to the CoreAudio callback.

use protocol::{AudioPacket, CodecMode};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

/// Holds a single buffered audio frame ready for decoding/playout.
#[allow(dead_code)]
#[derive(Clone)]
pub struct BufferedFrame {
    pub seq: u32,
    pub timestamp_ns: u64,
    pub codec: CodecMode,
    pub payload: Vec<u8>,
}

/// Statistics exposed by the jitter buffer.
#[derive(Debug, Default, Clone)]
pub struct JitterStats {
    pub packets_received: u64,
    pub packets_lost: u64,
    pub packets_dropped_late: u64,
    pub loss_rate_pct: f32,
    pub buffer_depth_ms: f32,
}

pub struct JitterBuffer {
    /// Target delay in milliseconds before beginning playout (e.g. 20ms)
    pub target_delay_ms: f32,
    /// Ordered map of buffered packets keyed by sequence number
    queue: BTreeMap<u32, BufferedFrame>,
    /// Expected next sequence number to play
    expected_seq: Option<u32>,
    /// Whether the buffer has filled to target threshold and started playout
    is_playing: bool,
    /// Total received packets
    packets_received: Arc<AtomicU64>,
    /// Total lost packets (detected by sequence gaps)
    packets_lost: Arc<AtomicU64>,
    /// Late packets dropped
    packets_dropped_late: Arc<AtomicU64>,
    /// Estimated frame duration in ms (derived from incoming packets)
    frame_duration_ms: f32,
}

impl JitterBuffer {
    pub fn new(target_delay_ms: f32) -> Self {
        Self {
            target_delay_ms: target_delay_ms.clamp(5.0, 200.0),
            queue: BTreeMap::new(),
            expected_seq: None,
            is_playing: false,
            packets_received: Arc::new(AtomicU64::new(0)),
            packets_lost: Arc::new(AtomicU64::new(0)),
            packets_dropped_late: Arc::new(AtomicU64::new(0)),
            frame_duration_ms: 10.0,
        }
    }

    /// Push an incoming packet into the jitter buffer.
    pub fn push(&mut self, packet: AudioPacket) {
        let seq = packet.header.seq;
        self.packets_received.fetch_add(1, Ordering::Relaxed);

        if let Some(expected) = self.expected_seq {
            // Sequence number wrap-around or old packet check
            let diff = seq.wrapping_sub(expected);
            if diff > 0x80000000 {
                // Packet arrived after its playout deadline has passed
                self.packets_dropped_late.fetch_add(1, Ordering::Relaxed);
                return;
            }
        } else {
            self.expected_seq = Some(seq);
        }

        self.queue.insert(
            seq,
            BufferedFrame {
                seq,
                timestamp_ns: packet.header.timestamp_ns,
                codec: packet.header.codec,
                payload: packet.payload.to_vec(),
            },
        );

        // Approximate frame duration from PCM packet size if not known
        if packet.header.codec == CodecMode::Pcm && packet.payload.len() > 0 {
            // 48000 Hz, 2 channels, 2 bytes/sample = 192 bytes per ms
            let ms = packet.payload.len() as f32 / 192.0;
            if ms > 0.0 {
                self.frame_duration_ms = ms;
            }
        }

        // Check if we have buffered enough to begin playout
        if !self.is_playing {
            let buffered_ms = self.queue.len() as f32 * self.frame_duration_ms;
            if buffered_ms >= self.target_delay_ms {
                self.is_playing = true;
                info!(
                    "Jitter buffer primed with {:.1}ms of audio (target: {:.1}ms). Playout starting.",
                    buffered_ms, self.target_delay_ms
                );
            }
        }
    }

    /// Pull the next frame for playback. Returns `None` if buffer is underrun.
    pub fn pop(&mut self) -> Option<BufferedFrame> {
        if !self.is_playing {
            return None;
        }

        let expected = match self.expected_seq {
            Some(s) => s,
            None => return None,
        };

        // Check if the expected frame is available
        if let Some(frame) = self.queue.remove(&expected) {
            self.expected_seq = Some(expected.wrapping_add(1));
            return Some(frame);
        }

        // If queue is completely empty: buffer underrun
        if self.queue.is_empty() {
            if self.is_playing {
                self.is_playing = false;
                warn!(
                    "Jitter buffer underrun! Pausing playout to refill to {:.1}ms target delay.",
                    self.target_delay_ms
                );
            }
            return None;
        }

        // Gap detection: check if earliest available sequence number indicates missing packets
        if let Some((&earliest_seq, _)) = self.queue.iter().next() {
            let gap = earliest_seq.wrapping_sub(expected);
            if gap < 20 {
                // Missing packet within reasonable window: declare lost and advance
                self.packets_lost.fetch_add(gap as u64, Ordering::Relaxed);
                warn!(
                    "Packet loss detected: sequence gap from {} to {} ({} packets lost)",
                    expected, earliest_seq, gap
                );
                self.expected_seq = Some(earliest_seq.wrapping_add(1));
                return self.queue.remove(&earliest_seq);
            } else {
                // Huge gap (e.g. sender restart): resynchronize
                warn!(
                    "Large sequence discontinuity (expected {}, got {}). Resynchronizing jitter buffer.",
                    expected, earliest_seq
                );
                self.expected_seq = Some(earliest_seq.wrapping_add(1));
                return self.queue.remove(&earliest_seq);
            }
        }

        None
    }

    pub fn stats(&self) -> JitterStats {
        let rx = self.packets_received.load(Ordering::Relaxed);
        let lost = self.packets_lost.load(Ordering::Relaxed);
        let dropped = self.packets_dropped_late.load(Ordering::Relaxed);
        let total = rx + lost;
        let loss_rate_pct = if total > 0 {
            (lost as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        let buffer_depth_ms = self.queue.len() as f32 * self.frame_duration_ms;

        JitterStats {
            packets_received: rx,
            packets_lost: lost,
            packets_dropped_late: dropped,
            loss_rate_pct,
            buffer_depth_ms,
        }
    }
}
