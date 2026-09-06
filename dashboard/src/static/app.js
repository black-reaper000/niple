// LAN Audio Dashboard Frontend Client

document.addEventListener('DOMContentLoaded', () => {
    const slider = document.getElementById('volume-slider');
    const volumeDisplay = document.getElementById('volume-display');
    const valLatency = document.getElementById('val-latency');
    const valRtt = document.getElementById('val-rtt');
    const valBuffer = document.getElementById('val-buffer');
    const valLoss = document.getElementById('val-loss');
    const valLossCount = document.getElementById('val-loss-count');
    const valCodec = document.getElementById('val-codec');
    const meterLatency = document.getElementById('meter-latency');
    const meterBuffer = document.getElementById('meter-buffer');
    const meterLoss = document.getElementById('meter-loss');
    const presetButtons = document.querySelectorAll('.btn-preset');

    let debounceTimer = null;

    function updateVolumeDisplay(vol) {
        volumeDisplay.textContent = `${vol}%`;
    }

    async function sendVolume(volPercent) {
        const volumeFraction = parseFloat(volPercent) / 100.0;
        try {
            await fetch('/api/volume', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ volume: volumeFraction })
            });
        } catch (e) {
            console.error('Failed to set volume:', e);
        }
    }

    slider.addEventListener('input', (e) => {
        const val = e.target.value;
        updateVolumeDisplay(val);

        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            sendVolume(val);
        }, 50);
    });

    presetButtons.forEach(btn => {
        btn.addEventListener('click', () => {
            const val = btn.getAttribute('data-vol');
            slider.value = val;
            updateVolumeDisplay(val);
            sendVolume(val);
        });
    });

    async function fetchStats() {
        try {
            const res = await fetch('/api/stats');
            if (res.ok) {
                const data = await res.json();

                // Latency (Estimated RTT / 2)
                const latencyMs = data.estimated_one_way_ms || 0.8;
                valLatency.innerHTML = `~${latencyMs.toFixed(1)} <span class="unit">ms</span>`;
                valRtt.textContent = `Derived from RTT: ${(latencyMs * 2).toFixed(1)} ms (RTT ÷ 2)`;
                meterLatency.style.width = `${Math.min(100, (latencyMs / 20.0) * 100)}%`;

                // Jitter Buffer Depth
                const bufMs = data.buffer_depth_ms || 20.0;
                valBuffer.innerHTML = `${bufMs.toFixed(1)} <span class="unit">ms</span>`;
                meterBuffer.style.width = `${Math.min(100, (bufMs / 50.0) * 100)}%`;

                // Packet Loss
                const lossPct = data.loss_rate_pct || 0.0;
                valLoss.innerHTML = `${lossPct.toFixed(2)} <span class="unit">%</span>`;
                valLossCount.textContent = `${data.packets_lost || 0} dropped packets detected`;
                meterLoss.style.width = `${Math.min(100, lossPct * 10)}%`;

                if (lossPct > 2.0) {
                    meterLoss.className = 'meter-fill accent-red';
                } else {
                    meterLoss.className = 'meter-fill success';
                }

                // Codec
                if (data.codec) {
                    valCodec.textContent = data.codec;
                }

                // Volume Sync (only if not actively dragging)
                if (data.volume !== undefined && document.activeElement !== slider) {
                    const currentPercent = Math.round(data.volume * 100.0);
                    slider.value = currentPercent;
                    updateVolumeDisplay(currentPercent);
                }
            }
        } catch (e) {
            // Dashboard or receiver offline
            document.getElementById('status-text').textContent = 'Connecting...';
            document.getElementById('stream-status').className = 'status-badge';
        }
    }

    // Poll telemetry stats every 1 second
    setInterval(fetchStats, 1000);
    fetchStats();
});
