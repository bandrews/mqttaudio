// ABOUTME: HTTP request handlers for mqttaudio REST API.
// ABOUTME: Each handler maps HTTP requests to internal commands or status queries.

use super::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Response type for command endpoints.
#[derive(Serialize)]
struct CommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl CommandResponse {
    fn ok() -> Self {
        Self {
            success: true,
            message: Some("Command accepted".to_string()),
            error: None,
        }
    }

    fn error(msg: &str) -> Self {
        Self {
            success: false,
            message: None,
            error: Some(msg.to_string()),
        }
    }
}

/// Send a command JSON to the command channel.
async fn send_command(state: &AppState, command_json: &str) -> Result<(), String> {
    state
        .cmd_tx
        .send(command_json.to_string())
        .await
        .map_err(|e| format!("Failed to send command: {}", e))
}

// =============================================================================
// Health Check
// =============================================================================

pub async fn handle_health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "mqttaudio",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// =============================================================================
// Version & Metrics
// =============================================================================

/// Build/version identity. `git_sha` is included only when the build injected
/// `MQTTAUDIO_GIT_SHA` (e.g. a CI/release build); it is omitted otherwise rather
/// than reported as a placeholder.
pub async fn handle_version() -> impl IntoResponse {
    let mut body = json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(sha) = option_env!("MQTTAUDIO_GIT_SHA") {
        body["git_sha"] = json!(sha);
    }
    Json(body)
}

/// Operational telemetry for monitoring. Every field is real: `uptime_seconds`
/// from the daemon's start instant, `clips` from the Sprint-6 limiter counter,
/// `xruns` from the Sprint-5 cpal stream-error counter, the activity counts from
/// the VoiceManager and the control-side status snapshot, and `ducking` the
/// control-side per-voice resolved target multiplier (a voice below 1.0 is being
/// ducked). No value is fabricated.
pub async fn handle_metrics(State(state): State<AppState>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;

    let uptime_seconds = state.start_time.elapsed().as_secs_f64();
    let clips = state.clip_count.load(Ordering::Relaxed);
    let xruns = state.xruns.load(Ordering::Relaxed);
    let voice_count = state.voice_manager.lock().voice_count();

    // Cache + memory-budget usage: resident decoded bytes, the headroom the auto-window
    // decision sees, and on-disk bytes. `null` headroom means an unlimited budget.
    let (cache_memory_bytes, cache_memory_entries, cache_headroom, cache_cap, cache_disk_bytes) = {
        let cache_mgr = state.cache_manager.lock().await;
        let mem = cache_mgr.memory_stats();
        let disk = cache_mgr.disk_stats();
        (
            mem.size_bytes,
            mem.entry_count,
            cache_mgr.memory_headroom(),
            cache_mgr.memory_cap(),
            disk.size_bytes,
        )
    };
    let headroom_json = if cache_headroom == usize::MAX {
        Value::Null
    } else {
        json!(cache_headroom)
    };
    // The resolved budget cap; null means an unlimited budget (no cap).
    let cap_json = cache_cap.map(|c| json!(c)).unwrap_or(Value::Null);

    let (active_samples, active_inputs, output_channels) = {
        let snapshot = state.status.read().unwrap();
        (
            snapshot.active_samples,
            snapshot.inputs.len(),
            snapshot.output_channels,
        )
    };

    let ducking: serde_json::Map<String, Value> = state
        .ducking
        .read()
        .unwrap()
        .iter()
        .map(|(voice, multiplier)| (voice.clone(), json!(multiplier)))
        .collect();

    // First-start play latency (Sprint 11, D50): enqueue-to-first-mix, published
    // by the audio thread and folded by the reaper. Zeros until a play is measured.
    let latency_last = state.latency.last_ns.load(Ordering::Relaxed);
    let latency_max = state.latency.max_ns.load(Ordering::Relaxed);
    let plays_measured = state.latency.plays_measured.load(Ordering::Relaxed);

    // Per-input capture-path counters (Sprint 13, D57): real relaxed-atomic
    // totals bumped by the capture callback, never placeholders.
    let input_capture: serde_json::Map<String, Value> = state
        .input_telemetry
        .iter()
        .map(|(voice, t)| {
            (
                voice.clone(),
                json!({
                    "resample_errors": t.resample_errors.load(Ordering::Relaxed),
                    "overflow_dropped_samples":
                        t.overflow_dropped_samples.load(Ordering::Relaxed),
                    "ratio_rejects": t.ratio_rejects.load(Ordering::Relaxed),
                    "scratch_regrows": t.scratch_regrows.load(Ordering::Relaxed),
                }),
            )
        })
        .collect();

    // Pitch-scratch regrows on the audio thread (Sprint 13, D58): zero unless a
    // device delivers blocks beyond the pre-size.
    let pitch_scratch_regrows = crate::audio::mixer::PITCH_SCRATCH_REGROWS.load(Ordering::Relaxed);

    Json(json!({
        "uptime_seconds": uptime_seconds,
        "clips": clips,
        "xruns": xruns,
        "active_voices": voice_count,
        "active_samples": active_samples,
        "active_inputs": active_inputs,
        "output_channels": output_channels,
        "cache": {
            "memory_bytes": cache_memory_bytes,
            "memory_entries": cache_memory_entries,
            "memory_headroom_bytes": headroom_json,
            "memory_cap_bytes": cap_json,
            "disk_bytes": cache_disk_bytes,
        },
        "ducking": ducking,
        "latency": {
            "play_to_first_mix_ns": {
                "last": latency_last,
                "max": latency_max,
            },
            "plays_measured": plays_measured,
        },
        "input_capture": input_capture,
        "pitch_scratch_regrows": pitch_scratch_regrows,
    }))
}

