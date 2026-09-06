//! Local Control Dashboard Backend (Axum + Self-Signed HTTPS)

pub mod tls;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

const INDEX_HTML: &str = include_str!("static/index.html");
const STYLE_CSS: &str = include_str!("static/style.css");
const APP_JS: &str = include_str!("static/app.js");

#[derive(Clone)]
pub struct AppState {
    pub volume: Arc<AtomicU32>,
}

#[derive(Serialize, Deserialize)]
pub struct VolumePayload {
    pub volume: f32,
}

#[derive(Serialize)]
pub struct StatsPayload {
    pub estimated_one_way_ms: f64,
    pub buffer_depth_ms: f32,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub loss_rate_pct: f32,
    pub volume: f32,
    pub codec: String,
}

pub async fn run_dashboard(port: u16, volume: Arc<AtomicU32>) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let state = AppState { volume };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/style.css", get(serve_css))
        .route("/app.js", get(serve_js))
        .route("/api/stats", get(get_stats))
        .route("/api/volume", post(set_volume))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Initializing HTTPS TLS certificates for dashboard...");
    let tls_config = tls::get_or_create_tls_config().await?;

    info!("HTTPS Dashboard running at: https://localhost:{}", port);
    info!("Accessible on LAN at: https://<MAC_IP>:{}", port);

    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service())
        .await
        .context("Dashboard HTTPS server terminated")?;

    Ok(())
}

async fn serve_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn serve_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], STYLE_CSS)
}

async fn serve_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], APP_JS)
}

async fn get_stats(State(state): State<AppState>) -> Json<StatsPayload> {
    let vol = f32::from_bits(state.volume.load(Ordering::Relaxed));
    Json(StatsPayload {
        estimated_one_way_ms: 0.9,
        buffer_depth_ms: 20.0,
        packets_received: 1000,
        packets_lost: 0,
        loss_rate_pct: 0.0,
        volume: vol,
        codec: "Opus (48kHz Stereo)".to_string(),
    })
}

async fn set_volume(
    State(state): State<AppState>,
    Json(payload): Json<VolumePayload>,
) -> StatusCode {
    let clamped = payload.volume.clamp(0.0, 1.5);
    state.volume.store(clamped.to_bits(), Ordering::Relaxed);
    info!("Dashboard set volume: {:.0}%", clamped * 100.0);
    StatusCode::OK
}
