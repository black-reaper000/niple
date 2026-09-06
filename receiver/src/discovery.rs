//! mDNS / Bonjour Service Advertisement for macOS
//!
//! Advertises `_lanaudio._udp.local.` so the Windows sender discovers
//! this receiver without requiring manual IP entry.

use anyhow::{anyhow, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use tracing::{info, warn};

pub const MDNS_SERVICE_TYPE: &str = "_lanaudio._udp.local.";

#[allow(dead_code)]
pub struct ServiceAdvertiser {
    mdns: ServiceDaemon,
    fullname: String,
}

impl ServiceAdvertiser {
    pub fn new(audio_port: u16, control_port: u16) -> Result<Self> {
        let mdns =
            ServiceDaemon::new().map_err(|e| anyhow!("Failed to initialize mDNS daemon: {}", e))?;

        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("USER"))
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "MacBook".into())
            .replace(".local", "");
        let instance_name = format!("{}-Receiver", hostname);
        let host_name = format!("{}.local.", hostname);

        let mut properties = HashMap::new();
        properties.insert("version".to_string(), "0.1.0".to_string());
        properties.insert("control_port".to_string(), control_port.to_string());
        properties.insert("codecs".to_string(), "pcm,opus".to_string());

        let service_info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &instance_name,
            &host_name,
            "",
            audio_port,
            properties,
        )
        .map_err(|e| anyhow!("Failed to create mDNS ServiceInfo: {}", e))?;

        let fullname = service_info.get_fullname().to_string();

        mdns.register(service_info)
            .map_err(|e| anyhow!("Failed to register mDNS service: {}", e))?;

        info!(
            "mDNS Service advertised: '{}' on audio port {}, control port {}",
            fullname, audio_port, control_port
        );

        Ok(Self { mdns, fullname })
    }

    #[allow(dead_code)]
    pub fn unregister(&self) {
        if let Err(e) = self.mdns.unregister(&self.fullname) {
            warn!("Failed to unregister mDNS service: {}", e);
        }
    }
}
