//! LAN Audio Protocol
//!
//! Defines the wire protocol for ultra-low latency audio streaming and control
//! across a local network between Windows and macOS.

use byteorder::{BigEndian, ByteOrder};
use std::fmt;

/// Length of the fixed UDP audio packet header in bytes.
///
/// Layout:
/// - sequence_number: u32 (4 bytes)
/// - timestamp_ns:    u64 (8 bytes)
/// - payload_len:     u16 (2 bytes)
/// - codec_flag:      u8  (1 byte)
/// Total = 15 bytes.
pub const AUDIO_HEADER_LEN: usize = 15;

/// Magic bytes for the separate control/ping protocol.
pub const CONTROL_MAGIC: [u8; 2] = [0x4C, 0x41]; // 'L', 'A'

/// Default audio port
pub const DEFAULT_AUDIO_PORT: u16 = 40000;

/// Default control and ping port
pub const DEFAULT_CONTROL_PORT: u16 = 40001;

/// Default sample rate for all audio streams (Hz)
pub const SAMPLE_RATE: u32 = 48000;

/// Default channels (stereo)
pub const CHANNELS: u16 = 2;

/// Supported audio codec modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecMode {
    /// Raw uncompressed interleaved 16-bit signed PCM (Little-Endian)
    /// Lowest latency, zero encode/decode algorithmic delay, ~1.536 Mbps bandwidth.
    Pcm = 0,
    /// Opus compressed audio
    /// Low bandwidth (~64-160 kbps), 2.5ms - 20ms algorithmic delay.
    Opus = 1,
}

impl CodecMode {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(CodecMode::Pcm),
            1 => Some(CodecMode::Opus),
            _ => None,
        }
    }
}

impl fmt::Display for CodecMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecMode::Pcm => write!(f, "PCM (Raw 16-bit)"),
            CodecMode::Opus => write!(f, "Opus"),
        }
    }
}

/// Errors that can occur during packet parsing.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("Packet is too short: expected at least {expected} bytes, got {actual}")]
    PacketTooShort { expected: usize, actual: usize },

    #[error(
        "Invalid payload length: expected {expected} bytes, actual payload buffer is {actual}"
    )]
    PayloadLengthMismatch { expected: usize, actual: usize },

    #[error("Unknown codec flag: {0}")]
    UnknownCodec(u8),

    #[error("Invalid control packet magic bytes")]
    InvalidControlMagic,

    #[error("Unknown control message type: {0}")]
    UnknownControlType(u8),

    #[error("Control packet too short")]
    ControlPacketTooShort,
}

/// Fixed-size header attached to every UDP audio packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    /// Monotonically increasing sequence number to detect drops and reordering.
    pub seq: u32,
    /// Sender's monotonic clock in nanoseconds since stream start.
    pub timestamp_ns: u64,
    /// Length of the encoded audio payload following this header in bytes.
    pub payload_len: u16,
    /// Codec identifier (0 = PCM, 1 = Opus).
    pub codec: CodecMode,
}

impl AudioHeader {
    /// Serializes the 15-byte header into the provided byte buffer.
    ///
    /// # Panics
    /// Panics if `buf.len() < AUDIO_HEADER_LEN`.
    pub fn serialize_into(&self, buf: &mut [u8]) {
        assert!(
            buf.len() >= AUDIO_HEADER_LEN,
            "buffer too small for audio header"
        );
        BigEndian::write_u32(&mut buf[0..4], self.seq);
        BigEndian::write_u64(&mut buf[4..12], self.timestamp_ns);
        BigEndian::write_u16(&mut buf[12..14], self.payload_len);
        buf[14] = self.codec as u8;
    }

    /// Deserializes a header from a slice of at least 15 bytes.
    pub fn deserialize(buf: &[u8]) -> Result<Self, ProtocolError> {
        if buf.len() < AUDIO_HEADER_LEN {
            return Err(ProtocolError::PacketTooShort {
                expected: AUDIO_HEADER_LEN,
                actual: buf.len(),
            });
        }

        let seq = BigEndian::read_u32(&buf[0..4]);
        let timestamp_ns = BigEndian::read_u64(&buf[4..12]);
        let payload_len = BigEndian::read_u16(&buf[12..14]);
        let codec_flag = buf[14];
        let codec =
            CodecMode::from_u8(codec_flag).ok_or(ProtocolError::UnknownCodec(codec_flag))?;

        Ok(AudioHeader {
            seq,
            timestamp_ns,
            payload_len,
            codec,
        })
    }
}

/// A parsed audio packet containing the fixed header and payload slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket<'a> {
    pub header: AudioHeader,
    pub payload: &'a [u8],
}

