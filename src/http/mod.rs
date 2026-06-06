// ABOUTME: HTTP REST API server for mqttaudio.
// ABOUTME: Provides endpoints mirroring MQTT commands, status queries, and WebSocket for log streaming.

mod handlers;
mod routes;
mod websocket;

pub use routes::create_router;
pub use websocket::LogBroadcaster;

use crate::cache::CacheManager;
use crate::config::HttpConfig;
use crate::voice::VoiceManager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::sync::mpsc;

/// Per-sample status the control thread knows when a sample is started. Live
/// playback position is owned by the audio thread and is not reflected here.
#[derive(Clone, Default)]
pub struct SampleStatus {
    pub internal_id: u64,
    pub sample_id: Option<String>,
    pub voice_id: String,
    pub file_path: String,
    pub total_frames: usize,
    pub sample_rate: u32,
    pub volume: f32,
    pub voice_volume: f32,
    pub speed: f32,
    pub loop_mode: bool,
    /// Windowed/streamed (forward-only) sample (Sprint W6 F4). The transport UI
    /// gates seek/speed/reverse off this instead of inferring from total_frames.
    pub windowed: bool,
    /// Live-position publisher shared with the RT `ActiveSample` (Sprint W6, DW12).
    /// When telemetry is enabled the audio thread stores the current frame position
    /// here; the status handler reads it. `None` for streamed sources and when the
    /// sample was started without a publisher. The Arc is cloned into the status
    /// snapshot, so reading it never touches the mixer (D22a).
    pub position: Option<Arc<AtomicUsize>>,
}

/// Per-input status the control thread knows for a configured live input.
#[derive(Clone, Default)]
pub struct InputStatus {
    pub index: usize,
    pub voice_id: String,
    pub volume: f32,
    pub channels: usize,
}

/// Control-side view of what is playing, exposed to the HTTP status handlers.
/// The control thread rebuilds it as it sends commands; the audio thread never
/// touches it (D20). Live per-sample position is not available control-side.
#[derive(Clone, Default)]
pub struct StatusSnapshot {
    pub active_samples: usize,
    pub output_channels: usize,
    pub samples: Vec<SampleStatus>,
    pub inputs: Vec<InputStatus>,
}

/// Shared application state passed to all HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    /// Channel to send commands (same as MQTT uses)
    pub cmd_tx: mpsc::Sender<String>,
    /// Read-only control-side snapshot of what is playing, for status queries
    pub status: Arc<RwLock<StatusSnapshot>>,
    /// Read-only access to voice manager for status queries
    pub voice_manager: Arc<Mutex<VoiceManager>>,
    /// Read-only access to cache manager for status queries
    pub cache_manager: Arc<tokio::sync::Mutex<CacheManager>>,
    /// Count of output samples the limiter held at the ceiling, for `/status`
    /// and `/metrics`. Produced by the Sprint-6 limiter on the audio thread.
    pub clip_count: Arc<AtomicU64>,
    /// Count of cpal stream-error callbacks (dropouts/underruns that triggered a
    /// stream rebuild), produced by the Sprint-5 audio engine. Surfaced in
    /// `/metrics` and `/status`.
    pub xruns: Arc<AtomicU64>,
    /// When the daemon started, for the `/metrics` uptime field.
    pub start_time: Instant,
    /// Control-side per-voice ducking multiplier (resolved target, where < 1.0
    /// means the voice is ducked). The control thread owns ducking (D20) and
    /// updates this whenever a target changes; the HTTP handlers read it for
    /// `/metrics` and `/status/voices`. A voice at full volume is absent.
    pub ducking: Arc<RwLock<HashMap<String, f32>>>,
    /// Optional auth token for Bearer authentication
    pub auth_token: Option<String>,
    /// Opt-in: require a valid token on ALL routes (status + ws included).
    pub require_auth: bool,
    /// Log broadcaster for WebSocket clients
    pub log_broadcaster: Arc<LogBroadcaster>,
    /// Opt-in telemetry gate (Sprint W6, DW3). Off by default. Shared with the RT
    /// `MixerState`; `POST /telemetry` flips it. When set, the audio thread
    /// publishes live positions and the status handler reports them; when clear,
    /// `/status/samples` reports `0` as before and the RT does no new work.
    pub telemetry_enabled: Arc<AtomicBool>,
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
#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    config: &HttpConfig,
    cmd_tx: mpsc::Sender<String>,
    status: Arc<RwLock<StatusSnapshot>>,
    voice_manager: Arc<Mutex<VoiceManager>>,
    cache_manager: Arc<tokio::sync::Mutex<CacheManager>>,
    clip_count: Arc<AtomicU64>,
    xruns: Arc<AtomicU64>,
    start_time: Instant,
    ducking: Arc<RwLock<HashMap<String, f32>>>,
    telemetry_enabled: Arc<AtomicBool>,
) -> Result<SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
    let log_broadcaster = Arc::new(LogBroadcaster::new());

    let state = AppState {
        cmd_tx,
        status,
        voice_manager,
        cache_manager,
        clip_count,
        xruns,
        start_time,
        ducking,
        auth_token: config.auth_token.clone(),
        require_auth: config.require_auth,
        log_broadcaster: log_broadcaster.clone(),
        telemetry_enabled,
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