// =============================================================================
// Generic Command Endpoint
// =============================================================================

/// Handle any command by accepting raw JSON.
/// Accepts the same JSON format as MQTT messages. A body the `Json` extractor
/// rejects (not JSON at all) is answered with 400 and the daemon's own
/// `CommandResponse` shape (D61), so every `/command` error parses the same way
/// for clients — never axum's plaintext rejection.
pub async fn handle_command(
    State(state): State<AppState>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let Json(body) = match body {
        Ok(json) => json,
        Err(rejection) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(CommandResponse::error(&format!(
                    "Invalid JSON: {}",
                    rejection.body_text()
                ))),
            );
        }
    };

    // An already-parsed `Value` always re-serializes.
    let command_json = body.to_string();

    match send_command(&state, &command_json).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

// =============================================================================
// Individual Command Endpoints
// =============================================================================

#[derive(Deserialize)]
pub struct PlayParams {
    file: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    volume: Option<f32>,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    fade_in: Option<u32>,
    #[serde(default)]
    start_position_ms: Option<u64>,
    #[serde(default, alias = "loop")]
    loop_mode: Option<bool>,
    #[serde(default)]
    crossfade_ms: Option<u32>,
}

pub async fn handle_play(
    State(state): State<AppState>,
    Json(params): Json<PlayParams>,
) -> impl IntoResponse {
    let mut message = json!({ "file": params.file });
    if let Some(id) = params.id {
        message["id"] = json!(id);
    }
    if let Some(volume) = params.volume {
        message["volume"] = json!(volume);
    }
    if let Some(voice) = params.voice {
        message["voice"] = json!(voice);
    }
    if let Some(fade_in) = params.fade_in {
        message["fade_in"] = json!(fade_in);
    }
    if let Some(start_position_ms) = params.start_position_ms {
        message["start_position_ms"] = json!(start_position_ms);
    }
    if let Some(loop_mode) = params.loop_mode {
        message["loop"] = json!(loop_mode);
    }
    if let Some(crossfade_ms) = params.crossfade_ms {
        message["crossfade_ms"] = json!(crossfade_ms);
    }

    let command = json!({
        "command": "play",
        "message": message
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct StopParams {
    #[serde(default)]
    internal_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    fade_out_ms: Option<u32>,
}

pub async fn handle_stop(
    State(state): State<AppState>,
    Json(params): Json<StopParams>,
) -> impl IntoResponse {
    let mut message = json!({});
    if let Some(internal_id) = params.internal_id {
        message["internal_id"] = json!(internal_id);
    }
    if let Some(id) = params.id {
        message["id"] = json!(id);
    }
    if let Some(file) = params.file {
        message["file"] = json!(file);
    }
    if let Some(voice) = params.voice {
        message["voice"] = json!(voice);
    }
    if let Some(fade_out_ms) = params.fade_out_ms {
        message["fade_out_ms"] = json!(fade_out_ms);
    }

    let command = json!({
        "command": "stop",
        "message": message
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

pub async fn handle_stopall(State(state): State<AppState>) -> impl IntoResponse {
    let command = json!({
        "command": "stopall",
        "message": {}
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct VolumeParams {
    #[serde(default)]
    internal_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    volume: f32,
}

pub async fn handle_volume(
    State(state): State<AppState>,
    Json(params): Json<VolumeParams>,
) -> impl IntoResponse {
    let mut message = json!({ "volume": params.volume });
    if let Some(internal_id) = params.internal_id {
        message["internal_id"] = json!(internal_id);
    }
    if let Some(id) = params.id {
        message["id"] = json!(id);
    }
    if let Some(file) = params.file {
        message["file"] = json!(file);
    }
    if let Some(voice) = params.voice {
        message["voice"] = json!(voice);
    }

    let command = json!({
        "command": "volume",
        "message": message
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct SeekParams {
    #[serde(default)]
    internal_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    position_ms: u64,
}

pub async fn handle_seek(
    State(state): State<AppState>,
    Json(params): Json<SeekParams>,
) -> impl IntoResponse {
    let mut message = json!({ "position_ms": params.position_ms });
    if let Some(internal_id) = params.internal_id {
        message["internal_id"] = json!(internal_id);
    }
    if let Some(id) = params.id {
        message["id"] = json!(id);
    }
    if let Some(file) = params.file {
        message["file"] = json!(file);
    }
    if let Some(voice) = params.voice {
        message["voice"] = json!(voice);
    }

    let command = json!({
        "command": "seek",
        "message": message
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct SpeedParams {
    #[serde(default)]
    internal_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    speed: f32,
    #[serde(default)]
    pitch_correction: Option<bool>,
}

pub async fn handle_speed(
    State(state): State<AppState>,
    Json(params): Json<SpeedParams>,
) -> impl IntoResponse {
    let mut message = json!({ "speed": params.speed });
    if let Some(internal_id) = params.internal_id {
        message["internal_id"] = json!(internal_id);
    }
    if let Some(id) = params.id {
        message["id"] = json!(id);
    }
    if let Some(file) = params.file {
        message["file"] = json!(file);
    }
    if let Some(voice) = params.voice {
        message["voice"] = json!(voice);
    }
    if let Some(pitch_correction) = params.pitch_correction {
        message["pitch_correction"] = json!(pitch_correction);
    }

    let command = json!({
        "command": "speed",
        "message": message
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct PrecacheParams {
    file: String,
}

pub async fn handle_precache(
    State(state): State<AppState>,
    Json(params): Json<PrecacheParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "precache",
        "message": { "file": params.file }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

pub async fn handle_cache_clear(State(state): State<AppState>) -> impl IntoResponse {
    let command = json!({
        "command": "cache_clear",
        "message": {}
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct CacheInvalidateParams {
    file: String,
}

pub async fn handle_cache_invalidate(
    State(state): State<AppState>,
    Json(params): Json<CacheInvalidateParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "cache_invalidate",
        "message": { "file": params.file }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

pub async fn handle_cache_reload(
    State(state): State<AppState>,
    Json(params): Json<CacheInvalidateParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "cache_reload",
        "message": { "file": params.file }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct VoiceStopParams {
    voice: String,
}

pub async fn handle_voice_stop(
    State(state): State<AppState>,
    Json(params): Json<VoiceStopParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "voice_stop",
        "message": { "voice": params.voice }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct VoiceFadeOutParams {
    voice: String,
    time_ms: u32,
}

pub async fn handle_voice_fade_out(
    State(state): State<AppState>,
    Json(params): Json<VoiceFadeOutParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "voice_fade_out",
        "message": {
            "voice": params.voice,
            "time": params.time_ms
        }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct VoiceVolumeParams {
    voice: String,
    volume: f32,
}

pub async fn handle_voice_volume(
    State(state): State<AppState>,
    Json(params): Json<VoiceVolumeParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "voice_volume",
        "message": {
            "voice": params.voice,
            "volume": params.volume
        }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct InputVolumeParams {
    input: String,
    volume: f32,
}

pub async fn handle_input_volume(
    State(state): State<AppState>,
    Json(params): Json<InputVolumeParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "input_volume",
        "message": {
            "input": params.input,
            "volume": params.volume
        }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

#[derive(Deserialize)]
pub struct InputMuteParams {
    input: String,
    #[serde(default)]
    mute: bool,
}

pub async fn handle_input_mute(
    State(state): State<AppState>,
    Json(params): Json<InputMuteParams>,
) -> impl IntoResponse {
    let command = json!({
        "command": "input_mute",
        "message": {
            "input": params.input,
            "mute": params.mute
        }
    });

    match send_command(&state, &command.to_string()).await {
        Ok(()) => (StatusCode::OK, Json(CommandResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error(&e)),
        ),
    }
}

// =============================================================================
// Status Endpoints
// =============================================================================

pub async fn handle_status(State(state): State<AppState>) -> impl IntoResponse {
    // Lock the async cache mutex first (and release it) so the sync parking_lot
    // guards below are never held across an await.
    let (mem_stats, disk_stats) = {
        let cache_mgr = state.cache_manager.lock().await;
        (cache_mgr.memory_stats(), cache_mgr.disk_stats())
    };

    let (sample_count, input_count, output_channels) = {
        let snapshot = state.status.read().unwrap();
        (
            snapshot.active_samples,
            snapshot.inputs.len(),
            snapshot.output_channels,
        )
    };
    let voice_count = state.voice_manager.lock().voice_count();
    let clip_count = state.clip_count.load(std::sync::atomic::Ordering::Relaxed);
    let xruns = state.xruns.load(std::sync::atomic::Ordering::Relaxed);

    Json(json!({
        "status": "running",
        "version": env!("CARGO_PKG_VERSION"),
        "active_samples": sample_count,
        "active_inputs": input_count,
        "active_voices": voice_count,
        "output_channels": output_channels,
        "clip_count": clip_count,
        "xruns": xruns,
        "cache": {
            "memory": {
                "entries": mem_stats.entry_count,
                "size_bytes": mem_stats.size_bytes
            },
            "disk": {
                "entries": disk_stats.entry_count,
                "size_bytes": disk_stats.size_bytes
            }
        }
    }))
}

pub async fn handle_samples(State(state): State<AppState>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;

    // Live position is published by the audio thread into each sample's atomic only
    // when telemetry is opted in (Sprint W6, DW3). When off, position-derived fields
    // are reported as 0 exactly as before — and the RT thread does no new work.
    let telemetry = state.telemetry_enabled.load(Ordering::Relaxed);
    let snapshot = state.status.read().unwrap();

    let samples: Vec<Value> = snapshot
        .samples
        .iter()
        .map(|s| {
            let total_ms = if s.sample_rate > 0 {
                (s.total_frames as u64 * 1000) / s.sample_rate as u64
            } else {
                0
            };
            let position = if telemetry {
                s.position.as_ref().map_or(0, |p| p.load(Ordering::Relaxed))
            } else {
                0
            };
            let position_ms = if s.sample_rate > 0 {
                (position as u64 * 1000) / s.sample_rate as u64
            } else {
                0
            };
            let progress_percent = if s.total_frames > 0 {
                (position as f64 / s.total_frames as f64 * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            json!({
                "internal_id": s.internal_id.to_string(),
                "id": s.sample_id,
                "voice": s.voice_id,
                "file": s.file_path,
                "position": position,
                "position_ms": position_ms,
                "total_frames": s.total_frames,
                "total_ms": total_ms,
                "sample_rate": s.sample_rate,
                "volume": s.volume,
                "voice_volume": s.voice_volume,
                "speed": s.speed,
                "loop_mode": s.loop_mode,
                "windowed": s.windowed,
                "progress_percent": progress_percent
            })
        })
        .collect();

    Json(json!({ "samples": samples }))
}

pub async fn handle_voices(State(state): State<AppState>) -> impl IntoResponse {
    let voices = state.voice_manager.lock().list_voices();
    let ducking = state.ducking.read().unwrap();

    // Enrich each voice with its resolved ducking multiplier (D40). A voice the
    // control thread has not ducked is at the resting full-volume multiplier 1.0.
    let voices: Vec<Value> = voices
        .into_iter()
        .map(|mut voice| {
            let multiplier = voice
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| ducking.get(id).copied())
                .unwrap_or(1.0);
            if let Some(obj) = voice.as_object_mut() {
                obj.insert("ducking_multiplier".to_string(), json!(multiplier));
            }
            voice
        })
        .collect();

    Json(json!({ "voices": voices }))
}

pub async fn handle_cache_status(State(state): State<AppState>) -> impl IntoResponse {
    let cache_mgr = state.cache_manager.lock().await;
    let mem_stats = cache_mgr.memory_stats();
    let disk_stats = cache_mgr.disk_stats();

    Json(json!({
        "memory": {
            "entries": mem_stats.entry_count,
            "size_bytes": mem_stats.size_bytes,
            "size_mb": mem_stats.size_bytes as f64 / (1024.0 * 1024.0)
        },
        "disk": {
            "entries": disk_stats.entry_count,
            "size_bytes": disk_stats.size_bytes,
            "size_mb": disk_stats.size_bytes as f64 / (1024.0 * 1024.0)
        }
    }))
}

pub async fn handle_inputs(State(state): State<AppState>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;
    let snapshot = state.status.read().unwrap();

    let inputs: Vec<Value> = snapshot
        .inputs
        .iter()
        .map(|input| {
            json!({
                "index": input.index,
                "voice_id": input.voice_id,
                "volume": input.applied_volume.as_ref()
                    .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
                    .unwrap_or(input.volume),
                "channels": input.channels,
                "muted": input.applied_muted.as_ref()
                    .map(|value| value.load(Ordering::Relaxed))
                    .unwrap_or(input.muted),
                "unmuted_volume": input.applied_unmuted_volume.as_ref()
                    .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
                    .unwrap_or(input.unmuted_volume),
                "ready": input.ready,
                "last_error": input.last_error
            })
        })
        .collect();

    Json(json!({ "inputs": inputs }))
}

// =============================================================================
// Telemetry opt-in (Sprint W6, DW3)
// =============================================================================

#[derive(Deserialize)]
pub struct TelemetryParams {
    enabled: bool,
}

/// Per-output-channel peak meters (Sprint W7), a poll fallback for the `/ws/state`
/// tick channel. Linear amplitudes; all zero when telemetry is off.
pub async fn handle_meters(State(state): State<AppState>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;
    let telemetry = state.telemetry_enabled.load(Ordering::Relaxed);
    let output: Vec<f32> = if telemetry {
        state
            .output_meters
            .iter()
            .map(|m| f32::from_bits(m.load(Ordering::Relaxed)))
            .collect()
    } else {
        vec![0.0; state.output_meters.len()]
    };
    Json(json!({ "output": output }))
}

/// Read-only running config (Sprint W8, DW11), secrets redacted. Config is read
/// once at startup, so this is a startup snapshot. The web app shows current
/// values and emits restart-required config snippets (config has no hot-reload).
pub async fn handle_config(State(state): State<AppState>) -> impl IntoResponse {
    Json((*state.config_json).clone())
}

/// Current telemetry-enable state.
pub async fn handle_telemetry_get(State(state): State<AppState>) -> impl IntoResponse {
    let enabled = state
        .telemetry_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    Json(json!({ "enabled": enabled }))
}

/// Opt in/out of live telemetry. OFF by default; when on, the audio thread
/// publishes live sample positions (and, from Sprint W7, meters + state events).
/// Off by default keeps the RT path free until a client is watching.
pub async fn handle_telemetry_set(
    State(state): State<AppState>,
    Json(params): Json<TelemetryParams>,
) -> impl IntoResponse {
    state
        .telemetry_enabled
        .store(params.enabled, std::sync::atomic::Ordering::Relaxed);
    Json(json!({ "enabled": params.enabled }))
}
