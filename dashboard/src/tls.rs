//! Self-Signed TLS Certificate Generation & Loading for HTTPS Dashboard

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use rcgen::generate_simple_self_signed;
use std::path::Path;
use tracing::info;

/// Loads existing TLS certificates from disk or automatically generates
/// an in-memory self-signed certificate for local HTTPS serving.
pub async fn get_or_create_tls_config() -> Result<RustlsConfig> {
    let cert_path = Path::new("certs/cert.pem");
    let key_path = Path::new("certs/key.pem");

    if cert_path.exists() && key_path.exists() {
        info!("Loading existing TLS certificate from certs/ directory");
        RustlsConfig::from_pem_file(cert_path, key_path)
            .await
            .context("Failed to load TLS certificates from disk")
    } else {
        info!("Generating ephemeral self-signed TLS certificate for local HTTPS dashboard...");
        let subject_alt_names = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "lan-audio.local".to_string(),
        ];

        let cert = generate_simple_self_signed(subject_alt_names)
            .context("Failed to generate self-signed certificate")?;

        let cert_pem = cert.cert.pem();
        let key_pem = cert.key_pair.serialize_pem();

        // Optionally persist to certs/ directory for reuse
        if let Err(e) = std::fs::create_dir_all("certs") {
            tracing::debug!("Could not create certs/ directory: {}", e);
        } else {
            let _ = std::fs::write(cert_path, &cert_pem);
            let _ = std::fs::write(key_path, &key_pem);
            info!("Saved self-signed certificate to certs/cert.pem and key to certs/key.pem");
        }

        RustlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes())
            .await
            .context("Failed to configure rustls with generated self-signed certificate")
    }
}
