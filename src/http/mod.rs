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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};
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
    /// Decoded channel count — the Speed dispatcher sizes a shipped pitch
    /// corrector from it (D56).
    pub channels: usize,
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

/// Control-side aggregate of first-start play latency (Sprint 11, D50): the time
/// from a play command's enqueue onto the audio ring to the first block in which
/// the sample mixed loaded audio, in nanoseconds. The audio thread stores each
/// play's value into its own pre-allocated probe atomic; the control-side reaper
/// folds fired probes into these fields; `/metrics` reads them. Zero values mean
/// no play has been measured yet.
#[derive(Default)]
pub struct PlayLatencyStats {
    /// The most recently measured play's enqueue-to-first-mix latency (ns).
    pub last_ns: AtomicU64,
    /// The largest latency measured since startup (ns).
    pub max_ns: AtomicU64,
    /// How many plays have been measured.
    pub plays_measured: AtomicU64,
}

/// Registers per-play first-mix probes and folds fired ones into a
/// [`PlayLatencyStats`] aggregate (Sprint 11, D50). The play path registers a
/// probe and attaches it to the outgoing sample (`set_latency_probe`); the audio
/// thread stores the measured latency into the probe on the sample's first mixed
/// block; the control-side reaper tick calls [`fold_fired`](Self::fold_fired).
pub struct LatencyTracker {
    pending: Mutex<Vec<Arc<AtomicU64>>>,
    stats: Arc<PlayLatencyStats>,
}

impl LatencyTracker {
    pub fn new(stats: Arc<PlayLatencyStats>) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            stats,
        }
    }

    /// Create and register a probe for one play. The caller attaches the clone to
    /// the sample before enqueueing it; the tracker keeps the other reference for
    /// folding.
    pub fn new_probe(&self) -> Arc<AtomicU64> {
        let probe = Arc::new(AtomicU64::new(0));
        self.pending.lock().push(probe.clone());
        probe
    }

    /// Fold every fired probe (non-zero value) into the aggregate and drop it.
    /// An unfired probe whose sample is gone (the tracker holds the only
    /// reference left) is dropped without folding — the play never reached its
    /// first mix (stopped early or failed). Called off-RT by the reaper tick.
    pub fn fold_fired(&self) {
        use std::sync::atomic::Ordering;
        let mut pending = self.pending.lock();
        pending.retain(|probe| {
            let v = probe.load(Ordering::Relaxed);
            if v == 0 {
                return Arc::strong_count(probe) > 1;
            }
            self.stats.last_ns.store(v, Ordering::Relaxed);
            self.stats.max_ns.fetch_max(v, Ordering::Relaxed);
            self.stats.plays_measured.fetch_add(1, Ordering::Relaxed);
            false
        });
    }
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
    /// Per-output-channel peak meters (Sprint W7), shared with the RT `MixerState`.
    /// Read by the state-event tick timer (as f32 bits).
    pub output_meters: Arc<Vec<AtomicU32>>,
    /// Broadcaster for the `/ws/state` typed state-event channel (Sprint W7). The
    /// tick timer publishes here only when telemetry is on AND ≥1 client is
    /// subscribed (DW3).
    pub state_broadcaster: Arc<LogBroadcaster>,
    /// The running config as redacted JSON (Sprint W8, DW11), for read-only
    /// `GET /config`. Config is read once at startup (DW8), so this is a startup
    /// snapshot; secrets (auth_token, mqtt password) are nulled out.
    pub config_json: Arc<serde_json::Value>,
    /// First-start play latency aggregate (Sprint 11, D50), for `/metrics`.
    pub latency: Arc<PlayLatencyStats>,
    /// Per-input capture-path counters (Sprint 13, D57), for `/metrics`:
    /// (voice id, counters) per configured live input.
    pub input_telemetry: Arc<Vec<(String, Arc<crate::audio::input::InputTelemetry>)>>,
}

/// Redact secrets from a serialized config for `GET /config` (DW11):
/// `http.auth_token` and `mqtt.password` become `null`. Pure, for the startup
/// snapshot and tests.
pub fn redact_config_json(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(http) = value.get_mut("http").and_then(|h| h.as_object_mut()) {
        if http.contains_key("auth_token") {
            http.insert("auth_token".to_string(), serde_json::Value::Null);
        }
    }
    if let Some(mqtt) = value.get_mut("mqtt").and_then(|m| m.as_object_mut()) {
        if mqtt.contains_key("password") {
            mqtt.insert("password".to_string(), serde_json::Value::Null);
        }
    }
    value
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
    output_meters: Arc<Vec<AtomicU32>>,
    config_json: Arc<serde_json::Value>,
    latency: Arc<PlayLatencyStats>,
    input_telemetry: Arc<Vec<(String, Arc<crate::audio::input::InputTelemetry>)>>,
) -> Result<SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
    let log_broadcaster = Arc::new(LogBroadcaster::new());
    let state_broadcaster = Arc::new(LogBroadcaster::new());

    // Clones for the state-event tick timer (Sprint W7), taken before `status` etc.
    // are moved into AppState.
    let tick_telemetry = telemetry_enabled.clone();
    let tick_status = status.clone();
    let tick_meters = output_meters.clone();
    let tick_broadcaster = state_broadcaster.clone();

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
        output_meters,
        state_broadcaster: state_broadcaster.clone(),
        config_json,
        latency,
        input_telemetry,
    };

    // State-event tick timer (~15 Hz, DW12): only does work when telemetry is on AND
    // a client is subscribed (DW3). Reads the RT-published position + meter atomics
    // (never the mixer, D22a) and broadcasts a compact tick frame.
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(66));
        loop {
            ticker.tick().await;
            if !tick_telemetry.load(Ordering::Relaxed) || tick_broadcaster.receiver_count() == 0 {
                continue;
            }
            let samples: Vec<serde_json::Value> = {
                let snap = tick_status.read().unwrap();
                snap.samples
                    .iter()
                    .map(|s| {
                        let position = s.position.as_ref().map_or(0, |p| p.load(Ordering::Relaxed));
                        let position_ms = if s.sample_rate > 0 {
                            (position as u64 * 1000) / s.sample_rate as u64
                        } else {
                            0
                        };
                        let progress = if s.total_frames > 0 {
                            (position as f64 / s.total_frames as f64 * 100.0).min(100.0)
                        } else {
                            0.0
                        };
                        serde_json::json!({
                            "internal_id": s.internal_id.to_string(),
                            "position_ms": position_ms,
                            "progress_percent": progress,
                        })
                    })
                    .collect()
            };
            let output: Vec<f32> = tick_meters
                .iter()
                .map(|m| f32::from_bits(m.load(Ordering::Relaxed)))
                .collect();
            let frame = serde_json::json!({
                "type": "tick",
                "samples": samples,
                "meters": { "output": output },
            });
            tick_broadcaster.broadcast(frame.to_string());
        }
    });

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
