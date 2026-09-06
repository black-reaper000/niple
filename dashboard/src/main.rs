//! Standalone Runner for LAN Audio Dashboard

use anyhow::Result;
use clap::Parser;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "lan-audio-dashboard",
    about = "Local HTTPS Control Dashboard for LAN Audio"
)]
struct Args {
    #[arg(short, long, default_value_t = 8443)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));

    println!(
        "Starting standalone LAN Audio Dashboard on https://localhost:{}",
        args.port
    );
    dashboard::run_dashboard(args.port, volume).await?;
    Ok(())
}
