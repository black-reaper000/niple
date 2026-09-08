# LAN Audio :

> **Native Low-Latency Windows &rarr; Mac Wireless Speaker**
> Turn your MacBook into a high-fidelity, real-time wireless speaker for your Windows PC over your local home network (LAN). Optimized for the **lowest achievable latency** with zero cloud relays, zero screen-capture pipeline overhead, and no WebRTC bloat.

---

## Why Not a Browser or WebRTC?

- **Zero Screen-Capture Overhead**: WebRTC solutions relying on `getDisplayMedia()` loopback capture in Chromium add an unavoidable **100–300 ms** buffering and pipeline delay before audio ever reaches the network stack. LAN Audio bypasses this completely by capturing raw audio directly at the hardware driver level using **Windows WASAPI loopback**.
- **No WebRTC Handshake or Processing**: WebRTC incurs DTLS-SRTP handshakes, ICE candidates, jitter buffering, and mandatory Acoustic Echo Cancellation (AEC) / Automatic Gain Control (AGC) tailored for microphones. System audio from PC games, media, or DAWs does not need AEC/AGC. LAN Audio transmits directly over bare UDP packets.
- **Direct Peer-to-Peer UDP**: Audio travels directly between your two devices over UDP. Nothing is relayed. No cloud server, no TURN server, and no telemetry.

---

## Architecture Overview

```
 [ Windows PC ]                                              [ MacBook ]
+----------------------------+                            +-----------------------------+
| System Audio (WASAPI)      |                            | CoreAudio Output Device     |
|   |                        |                            |   ^                         |
|   v                        |                            |   |                         |
| Format Norm (48kHz Stereo) |                            | Volume Scaling (0-150%)     |
|   |                        |                            |   ^                         |
|   v                        |                            |   |                         |
| Encoder (PCM / Opus)       |                            | Audio Decoder (PCM / Opus)  |
|   |                        |                            |   ^                         |
|   v                        |                            |   |                         |
| UDP Audio Socket (40000)   | ====== Direct UDP =======> | Adaptive Jitter Buffer      |
|                            |      (Audio Stream)        |   ^                         |
| UDP Control/Ping (40001)   | <===== Ping / Pong ======> | UDP Audio Socket (40000)    |
|                            |      (RTT Measurement)     | UDP Control Server (40001)  |
| mDNS Discovery Browser     | ...... _lanaudio ......--> | mDNS Service Advertiser     |
+----------------------------+                            |                             |
                                                          | Optional Web Dashboard      |
                                                          | (HTTPS:8443, self-signed)   |
                                                          +-----------------------------+
```

---

## Project Structure

```
lan-audio/
├── Cargo.toml                  # Cargo workspace manifest
├── README.md                   # Full system documentation
├── .env.example                # Sample environment configuration
├── .gitignore                  # Git ignore rules
├── .github/workflows/ci.yml    # Multi-platform CI (Windows & macOS)
├── protocol/                   # Shared binary packet format & latency estimator
│   ├── Cargo.toml
│   ├── README.md               # Detailed packet wire layout & ping specification
│   └── src/lib.rs
├── sender/                     # Windows WASAPI loopback capture & UDP sender
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # CLI arguments & main stream loop
│       ├── audio.rs            # WASAPI loopback capture via cpal
│       ├── encoder.rs          # Low-latency PCM and Opus encoders
│       ├── discovery.rs        # mDNS receiver discovery
│       └── ping.rs             # Round-trip ping latency estimator
├── receiver/                   # macOS CoreAudio playback & jitter buffer
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # CLI arguments & playback loop
│       ├── audio.rs            # CoreAudio playback & lock-free volume gain
│       ├── decoder.rs          # PCM and Opus decoders with packet loss concealment
│       ├── jitter_buffer.rs    # Adaptive reordering jitter buffer
│       ├── discovery.rs        # mDNS Bonjour service advertisement
│       └── control.rs          # Control server & ping responder
└── dashboard/                  # Optional self-signed HTTPS web control UI
    ├── Cargo.toml
    └── src/
        ├── lib.rs              # Axum HTTPS server & telemetry endpoints
        ├── main.rs             # Standalone dashboard binary runner
        ├── tls.rs              # Self-signed certificate generator/loader
        └── static/
            ├── index.html      # Glassmorphism dark-mode UI
            ├── style.css       # Clean aesthetics & micro-animations
            └── app.js          # Real-time WebSocket/polling telemetry & controls
```

---

## Exact Build Commands

### On Windows (Sender)
Ensure the Rust toolchain is installed (`rustup`). Then run:

```powershell
# Build sender in release mode
cargo build --release -p sender

# Binary will be located at:
# .\target\release\sender.exe
```

### On macOS (Receiver & Dashboard)
Ensure Rust and Xcode command-line tools (`xcode-select --install`) are installed:

```bash
# Build receiver and dashboard in release mode
cargo build --release -p receiver -p dashboard

# Binaries will be located at:
# ./target/release/receiver
# ./target/release/dashboard
```