impl<'a> AudioPacket<'a> {
    /// Parses an audio packet from a received UDP datagram buffer.
    pub fn parse(buf: &'a [u8]) -> Result<Self, ProtocolError> {
        let header = AudioHeader::deserialize(buf)?;
        let expected_total = AUDIO_HEADER_LEN + header.payload_len as usize;

        if buf.len() < expected_total {
            return Err(ProtocolError::PayloadLengthMismatch {
                expected: header.payload_len as usize,
                actual: buf.len().saturating_sub(AUDIO_HEADER_LEN),
            });
        }

        let payload = &buf[AUDIO_HEADER_LEN..expected_total];
        Ok(AudioPacket { header, payload })
    }
}

/// Types of control messages sent over the dedicated control UDP port.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlMessage {
    /// Ping message sent by sender to measure round-trip time.
    Ping { id: u64, sender_send_ts_ns: u64 },
    /// Pong response sent immediately by receiver.
    Pong {
        id: u64,
        sender_send_ts_ns: u64,
        receiver_recv_ts_ns: u64,
    },
    /// Request to set receiver playback volume gain (0.0 to 1.5).
    SetVolume { volume: f32 },
    /// Telemetry stats response.
    StatsResponse {
        packets_received: u64,
        packets_lost: u64,
        loss_rate_pct: f32,
        jitter_buffer_depth_ms: f32,
        volume: f32,
        codec: u8,
    },
}

impl ControlMessage {
    /// Serializes the control message into bytes.
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        buf.extend_from_slice(&CONTROL_MAGIC);

        match self {
            ControlMessage::Ping {
                id,
                sender_send_ts_ns,
            } => {
                buf.push(0x01); // type
                let mut b = [0u8; 16];
                BigEndian::write_u64(&mut b[0..8], *id);
                BigEndian::write_u64(&mut b[8..16], *sender_send_ts_ns);
                buf.extend_from_slice(&b);
            }
            ControlMessage::Pong {
                id,
                sender_send_ts_ns,
                receiver_recv_ts_ns,
            } => {
                buf.push(0x02); // type
                let mut b = [0u8; 24];
                BigEndian::write_u64(&mut b[0..8], *id);
                BigEndian::write_u64(&mut b[8..16], *sender_send_ts_ns);
                BigEndian::write_u64(&mut b[16..24], *receiver_recv_ts_ns);
                buf.extend_from_slice(&b);
            }
            ControlMessage::SetVolume { volume } => {
                buf.push(0x03); // type
                let mut b = [0u8; 4];
                BigEndian::write_f32(&mut b, *volume);
                buf.extend_from_slice(&b);
            }
            ControlMessage::StatsResponse {
                packets_received,
                packets_lost,
                loss_rate_pct,
                jitter_buffer_depth_ms,
                volume,
                codec,
            } => {
                buf.push(0x05); // type
                let mut b = [0u8; 29];
                BigEndian::write_u64(&mut b[0..8], *packets_received);
                BigEndian::write_u64(&mut b[8..16], *packets_lost);
                BigEndian::write_f32(&mut b[16..20], *loss_rate_pct);
                BigEndian::write_f32(&mut b[20..24], *jitter_buffer_depth_ms);
                BigEndian::write_f32(&mut b[24..28], *volume);
                b[28] = *codec;
                buf.extend_from_slice(&b);
            }
        }
        buf
    }

    /// Deserializes a control message from bytes.
    pub fn deserialize(buf: &[u8]) -> Result<Self, ProtocolError> {
        if buf.len() < 3 {
            return Err(ProtocolError::ControlPacketTooShort);
        }
        if buf[0..2] != CONTROL_MAGIC {
            return Err(ProtocolError::InvalidControlMagic);
        }

        let msg_type = buf[2];
        let payload = &buf[3..];

        match msg_type {
            0x01 => {
                if payload.len() < 16 {
                    return Err(ProtocolError::ControlPacketTooShort);
                }
                let id = BigEndian::read_u64(&payload[0..8]);
                let sender_send_ts_ns = BigEndian::read_u64(&payload[8..16]);
                Ok(ControlMessage::Ping {
                    id,
                    sender_send_ts_ns,
                })
            }
            0x02 => {
                if payload.len() < 24 {
                    return Err(ProtocolError::ControlPacketTooShort);
                }
                let id = BigEndian::read_u64(&payload[0..8]);
                let sender_send_ts_ns = BigEndian::read_u64(&payload[8..16]);
                let receiver_recv_ts_ns = BigEndian::read_u64(&payload[16..24]);
                Ok(ControlMessage::Pong {
                    id,
                    sender_send_ts_ns,
                    receiver_recv_ts_ns,
                })
            }
            0x03 => {
                if payload.len() < 4 {
                    return Err(ProtocolError::ControlPacketTooShort);
                }
                let volume = BigEndian::read_f32(&payload[0..4]);
                Ok(ControlMessage::SetVolume { volume })
            }
            0x05 => {
                if payload.len() < 29 {
                    return Err(ProtocolError::ControlPacketTooShort);
                }
                let packets_received = BigEndian::read_u64(&payload[0..8]);
                let packets_lost = BigEndian::read_u64(&payload[8..16]);
                let loss_rate_pct = BigEndian::read_f32(&payload[16..20]);
                let jitter_buffer_depth_ms = BigEndian::read_f32(&payload[20..24]);
                let volume = BigEndian::read_f32(&payload[24..28]);
                let codec = payload[28];
                Ok(ControlMessage::StatsResponse {
                    packets_received,
                    packets_lost,
                    loss_rate_pct,
                    jitter_buffer_depth_ms,
                    volume,
                    codec,
                })
            }
            other => Err(ProtocolError::UnknownControlType(other)),
        }
    }
}

