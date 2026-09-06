//! mDNS Discovery for macOS Receiver
//!
//! Discovers the macOS receiver broadcasting `_lanaudio._udp.local.` on the LAN.

use anyhow::{anyhow, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub const MDNS_SERVICE_TYPE: &str = "_lanaudio._udp.local.";

/// Attempt to discover the macOS receiver on the local network via mDNS.
pub async fn discover_receiver(timeout: Duration) -> Result<SocketAddr> {
    info!(
        "Searching for macOS receiver on LAN via mDNS ('{}')...",
        MDNS_SERVICE_TYPE
    );

    let mdns =
        ServiceDaemon::new().map_err(|e| anyhow!("Failed to initialize mDNS daemon: {}", e))?;
    let receiver = mdns
        .browse(MDNS_SERVICE_TYPE)
        .map_err(|e| anyhow!("Failed to browse mDNS services: {}", e))?;

    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        // Non-blocking poll for discovery events
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let port = info.get_port();
                let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();

                info!(
                    "Discovered receiver: '{}' at {:?}:{}",
                    info.get_fullname(),
                    addrs,
                    port
                );

                // Prioritize IPv4 addresses for maximum compatibility
                if let Some(ipv4) = addrs.iter().find(|ip| ip.is_ipv4()) {
                    let addr = SocketAddr::new(*ipv4, port);
                    info!("Connecting to receiver at: {}", addr);
                    let _ = mdns.stop_browse(MDNS_SERVICE_TYPE);
                    return Ok(addr);
                } else if let Some(ip) = addrs.first() {
                    let addr = SocketAddr::new(*ip, port);
                    info!("Connecting to receiver at: {}", addr);
                    let _ = mdns.stop_browse(MDNS_SERVICE_TYPE);
                    return Ok(addr);
                }
            }
            Ok(other) => {
                tracing::debug!("mDNS event: {:?}", other);
            }
            Err(_) => {
                // Timeout on this single poll, loop until deadline
            }
        }
    }

    let _ = mdns.stop_browse(MDNS_SERVICE_TYPE);
    warn!(
        "mDNS discovery timed out after {:.1}s. This may occur if Wi-Fi client isolation \
         is enabled on your router, or if the receiver is not yet started.",
        timeout.as_secs_f32()
    );

    Err(anyhow!(
        "mDNS auto-discovery timed out. Fall back to manual IP entry:\n\
         Run the receiver on macOS, run `ipconfig getifaddr en0` to find its IP, \
         and launch the sender with: `sender.exe --target-ip <MAC_IP>`"
    ))
}