### Workspace Build (All Packages)
```bash
cargo build --release
```

---

## Exact Run Commands

### 1. Start the Receiver on macOS

```bash
# Basic run (Auto-advertises via mDNS on port 40000, control on 40001, dashboard on 8443)
./target/release/receiver

# Or with custom flags:
./target/release/receiver \
  --port 40000 \
  --control-port 40001 \
  --jitter-buffer-ms 20 \
  --dashboard \
  --dashboard-port 8443
```

#### Available Receiver Flags:
- `--listen-ip <IP>` (default: `0.0.0.0`): Local IP address to bind.
- `--port <PORT>` (default: `40000`): UDP port for incoming audio.
- `--control-port <PORT>` (default: `40001`): UDP port for ping and volume controls.
- `--jitter-buffer-ms <MS>` (default: `20`): Target jitter buffer depth.
- `--device <NAME>`: Substring filter for macOS audio output device (e.g. `"MacBook Pro Speakers"`).
- `--list-devices`: Enumerate available macOS audio output devices and exit.
- `--no-mdns`: Disable mDNS Bonjour advertisement.
- `--dashboard`: Enable local self-signed HTTPS dashboard (default: `true`).
- `--dashboard-port <PORT>` (default: `8443`): HTTPS dashboard port.

---

### 2. Start the Sender on Windows

```powershell
# Auto-discovery mode (uses mDNS to find the Mac receiver automatically)
.\target\release\sender.exe

# Or specify the Mac's LAN IP directly (if mDNS is blocked by Wi-Fi isolation):
.\target\release\sender.exe --target-ip 10.32.148.50

# Lowest-latency uncompressed PCM mode on Gigabit LAN:
.\target\release\sender.exe --target-ip 10.32.148.50 --codec pcm --frame-size-ms 5

# Wi-Fi friendly Opus compressed mode (default):
.\target\release\sender.exe --target-ip 10.32.148.50 --codec opus --frame-size-ms 10
```

#### Available Sender Flags:
- `--target-ip <IP>` (default: `"auto"`): Receiver IP address or `"auto"` to browse via mDNS.
- `--port <PORT>` (default: `40000`): UDP audio port.
- `--control-port <PORT>` (default: `40001`): UDP control & ping port.
- `--codec <pcm|opus>` (default: `"opus"`): Audio compression mode.
- `--frame-size-ms <MS>` (default: `10.0`): Frame chunk duration in ms (`2.5`, `5`, `10`, `20`).
- `--device <NAME>`: Output device to loopback (defaults to default Windows playback device).
- `--list-devices`: Enumerate available Windows audio endpoints for loopback capture and exit.

---

## How to Find Your LAN IP Address

If mDNS auto-discovery fails (e.g. due to Wi-Fi client isolation on your router):

### On macOS:
Open Terminal and run:
```bash
ipconfig getifaddr en0
```
*(If on Wi-Fi, this will output an IP such as `10.32.148.50` or `192.168.1.45`)*.

### On Windows:
Open PowerShell and run:
```powershell
ipconfig
```
Look for **IPv4 Address** under your active Wi-Fi or Ethernet adapter (e.g. `10.32.148.69`).

---

## Codec Modes & Latency Optimization Trade-offs

| Feature | Raw PCM Mode (`--codec pcm`) | Opus Mode (`--codec opus`) |
| :--- | :--- | :--- |
| **Algorithmic Delay** | **0.0 ms** (instant sample packing) | **~2.5 – 10 ms** (lookahead + frame duration) |
| **Bandwidth** | **~1.536 Mbps** (48kHz &times; 16-bit &times; 2 channels) | **~64 – 160 kbps** (~90% bandwidth reduction) |
| **Packet Size** | 960 bytes per 5ms frame (fits within standard MTU) | ~80 – 160 bytes per frame |
| **Best Used For** | Gigabit Ethernet or clear 5GHz/6GHz Wi-Fi | Congested 2.4GHz Wi-Fi or multi-hop LANs |
| **CPU Overhead** | Minimal (simple integer conversion) | Low (highly optimized Opus SIMD routines) |

### Recommended Jitter Buffer Pairings
- **Ultra-low latency LAN**: `--codec pcm --frame-size-ms 5` on sender, with `--jitter-buffer-ms 15` on receiver. End-to-end latency: **~15–20 ms**.
- **Balanced LAN / Wi-Fi**: `--codec opus --frame-size-ms 10` on sender, with `--jitter-buffer-ms 20` on receiver. End-to-end latency: **~30–35 ms**.

---

## Latency Estimation & Clock Synchronization

Because two different computers have non-synchronized wall clocks that drift over time, subtracting the sender's timestamp from the receiver's clock yields invalid or negative latency figures.

