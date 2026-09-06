# LAN Audio Wire Protocol Specification

This document details the binary layout of audio packets, control messages, and the round-trip latency estimation mechanism used in **LAN Audio**.

---

## 1. UDP Audio Streaming Packet

Every audio packet sent from the Windows sender to the macOS receiver is a single UDP datagram with a fixed 15-byte header followed by the audio payload.

### Binary Header Layout (15 Bytes, Big-Endian)

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                       Sequence Number                         | (4 bytes, u32)
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|                 Sender Monotonic Timestamp                    | (8 bytes, u64)
|                     (nanoseconds)                             |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|         Payload Length        |  Codec Flag   |               |
|            (2 bytes)          |    (1 byte)   |               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+               |
|                                                               |
|                    Audio Payload Data                         | (variable length)
|                 (PCM S16LE or Opus bytes)                     |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Field Definitions

| Field | Type | Size | Description |
| :--- | :--- | :--- | :--- |
| `Sequence Number` | `u32` (BE) | 4 bytes | Monotonically increasing counter (`0, 1, 2, ...`). Enables the receiver to detect packet loss, identify gaps, and reorder out-of-order packets. |
| `Sender Monotonic Timestamp`| `u64` (BE) | 8 bytes | Sender's monotonic elapsed time in nanoseconds since audio stream start (`std::time::Instant`). Used for jitter calculation and sender telemetry. |
| `Payload Length` | `u16` (BE) | 2 bytes | Exact length of the following audio payload in bytes. |
| `Codec Flag` | `u8` | 1 byte | Codec format: `0x00` = Raw PCM (S16LE stereo), `0x01` = Opus. |
| `Audio Payload` | `[u8]` | Variable | The actual audio samples. |

### Codec Formats

- **Raw PCM (`0x00`)**:
  - Sample Rate: 48,000 Hz
  - Channels: 2 (Stereo, interleaved: L, R, L, R...)
  - Sample Format: 16-bit signed integer little-endian (`i16`)
  - Frame Duration: Typically 5.0ms (240 samples/ch = 960 bytes) or 2.5ms (120 samples/ch = 480 bytes).
  - Advantage: Zero algorithmic delay, zero CPU encode/decode overhead, no compression artifacts. Fits cleanly inside standard 1500-byte MTU without IP fragmentation.
- **Opus (`0x01`)**:
  - Sample Rate: 48,000 Hz
  - Channels: 2 (Stereo)
  - Frame Duration: Configurable 2.5ms (120 samples), 5ms (240 samples), 10ms (480 samples), or 20ms (960 samples). Sane default: 10ms.
  - Advantage: Drastically lower bandwidth (~64–160 kbps vs ~1.5 Mbps for PCM), ideal for congested Wi-Fi links.

---

## 2. Control & Ping Protocol (Dedicated UDP Port)

To preserve strict real-time audio pipeline predictability, control messages (ping, pong, volume adjustments, telemetry stats) are transmitted over a separate UDP socket (default port `AUDIO_PORT + 1 = 40001`).

### Packet Format
- **Magic**: `[0x4C, 0x41]` (ASCII `'L'`, `'A'`)
- **Message Type**: `u8`
  - `0x01` = `Ping`: `[id: u64, sender_send_ts_ns: u64]` (19 bytes total)
  - `0x02` = `Pong`: `[id: u64, sender_send_ts_ns: u64, receiver_recv_ts_ns: u64]` (27 bytes total)
  - `0x03` = `SetVolume`: `[volume: f32]` (7 bytes total)
  - `0x05` = `StatsResponse`: Telemetry stats payload.

---

## 3. Ping-Based Round-Trip Latency Estimation

### The Synchronization Challenge
In a distributed home network without PTP (Precision Time Protocol) hardware or microsecond-precision NTP synchronization, the wall clocks of a Windows PC and a MacBook will typically drift by tens to hundreds of milliseconds. Subtracting the sender's wall clock from the receiver's wall clock produces wildly inaccurate and misleading latency numbers (often negative or inflated).

### Our Approach: Periodic Round-Trip Ping
1. **Periodic Ping**: Every 2 seconds, the sender transmits a lightweight `Ping` message containing its current monotonic timestamp ($T_{send}$) to the receiver's control port.
2. **Immediate Pong**: Upon receipt, the receiver immediately replies with a `Pong` echoing $T_{send}$.
3. **Round-Trip Calculation**: When the sender receives the `Pong` at monotonic time $T_{recv}$, it calculates the Round-Trip Time:
   $$\text{RTT} = T_{recv} - T_{send}$$
4. **One-Way Latency Estimate**: Under standard LAN switching and Wi-Fi conditions, network propagation is largely symmetric. The one-way network latency is estimated as:
   $$\text{Estimated One-Way Latency} \approx \frac{\text{RTT}}{2}$$
5. **Smoothing**: An Exponential Moving Average (EMA) with $\alpha = 0.3$ is applied to dampen instantaneous Wi-Fi transmission spikes:
   $$\text{RTT}_{smoothed} = \alpha \cdot \text{RTT}_{new} + (1 - \alpha) \cdot \text{RTT}_{previous}$$

### Total End-to-End Latency Breakdown
The true acoustic end-to-end latency is the sum of:
$$\text{Total Latency} \approx \tau_{\text{WASAPI capture}} + \tau_{\text{codec encode}} + \tau_{\text{network (one-way)}} + \tau_{\text{jitter buffer}} + \tau_{\text{CoreAudio render}}$$

- **PCM Mode**: $\tau_{\text{encode}} = 0\text{ ms}$, $\tau_{\text{network}} \approx 0.5\text{--}1.5\text{ ms}$, $\tau_{\text{jitter}} \approx 10\text{--}20\text{ ms}$. Typical end-to-end: **~15–25 ms**.
- **Opus Mode (10ms frame)**: $\tau_{\text{encode}} \approx 10\text{ ms}$ algorithmic lookahead + frame duration, $\tau_{\text{network}} \approx 0.5\text{--}1.5\text{ ms}$, $\tau_{\text{jitter}} \approx 20\text{ ms}$. Typical end-to-end: **~30–45 ms**.

> [!NOTE]
> All latency figures presented in the logs and web dashboard are explicitly labeled as **estimates derived from RTT/2**, not lab-grade hardware measurements.
