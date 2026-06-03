// ABOUTME: HTTP REST API server for mqttaudio.
// ABOUTME: Provides endpoints mirroring MQTT commands, status queries, and WebSocket for log streaming.

mod handlers;
mod routes;
mod websocket;

pub use routes::create_router;
pub use websocket::LogBroadcaster;

use crate::audio::mixer::MixerState;
use crate::cache::CacheManager;
use crate::config::HttpConfig;
use crate::voice::VoiceManager;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Shared application state passed to all HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    /// Channel to send commands (same as MQTT uses)
    pub cmd_tx: mpsc::Sender<String>,
    /// Read-only access to mixer state for status queries
    pub mixer_state: Arc<Mutex<MixerState>>,
    /// Read-only access to voice manager for status queries
    pub voice_manager: Arc<Mutex<VoiceManager>>,
    /// Read-only access to cache manager for status queries
    pub cache_manager: Arc<tokio::sync::Mutex<CacheManager>>,
    /// Optional auth token for Bearer authentication
    pub auth_token: Option<String>,
    /// Opt-in: require a valid token on ALL routes (status + ws included).
    pub require_auth: bool,
    /// Log broadcaster for WebSocket clients
    pub log_broadcaster: Arc<LogBroadcaster>,
}

/// Return a warning when the HTTP control API is exposed on a non-loopback
/// address without authentication. Pure, for testing and a startup log.
pub fn exposure_warning(
    addr: &SocketAddr,
    auth_token: &Option<String>,
    require_auth: bool,
) -> Option<String> {
    if !addr.ip().is_loopback() && auth_token.is_none() && !require_auth {
        Some(format!(
            "HTTP control API exposed on {} without authentication. Set http.auth_token \
             and http.require_auth to restrict access, or bind to 127.0.0.1.",
            addr
        ))
    } else {
        None
    }
}

/// Start the HTTP server.
/// Returns the actual bound address (useful when port 0 is used for auto-selection).
pub async fn start_server(
    config: &HttpConfig,
    cmd_tx: mpsc::Sender<String>,
    mixer_state: Arc<Mutex<MixerState>>,
    voice_manager: Arc<Mutex<VoiceManager>>,
    cache_manager: Arc<tokio::sync::Mutex<CacheManager>>,
) -> Result<SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
    let log_broadcaster = Arc::new(LogBroadcaster::new());

    let state = AppState {
        cmd_tx,
        mixer_state,
        voice_manager,
        cache_manager,
        auth_token: config.auth_token.clone(),
        require_auth: config.require_auth,
        log_broadcaster: log_broadcaster.clone(),
    };

    let app = create_router(state, config.cors_permissive, config.websocket_enabled);

    let addr: SocketAddr = format!("{}:{}", config.bind_address, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    if let Some(warning) = exposure_warning(&actual_addr, &config.auth_token, config.require_auth) {
        tracing::warn!("{}", warning);
    }
    tracing::info!("HTTP server listening on http://{}", actual_addr);

    // Spawn the server in the background
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("HTTP server error: {}", e);
        }
    });

    Ok(actual_addr)
}