Instead, LAN Audio implements a **lightweight periodic round-trip ping**:
1. Every 2 seconds, the sender transmits a UDP `Ping` packet containing its local monotonic timestamp ($T_1$) to receiver port 40001.
2. The receiver immediately replies with a `Pong` packet echoing $T_1$.
3. The sender receives the reply at monotonic time $T_2$, calculating:
   $$\text{Round-Trip Time (RTT)} = T_2 - T_1$$
4. Under symmetric LAN conditions, one-way network latency is estimated as:
   $$\text{Estimated One-Way Latency} \approx \frac{\text{RTT}}{2}$$
5. An exponential moving average (EMA) is applied to prevent spurious spikes from distorting the display.

> [!NOTE]
> This figure represents **network transmission latency**. The full acoustic end-to-end latency includes WASAPI loopback capture buffer time (~5–10ms) and the receiver jitter buffer depth (default 20ms).

---

## Self-Signed HTTPS Control Dashboard

The macOS receiver includes an optional, non-blocking local web UI running on `https://<mac-ip>:8443` (or `https://localhost:8443`).

### Generating the Certificate

The dashboard automatically generates an in-memory or persisted self-signed certificate using `rcgen` upon first run. Alternatively, you can generate your own using `openssl`:

```bash
mkdir -p certs
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout certs/key.pem \
  -out certs/cert.pem \
  -days 365 \
  -subj "/CN=lan-audio.local"
```

### Trusting the Certificate in Your Browser

Because the certificate is self-signed for local LAN use, visiting `https://<mac-ip>:8443` from Chrome, Safari, or Firefox will display a security warning:
1. **Chrome / Edge**: Click **Advanced** &rarr; **Proceed to <mac-ip> (unsafe)**.
2. **Safari**: Click **Show Details** &rarr; **Visit this website** &rarr; confirm with Touch ID / password.
3. This warning is completely normal and expected for self-signed LAN certificates.

### Non-Audio-Critical Architecture
The dashboard is strictly a management convenience layer:
- Master volume changes made on the dashboard apply **directly to the CoreAudio output gain multiplier on the receiver**.
- The sender continues streaming at reference line level regardless of dashboard volume settings.
- If the dashboard is closed or unbuilt, audio streaming continues uninterrupted.

---

## Error Handling & Diagnostics

| Failure Mode | Behavior & Diagnostic Message |
| :--- | :--- |
| **No output device on Windows** | Logs diagnostic: `"No default audio output device found! WASAPI loopback requires an active playback device (e.g. Speakers, HDMI output, or VB-Audio Virtual Cable). Please enable an audio endpoint in Windows Sound settings."` |
| **No output device on macOS** | Logs: `"No default audio output device found! Please connect speakers/headphones or select a valid output device in macOS Sound Preferences."` |
| **Output device unplugged mid-stream** | Audio callback catches stream error, pads buffer with silence, and logs descriptive error without crashing. |
| **mDNS discovery blocked** | After 5s timeout, reports: `"mDNS auto-discovery timed out. Fall back to manual IP entry: sender.exe --target-ip <MAC_IP>"` |
| **UDP port in use** | Returns clean error indicating which port is already bound. |
| **Receiver unreachable** | After 3 missed pings, logs diagnostic: `"[DIAGNOSTIC] Receiver not responding after N attempts. Check both devices are on the same LAN and not on a guest/isolated Wi-Fi network."` |
| **High packet loss (> 2%)** | Surfaced in logs and dashboard: `"[WARNING] Packet loss is X.XX%, exceeding 2.0% threshold. Check Wi-Fi interference or increase jitter buffer."` |
| **Opus frame decode failure** | Drops corrupted frame, logs warning, and continues playback without interrupting stream. |

---

## Known OS-Level Limitations

1. **Windows Silent Stream Behavior**: On Windows, WASAPI loopback streams produce silent buffers if no applications are currently playing sound. If your PC is connected to a monitor with no speakers, ensure Windows default playback is directed to an active endpoint (such as HDMI audio or a virtual cable like **VB-Audio Virtual Cable**).
2. **macOS Microphone / Audio Permissions**: In modern macOS versions, terminal binaries that access audio hardware may trigger a system permission prompt. If audio does not play, verify that your terminal emulator (Terminal, iTerm2, etc.) has permission under **System Settings &rarr; Privacy & Security &rarr; Microphone / Accessibility**.
3. **Router Client Isolation**: Some guest Wi-Fi networks and mesh routers enable "Client Isolation" by default, preventing UDP traffic between devices on the same Wi-Fi. Ensure both the PC and MacBook are on your primary home Wi-Fi or Ethernet.
4. **Platform-Specific Audio Drivers**: WASAPI loopback is specific to Windows and CoreAudio is specific to macOS. This software is designed natively for Windows &rarr; Mac streaming.

---

## Security & Privacy

- **Audio Data Isolation**: Audio data travels directly peer-to-peer over your local home network using UDP.
- **No Cloud Relay**: No intermediate servers, STUN/TURN relays, or third-party cloud services are used.
- **Zero Audio Storage**: No audio is ever recorded, written to disk, or logged.
