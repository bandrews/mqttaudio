// ABOUTME: HTTP request handlers for mqttaudio REST API.
// ABOUTME: Each handler maps HTTP requests to internal commands or status queries.

use super::AppState;
use crate::mqtt::commands::{CommandErrorKind, CommandRequest};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
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
    fn success(message: String) -> Self {
        Self {
            success: true,
            message: Some(message),
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

fn status_for(kind: CommandErrorKind) -> StatusCode {
    match kind {
        CommandErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
        CommandErrorKind::NotFound => StatusCode::NOT_FOUND,
        CommandErrorKind::Forbidden => StatusCode::FORBIDDEN,
        CommandErrorKind::Cancelled => StatusCode::CONFLICT,
        CommandErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Send a command to the processing loop and wait for its actual outcome,
/// so HTTP clients learn whether the command worked rather than a blanket
/// "accepted". The wait is bounded; a load that takes absurdly long reports
/// a timeout instead of hanging the request.
async fn send_command(state: &AppState, command_json: &str) -> (StatusCode, Json<CommandResponse>) {
    let (request, reply_rx) = CommandRequest::with_reply(command_json.to_string());
    if state.cmd_tx.send(request).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error("Command channel closed")),
        );
    }

    match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
        Ok(Ok(Ok(message))) => (StatusCode::OK, Json(CommandResponse::success(message))),
        Ok(Ok(Err(err))) => (status_for(err.kind), Json(CommandResponse::error(&err.message))),
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(CommandResponse::error("Command was dropped before completion")),
        ),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(CommandResponse::error("Timed out waiting for the command result")),
        ),
    }
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
// Generic Command Endpoint
// =============================================================================

/// Handle any command by accepting raw JSON.
/// Accepts the same JSON format as MQTT messages.
pub async fn handle_command(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let command_json = match serde_json::to_string(&body) {
        Ok(json) => json,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(CommandResponse::error(&format!("Invalid JSON: {}", e))),
            );
        }
    };

    send_command(&state, &command_json).await
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
    /// Channel routing, same shape as the MQTT command:
    /// [{"src": 0, "dest": "rear_left"}, ...]
    #[serde(default)]
    channel_map: Option<Value>,
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
    if let Some(channel_map) = params.channel_map {
        message["channel_map"] = channel_map;
    }

    let command = json!({
        "command": "play",
        "message": message
    });

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
}

pub async fn handle_stopall(State(state): State<AppState>) -> impl IntoResponse {
    let command = json!({
        "command": "stopall",
        "message": {}
    });

    send_command(&state, &command.to_string()).await
}

#[derive(Deserialize, Default)]
pub struct FadeAllParams {
    /// Fade duration in milliseconds. Omitted means the daemon's default.
    #[serde(default)]
    time: Option<u32>,
}

pub async fn handle_fadeall(
    State(state): State<AppState>,
    Json(params): Json<FadeAllParams>,
) -> impl IntoResponse {
    let mut message = json!({});
    if let Some(time) = params.time {
        message["time"] = json!(time);
    }

    let command = json!({
        "command": "fadeall",
        "message": message
    });

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
}

pub async fn handle_cache_clear(State(state): State<AppState>) -> impl IntoResponse {
    let command = json!({
        "command": "cache_clear",
        "message": {}
    });

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
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

    send_command(&state, &command.to_string()).await
}

#[derive(Deserialize)]
pub struct InputMuteParams {
    input: String,
    /// Required, matching the MQTT command: an omitted field must error
    /// rather than silently unmute
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

    send_command(&state, &command.to_string()).await
}

// =============================================================================
// Status Endpoints
// =============================================================================

pub async fn handle_status(State(state): State<AppState>) -> impl IntoResponse {
    // Take the async cache lock first and release it before touching the
    // std mutexes: a status poll must never pin the mixer lock while
    // waiting on cache work (that would starve the audio callback)
    let (mem_stats, disk_stats) = {
        let cache_mgr = state.cache_manager.lock().await;
        (cache_mgr.memory_stats(), cache_mgr.disk_stats())
    };

    let mixer = state.mixer_state.lock().unwrap();
    let voice_mgr = state.voice_manager.lock().unwrap();

    let sample_count = mixer.active_samples.len();
    let input_count = mixer.live_inputs.len();
    let voice_count = voice_mgr.voice_count();

    Json(json!({
        "status": "running",
        "version": env!("CARGO_PKG_VERSION"),
        "active_samples": sample_count,
        "active_inputs": input_count,
        "active_voices": voice_count,
        "output_channels": mixer.output_channels,
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
    let mixer = state.mixer_state.lock().unwrap();

    let samples: Vec<Value> = mixer
        .active_samples
        .iter()
        .map(|s| {
            let sample_rate = s.buffer.sample_rate();
            let position_ms = if sample_rate > 0 {
                (s.position as u64 * 1000) / sample_rate as u64
            } else {
                0
            };
            let total_ms = if sample_rate > 0 {
                (s.buffer.frames() as u64 * 1000) / sample_rate as u64
            } else {
                0
            };
            json!({
                "internal_id": s.id.to_string(),
                "id": s.sample_id,
                "voice": s.voice_id,
                "file": s.file_path,
                "position": s.position,
                "position_ms": position_ms,
                "total_frames": s.buffer.frames(),
                "total_ms": total_ms,
                "sample_rate": sample_rate,
                "volume": s.volume,
                "voice_volume": s.voice_volume,
                "speed": s.speed,
                "loop_mode": s.loop_mode,
                "progress_percent": if s.buffer.frames() > 0 {
                    (s.position as f64 / s.buffer.frames() as f64 * 100.0).round()
                } else {
                    0.0
                }
            })
        })
        .collect();

    Json(json!({ "samples": samples }))
}

pub async fn handle_voices(State(state): State<AppState>) -> impl IntoResponse {
    let voice_mgr = state.voice_manager.lock().unwrap();
    let voices = voice_mgr.list_voices();

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
    let mixer = state.mixer_state.lock().unwrap();

    let inputs: Vec<Value> = mixer
        .live_inputs
        .iter()
        .enumerate()
        .map(|(idx, input)| {
            json!({
                "index": idx,
                "voice_id": input.voice_id,
                "volume": input.volume,
                "channels": input.input_channels,
                "muted": input.volume == 0.0,
                "backlog_frames": input.backlog_frames(),
                "max_backlog_frames": input.max_backlog_frames,
                "dropped_frames": input.dropped_frames(),
                "trimmed_frames": input.trimmed_frames,
                "underrun_frames": input.underrun_frames
            })
        })
        .collect();

    Json(json!({ "inputs": inputs }))
}