/// Latency and RTT estimation utilities.
///
/// Clock Synchronization Note:
/// Because the two machines' wall clocks are not synchronized, one-way latency
/// cannot be computed by subtracting sender timestamp from receiver clock.
/// Instead, we run lightweight round-trip pings. Under symmetric LAN conditions,
/// one-way network latency is estimated as approximately RTT / 2.
pub struct LatencyEstimator {
    /// Moving average RTT in nanoseconds
    rtt_ema_ns: f64,
    /// Alpha for exponential moving average
    alpha: f64,
}

impl LatencyEstimator {
    pub fn new(alpha: f64) -> Self {
        Self {
            rtt_ema_ns: 0.0,
            alpha: alpha.clamp(0.01, 1.0),
        }
    }

    /// Record a round-trip ping measurement.
    /// `send_ts_ns`: Monotonic timestamp when Ping was sent by sender.
    /// `recv_ts_ns`: Monotonic timestamp when Pong was received back by sender.
    pub fn record_ping(&mut self, send_ts_ns: u64, recv_ts_ns: u64) -> (f64, f64) {
        let rtt_ns = recv_ts_ns.saturating_sub(send_ts_ns) as f64;
        if self.rtt_ema_ns == 0.0 {
            self.rtt_ema_ns = rtt_ns;
        } else {
            self.rtt_ema_ns = self.alpha * rtt_ns + (1.0 - self.alpha) * self.rtt_ema_ns;
        }

        let rtt_ms = self.rtt_ema_ns / 1_000_000.0;
        let estimated_one_way_ms = rtt_ms / 2.0;
        (rtt_ms, estimated_one_way_ms)
    }

    pub fn current_rtt_ms(&self) -> f64 {
        self.rtt_ema_ns / 1_000_000.0
    }

    pub fn current_estimated_one_way_ms(&self) -> f64 {
        self.current_rtt_ms() / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_header_roundtrip() {
        let header = AudioHeader {
            seq: 42,
            timestamp_ns: 1234567890,
            payload_len: 960,
            codec: CodecMode::Pcm,
        };

        let mut buf = [0u8; AUDIO_HEADER_LEN];
        header.serialize_into(&mut buf);

        let parsed = AudioHeader::deserialize(&buf).unwrap();
        assert_eq!(header, parsed);
    }

    #[test]
    fn test_audio_packet_parse() {
        let payload_data = vec![0x12, 0x34, 0x56, 0x78];
        let header = AudioHeader {
            seq: 100,
            timestamp_ns: 5000,
            payload_len: payload_data.len() as u16,
            codec: CodecMode::Opus,
        };

        let mut packet_buf = vec![0u8; AUDIO_HEADER_LEN + payload_data.len()];
        header.serialize_into(&mut packet_buf[0..AUDIO_HEADER_LEN]);
        packet_buf[AUDIO_HEADER_LEN..].copy_from_slice(&payload_data);

        let packet = AudioPacket::parse(&packet_buf).unwrap();
        assert_eq!(packet.header, header);
        assert_eq!(packet.payload, payload_data.as_slice());
    }

    #[test]
    fn test_control_ping_pong_roundtrip() {
        let ping = ControlMessage::Ping {
            id: 7,
            sender_send_ts_ns: 100_000_000,
        };
        let bytes = ping.serialize();
        let parsed = ControlMessage::deserialize(&bytes).unwrap();
        assert_eq!(ping, parsed);

        let pong = ControlMessage::Pong {
            id: 7,
            sender_send_ts_ns: 100_000_000,
            receiver_recv_ts_ns: 101_500_000,
        };
        let bytes = pong.serialize();
        let parsed = ControlMessage::deserialize(&bytes).unwrap();
        assert_eq!(pong, parsed);
    }

    #[test]
    fn test_control_volume_roundtrip() {
        let vol = ControlMessage::SetVolume { volume: 0.85 };
        let bytes = vol.serialize();
        let parsed = ControlMessage::deserialize(&bytes).unwrap();
        if let ControlMessage::SetVolume { volume } = parsed {
            assert!((volume - 0.85).abs() < 1e-6);
        } else {
            panic!("Expected SetVolume message");
        }
    }

    #[test]
    fn test_latency_estimator() {
        let mut est = LatencyEstimator::new(0.5);
        // 2ms RTT
        let (rtt_ms, one_way_ms) = est.record_ping(1_000_000, 3_000_000);
        assert!((rtt_ms - 2.0).abs() < 1e-4);
        assert!((one_way_ms - 1.0).abs() < 1e-4);
    }
}
