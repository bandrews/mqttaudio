// ABOUTME: Integration tests for the HTTP REST API.
// ABOUTME: Tests endpoints, authentication, and command routing.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mqttaudio::cache::CacheManager;
use mqttaudio::http::{
    create_router, AppState, InputStatus, LogBroadcaster, SampleStatus, StatusSnapshot,
};
use mqttaudio::voice::VoiceManager;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tower::util::ServiceExt;

/// Build a control-side `SampleStatus` with the static metadata the snapshot
/// carries. Live position is audio-thread-owned and intentionally absent.
fn sample_status(
    internal_id: u64,
    voice: &str,
    file: &str,
    total_frames: usize,
    sample_rate: u32,
) -> SampleStatus {
    SampleStatus {
        internal_id,
        voice_id: voice.to_string(),
        file_path: file.to_string(),
        total_frames,
        sample_rate,
        ..Default::default()
    }
}

/// Create a test AppState with mock components.
fn create_test_state() -> (AppState, mpsc::Receiver<String>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>(100);

    let status = Arc::new(RwLock::new(StatusSnapshot {
        active_samples: 0,
        output_channels: 2,
        samples: Vec::new(),
        inputs: Vec::new(),
    }));

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));
    let cache_manager = Arc::new(tokio::sync::Mutex::new(
        CacheManager::new(std::env::temp_dir().join("mqttaudio_test_cache")).unwrap(),
    ));

    let state = AppState {
        cmd_tx,
        status,
        voice_manager,
        cache_manager,
        clip_count: Arc::new(AtomicU64::new(0)),
        xruns: Arc::new(AtomicU64::new(0)),
        start_time: std::time::Instant::now(),
        ducking: Arc::new(RwLock::new(std::collections::HashMap::new())),
        auth_token: None,
        require_auth: false,
        log_broadcaster: Arc::new(LogBroadcaster::new()),
        telemetry_enabled: Arc::new(AtomicBool::new(false)),
        output_meters: Arc::new(Vec::new()),
        state_broadcaster: Arc::new(LogBroadcaster::new()),
        config_json: Arc::new(serde_json::json!({})),
        latency: Arc::new(mqttaudio::http::PlayLatencyStats::default()),
    };

    (state, cmd_rx)
}

/// Create a test AppState with a token AND require_auth enabled (all routes gated).
fn create_test_state_require_auth(token: &str) -> (AppState, mpsc::Receiver<String>) {
    let (mut state, cmd_rx) = create_test_state();
    state.auth_token = Some(token.to_string());
    state.require_auth = true;
    (state, cmd_rx)
}

/// Create a test AppState with authentication enabled.
fn create_test_state_with_auth(token: &str) -> (AppState, mpsc::Receiver<String>) {
    let (mut state, cmd_rx) = create_test_state();
    state.auth_token = Some(token.to_string());
    (state, cmd_rx)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "mqttaudio");
}

#[tokio::test]
async fn test_status_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "running");
    assert_eq!(json["active_samples"], 0);
    assert_eq!(json["output_channels"], 2);
    // The limiter clip/over counter is surfaced (F2); fresh state reports 0.
    assert_eq!(json["clip_count"], 0);
    // The cpal stream-error (dropout/xrun) counter is surfaced on /status too,
    // closing the Sprint-5 residual; fresh state reports 0.
    assert_eq!(json["xruns"], 0);
}

#[tokio::test]
async fn test_status_surfaces_the_real_xrun_counter() {
    // /status reads the SAME xruns Arc<AtomicU64> the cpal error callback
    // increments (Sprint 5). Bumping it must show through verbatim.
    let (state, _rx) = create_test_state();
    state.xruns.store(4, std::sync::atomic::Ordering::Relaxed);
    let json = get_json(state, "/status").await;
    assert_eq!(
        json["xruns"], 4,
        "status must reflect the shared xruns counter"
    );
}

#[tokio::test]
async fn test_command_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let command = serde_json::json!({
        "command": "stopall",
        "message": {}
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/command")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(command.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify the command was sent to the channel
    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "stopall");
}

#[tokio::test]
async fn test_play_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let play_params = serde_json::json!({
        "file": "/path/to/test.wav",
        "volume": 0.8,
        "voice": "test_voice"
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/play")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(play_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify the command was sent to the channel
    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "play");
    assert_eq!(parsed["message"]["file"], "/path/to/test.wav");
    // Use approximate comparison for floats
    let volume = parsed["message"]["volume"].as_f64().unwrap();
    assert!((volume - 0.8).abs() < 0.001);
    assert_eq!(parsed["message"]["voice"], "test_voice");
}

#[tokio::test]
async fn test_stopall_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/stopall")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify the command was sent
    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "stopall");
}

#[tokio::test]
async fn test_auth_required_without_token() {
    let (state, _rx) = create_test_state_with_auth("secret_token_123");
    let app = create_router(state, false, false);

    // Try to access a command endpoint without auth
    let request = Request::builder()
        .method(Method::POST)
        .uri("/stopall")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should be unauthorized
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_with_bearer_token() {
    let (state, mut rx) = create_test_state_with_auth("secret_token_123");
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/stopall")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer secret_token_123")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify command was sent
    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "stopall");
}

#[tokio::test]
async fn test_auth_with_query_param() {
    let (state, mut rx) = create_test_state_with_auth("secret_token_123");
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/stopall?token=secret_token_123")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify command was sent
    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "stopall");
}

#[tokio::test]
async fn test_status_endpoints_no_auth_required() {
    let (state, _rx) = create_test_state_with_auth("secret_token_123");
    let app = create_router(state, false, false);

    // Status endpoints should be accessible without auth
    let request = Request::builder()
        .method(Method::GET)
        .uri("/status")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_samples_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/samples")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["samples"].is_array());
}

#[tokio::test]
async fn test_voices_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/voices")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["voices"].is_array());
}

#[tokio::test]
async fn test_cache_status_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/cache")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["memory"].is_object());
    assert!(json["disk"].is_object());
}

#[tokio::test]
async fn test_metrics_endpoint_includes_cache_and_budget() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Core telemetry stays present.
    assert!(json["uptime_seconds"].is_number());
    assert!(json["xruns"].is_number());

    // Cache + memory-budget usage (the never-OOM observability).
    let cache = &json["cache"];
    assert!(cache.is_object(), "metrics must include a cache section");
    assert!(cache["memory_bytes"].is_number());
    assert!(cache["memory_entries"].is_number());
    assert!(cache["disk_bytes"].is_number());
    // The test cache has an unlimited budget (CacheManager::new), so both the cap and
    // the headroom under it are null.
    assert!(cache["memory_headroom_bytes"].is_null());
    assert!(
        cache["memory_cap_bytes"].is_null(),
        "unlimited budget must report a null cap"
    );
}

#[tokio::test]
async fn test_inputs_endpoint() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/inputs")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["inputs"].is_array());
}

#[tokio::test]
async fn test_stop_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let stop_params = serde_json::json!({
        "voice": "test_voice",
        "fade_out_ms": 500
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/stop")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(stop_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "stop");
    assert_eq!(parsed["message"]["voice"], "test_voice");
    assert_eq!(parsed["message"]["fade_out_ms"], 500);
}

#[tokio::test]
async fn test_volume_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let volume_params = serde_json::json!({
        "voice": "music",
        "volume": 0.5
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/volume")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(volume_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "volume");
    assert_eq!(parsed["message"]["voice"], "music");
    let volume = parsed["message"]["volume"].as_f64().unwrap();
    assert!((volume - 0.5).abs() < 0.001);
}

#[tokio::test]
async fn test_seek_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let seek_params = serde_json::json!({
        "id": "sample123",
        "position_ms": 5000
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/seek")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(seek_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "seek");
    assert_eq!(parsed["message"]["id"], "sample123");
    assert_eq!(parsed["message"]["position_ms"], 5000);
}

#[tokio::test]
async fn test_speed_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let speed_params = serde_json::json!({
        "voice": "narration",
        "speed": 1.5,
        "pitch_correction": true
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/speed")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(speed_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "speed");
    assert_eq!(parsed["message"]["voice"], "narration");
    let speed = parsed["message"]["speed"].as_f64().unwrap();
    assert!((speed - 1.5).abs() < 0.001);
    assert_eq!(parsed["message"]["pitch_correction"], true);
}

#[tokio::test]
async fn test_precache_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let precache_params = serde_json::json!({
        "file": "/sounds/startup.wav"
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/precache")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(precache_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "precache");
    assert_eq!(parsed["message"]["file"], "/sounds/startup.wav");
}

#[tokio::test]
async fn test_cache_clear_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/cache/clear")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "cache_clear");
}

#[tokio::test]
async fn test_voice_volume_endpoint() {
    let (state, mut rx) = create_test_state();
    let app = create_router(state, false, false);

    let voice_volume_params = serde_json::json!({
        "voice": "ambient",
        "volume": 0.3
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/voice/volume")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(voice_volume_params.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let received = rx.try_recv().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(parsed["command"], "voice_volume");
    assert_eq!(parsed["message"]["voice"], "ambient");
    let volume = parsed["message"]["volume"].as_f64().unwrap();
    assert!((volume - 0.3).abs() < 0.001);
}

#[tokio::test]
async fn test_samples_endpoint_reports_timing_metadata() {
    let (state, _rx) = create_test_state();

    // The control thread records each started sample's static metadata in the
    // status snapshot. Live playback position is owned by the audio thread and
    // is not in the snapshot (D20/D22a: the control plane never reads MixerState),
    // so the position-derived fields are reported as 0 while total_ms / sample_rate
    // remain truthful from the metadata.
    {
        let mut snapshot = state.status.write().unwrap();
        snapshot.active_samples = 1;
        snapshot.samples.push(sample_status(
            1,
            "test_voice",
            "/test/audio.wav",
            96000, // 2 seconds of audio at 48 kHz
            48000,
        ));
    }

    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/samples")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify samples array has one element
    let samples = json["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 1);

    let sample_json = &samples[0];

    // total_ms is derived from the snapshot's total_frames + sample_rate.
    assert!(
        sample_json.get("total_ms").is_some(),
        "total_ms field should be present"
    );
    let total_ms = sample_json["total_ms"].as_u64().unwrap();
    assert_eq!(
        total_ms, 2000,
        "total_ms should be 2000 for 2 seconds of audio"
    );

    assert!(
        sample_json.get("sample_rate").is_some(),
        "sample_rate field should be present"
    );
    let returned_sample_rate = sample_json["sample_rate"].as_u64().unwrap();
    assert_eq!(returned_sample_rate, 48000, "sample_rate should be 48000");

    // Live position is audio-thread-owned and not in the control-side snapshot,
    // so these fields are reported as 0 (documented behavior under D20/D22a).
    let position = sample_json["position"].as_u64().unwrap();
    assert_eq!(position, 0, "position is not tracked control-side");
    let position_ms = sample_json["position_ms"].as_u64().unwrap();
    assert_eq!(position_ms, 0, "position_ms is not tracked control-side");
}

// Sprint W6: live position is gated by the opt-in telemetry flag (DW3). The audio
// thread publishes into a per-sample atomic; the handler reports it only when
// telemetry is on. With telemetry off the fields stay 0 (the pre-W6 behavior).
async fn get_samples_json(app: axum::Router) -> serde_json::Value {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/samples")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_samples_position_is_telemetry_gated() {
    let (state, _rx) = create_test_state();
    let flag = state.telemetry_enabled.clone();

    // A full-load sample with a live-position publisher at 1.0s in (48000 frames of
    // a 10s / 480000-frame buffer), and a streamed source (windowed, no publisher).
    let pos = Arc::new(AtomicUsize::new(48_000));
    {
        let mut snapshot = state.status.write().unwrap();
        let mut full = sample_status(1, "music", "/a.wav", 480_000, 48_000);
        full.position = Some(pos.clone());
        full.windowed = false;
        snapshot.samples.push(full);
        let mut streamed = sample_status(2, "stream", "/b.wav", 0, 48_000);
        streamed.windowed = true;
        streamed.position = None;
        snapshot.samples.push(streamed);
    }

    let app = create_router(state, false, false);

    // Telemetry OFF (default): position is 0 even though the atomic holds 48000.
    flag.store(false, Ordering::Relaxed);
    let json = get_samples_json(app.clone()).await;
    let samples = json["samples"].as_array().unwrap();
    assert_eq!(samples[0]["position"].as_u64().unwrap(), 0);
    assert_eq!(samples[0]["position_ms"].as_u64().unwrap(), 0);
    assert_eq!(samples[0]["progress_percent"].as_f64().unwrap(), 0.0);
    assert_eq!(samples[0]["windowed"], serde_json::json!(false));
    assert_eq!(samples[1]["windowed"], serde_json::json!(true));

    // Telemetry ON: the live position is reported (48000 frames = 1000ms = 10%).
    flag.store(true, Ordering::Relaxed);
    let json = get_samples_json(app.clone()).await;
    let samples = json["samples"].as_array().unwrap();
    assert_eq!(samples[0]["position"].as_u64().unwrap(), 48_000);
    assert_eq!(samples[0]["position_ms"].as_u64().unwrap(), 1000);
    assert_eq!(samples[0]["progress_percent"].as_f64().unwrap(), 10.0);
    // The streamed sample has no publisher, so it stays at 0 even with telemetry on.
    assert_eq!(samples[1]["position"].as_u64().unwrap(), 0);

    // The audio thread advancing the atomic is reflected on the next read.
    pos.store(96_000, Ordering::Relaxed);
    let json = get_samples_json(app).await;
    assert_eq!(json["samples"][0]["position_ms"].as_u64().unwrap(), 2000);
}

#[tokio::test]
async fn test_telemetry_toggle_route() {
    let (state, _rx) = create_test_state();
    let flag = state.telemetry_enabled.clone();
    let app = create_router(state, false, false);

    // GET reports the default (off).
    let get = Request::builder()
        .method(Method::GET)
        .uri("/telemetry")
        .body(Body::empty())
        .unwrap();
    let body = axum::body::to_bytes(
        app.clone().oneshot(get).await.unwrap().into_body(),
        usize::MAX,
    )
    .await
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], serde_json::json!(false));

    // POST enables it; the shared flag flips.
    let post = Request::builder()
        .method(Method::POST)
        .uri("/telemetry")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    let response = app.oneshot(post).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        flag.load(Ordering::Relaxed),
        "POST /telemetry should set the flag"
    );
}

#[test]
fn redact_config_json_nulls_secrets_and_keeps_the_rest() {
    let input = serde_json::json!({
        "http": { "auth_token": "supersecret", "port": 8080 },
        "mqtt": { "password": "pw", "server": "localhost" },
        "audio": { "sample_rate": 48000 },
    });
    let out = mqttaudio::http::redact_config_json(input);
    assert!(
        out["http"]["auth_token"].is_null(),
        "auth_token must be redacted"
    );
    assert!(
        out["mqtt"]["password"].is_null(),
        "mqtt password must be redacted"
    );
    // Non-secret fields are preserved verbatim.
    assert_eq!(out["http"]["port"], 8080);
    assert_eq!(out["mqtt"]["server"], "localhost");
    assert_eq!(out["audio"]["sample_rate"], 48000);
}

#[tokio::test]
async fn test_config_endpoint_returns_snapshot() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/config")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_samples_endpoint_total_ms_handles_zero_sample_rate() {
    // total_ms must handle the edge case gracefully: when sample_rate is 0
    // (e.g. a still-loading streaming buffer recorded in the snapshot), the
    // handler must not divide by zero and must report total_ms as 0.
    let (state, _rx) = create_test_state();

    {
        let mut snapshot = state.status.write().unwrap();
        snapshot.active_samples = 1;
        snapshot.samples.push(sample_status(
            1,
            "test_voice",
            "/test/audio.wav",
            1000, // frames
            0,    // sample_rate 0 -> divide-by-zero guard
        ));
    }

    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/samples")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let samples = json["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 1);

    // With sample_rate 0, total_ms should be 0 (not cause a divide-by-zero).
    let total_ms = samples[0]["total_ms"].as_u64().unwrap();
    assert_eq!(total_ms, 0, "total_ms should be 0 when sample_rate is 0");
    // position_ms remains 0 as well (not tracked control-side).
    let position_ms = samples[0]["position_ms"].as_u64().unwrap();
    assert_eq!(position_ms, 0, "position_ms should be 0");
}

#[tokio::test]
async fn test_require_auth_protects_status_routes() {
    let (state, _rx) = create_test_state_require_auth("tok");
    let app = create_router(state, false, false);

    // Without a token, even /status is now 401.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/status")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // With the Bearer token, /status returns 200.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/status")
        .header(header::AUTHORIZATION, "Bearer tok")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_health_open_even_under_require_auth() {
    let (state, _rx) = create_test_state_require_auth("tok");
    let app = create_router(state, false, false);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn test_exposure_warning_only_when_exposed_without_auth() {
    use mqttaudio::http::exposure_warning;
    use std::net::SocketAddr;

    let loopback: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    let exposed: SocketAddr = "0.0.0.0:8080".parse().unwrap();

    assert!(exposure_warning(&exposed, &None, false).is_some());
    assert!(exposure_warning(&loopback, &None, false).is_none());
    assert!(exposure_warning(&exposed, &Some("t".to_string()), false).is_none());
    assert!(exposure_warning(&exposed, &None, true).is_none());
}

// =============================================================================
// Error-path contract tests (F10)
//
// These lock the HTTP error contract that earlier example-based tests left
// unasserted: a non-JSON body, bodies missing a required field, and the
// command-channel-closed 500. They assert the codes/shapes the current router
// already produces; if a future change alters them, these turn red.
// =============================================================================

#[tokio::test]
async fn test_command_non_json_body_returns_400() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    // A body that is not valid JSON, sent with the JSON content type, is
    // rejected by axum's `Json<Value>` extractor with 400 before the handler
    // runs. The rejection body is axum's plaintext parse error, NOT a
    // `CommandResponse` — `handle_command`'s own invalid-JSON branch is
    // unreachable because re-serializing an already-parsed `Value` cannot fail
    // (see docs/bugs.md: surfaced dead error branch).
    let request = Request::builder()
        .method(Method::POST)
        .uri("/command")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("this is not json"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Failed to parse the request body as JSON"),
        "400 body should be axum's JSON parse rejection, got: {text}"
    );
    // It is plaintext, not a CommandResponse JSON object.
    assert!(
        serde_json::from_slice::<serde_json::Value>(&body).is_err(),
        "the 400 rejection body is plaintext, not JSON"
    );
}

#[tokio::test]
async fn test_play_missing_required_file_returns_4xx() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    // `PlayParams.file` has no #[serde(default)]; omitting it is an axum
    // deserialization rejection (422 Unprocessable Entity).
    let body = serde_json::json!({ "volume": 0.5 });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/play")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert!(
        response.status().is_client_error(),
        "missing required `file` must be a 4xx, got {}",
        response.status()
    );
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_volume_missing_required_volume_returns_4xx() {
    let (state, _rx) = create_test_state();
    let app = create_router(state, false, false);

    // `VolumeParams.volume` is required (no default); omitting it is a 422.
    let body = serde_json::json!({ "voice": "music" });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/volume")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert!(
        response.status().is_client_error(),
        "missing required `volume` must be a 4xx, got {}",
        response.status()
    );
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_command_send_failure_returns_500_with_success_false() {
    let (state, cmd_rx) = create_test_state();
    // Dropping the receiver closes the command channel; the next send() fails,
    // driving `handle_command` into its 500 + CommandResponse::error branch.
    drop(cmd_rx);
    let app = create_router(state, false, false);

    let command = serde_json::json!({ "command": "stopall", "message": {} });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/command")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(command.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().is_some_and(|e| !e.is_empty()),
        "500 response must carry a non-empty error string, got: {json}"
    );
    // The success path's `message` field must be absent on an error response.
    assert!(
        json.get("message").is_none(),
        "error response omits message"
    );
}

// =============================================================================
// Real-value status tests (F12)
//
// Earlier status tests asserted only container types on empty state. These
// populate voices, inputs, and the cache and assert the serialized *values*
// (the muted-toggle, reported channels, voice volumes, and cache counts/sizes)
// the handlers compute, mirroring test_samples_endpoint_reports_timing_metadata.
// =============================================================================

#[tokio::test]
async fn test_voices_endpoint_reports_populated_values() {
    let (state, _rx) = create_test_state();

    // Populate two voices with distinct sample counts and a non-default volume.
    {
        let mut vm = state.voice_manager.lock();
        vm.add_sample_to_voice("music"); // music: 1 sample
        vm.add_sample_to_voice("effects"); // effects: 2 samples
        vm.add_sample_to_voice("effects");
        assert!(vm.set_voice_volume("music", 0.25));
    }

    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/voices")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let voices = json["voices"].as_array().unwrap();
    assert_eq!(
        voices.len(),
        2,
        "exactly the two populated voices are reported"
    );

    // Order is unspecified (HashMap), so look each up by id.
    let by_id = |name: &str| -> serde_json::Value {
        voices
            .iter()
            .find(|v| v["id"] == name)
            .unwrap_or_else(|| panic!("voice {name} should be present in {json}"))
            .clone()
    };

    let music = by_id("music");
    assert_eq!(music["sample_count"], 1, "music has one sample");
    let music_vol = music["volume"].as_f64().unwrap();
    assert!(
        (music_vol - 0.25).abs() < 1e-6,
        "music volume should be the value we set (0.25), got {music_vol}"
    );

    let effects = by_id("effects");
    assert_eq!(effects["sample_count"], 2, "effects has two samples");
    let effects_vol = effects["volume"].as_f64().unwrap();
    assert!(
        (effects_vol - 1.0).abs() < 1e-6,
        "effects keeps the default volume 1.0, got {effects_vol}"
    );
}

#[tokio::test]
async fn test_inputs_endpoint_reports_muted_toggle_and_channels() {
    let (state, _rx) = create_test_state();

    // Two inputs: one at full volume (not muted), one at exactly 0.0 (muted).
    // The handler derives `muted` as `volume == 0.0`.
    {
        let mut snapshot = state.status.write().unwrap();
        snapshot.inputs.push(InputStatus {
            index: 0,
            voice_id: "mic_live".to_string(),
            volume: 0.8,
            channels: 2,
        });
        snapshot.inputs.push(InputStatus {
            index: 1,
            voice_id: "mic_muted".to_string(),
            volume: 0.0,
            channels: 6,
        });
    }

    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/inputs")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let inputs = json["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 2, "both inputs are reported");

    // Inputs preserve insertion order (Vec), so index 0 is the live one.
    let live = &inputs[0];
    assert_eq!(live["index"], 0);
    assert_eq!(live["voice_id"], "mic_live");
    assert_eq!(live["channels"], 2, "channels reported verbatim");
    let live_vol = live["volume"].as_f64().unwrap();
    assert!(
        (live_vol - 0.8).abs() < 1e-6,
        "volume reported, got {live_vol}"
    );
    assert_eq!(live["muted"], false, "non-zero volume is not muted");

    let muted = &inputs[1];
    assert_eq!(muted["index"], 1);
    assert_eq!(muted["voice_id"], "mic_muted");
    assert_eq!(muted["channels"], 6, "six-channel input reported as 6");
    assert_eq!(muted["muted"], true, "volume == 0.0 reports muted = true");
}

#[tokio::test]
async fn test_cache_status_endpoint_reports_populated_entry_and_size() {
    let (state, _rx) = create_test_state();

    // Load a real fixture so the memory cache holds exactly one decoded entry
    // with a measurable byte size (allowlist is empty -> local path permitted).
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/audio/test_440hz_2s.wav");
    let loaded_bytes = {
        let mut cm = state.cache_manager.lock().await;
        cm.get_or_load(fixture, 48000)
            .await
            .expect("fixture should decode and populate the cache");
        cm.memory_stats().size_bytes
    };
    assert!(loaded_bytes > 0, "decoded entry must have a non-zero size");

    let app = create_router(state, false, false);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/status/cache")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Memory cache: exactly one entry, the byte size the cache reports, and a
    // size_mb that is the byte count divided by 1 MiB.
    assert_eq!(
        json["memory"]["entries"], 1,
        "one fixture was loaded, so one memory entry, got {json}"
    );
    let size_bytes = json["memory"]["size_bytes"].as_u64().unwrap();
    assert_eq!(
        size_bytes, loaded_bytes,
        "reported size_bytes must equal the cache's memory_stats"
    );
    let size_mb = json["memory"]["size_mb"].as_f64().unwrap();
    assert!(
        (size_mb - (size_bytes as f64 / (1024.0 * 1024.0))).abs() < 1e-9,
        "size_mb must be size_bytes / 1 MiB, got {size_mb}"
    );

    // Disk cache was untouched: zero entries, zero bytes.
    assert_eq!(json["disk"]["entries"], 0, "disk cache stays empty");
    assert_eq!(json["disk"]["size_bytes"], 0);
}

// =============================================================================
// /version and /metrics (F3 / D40)
// =============================================================================

/// GET a path on a router built from `state` and return its parsed JSON body,
/// asserting a 200. Keeps the observability tests focused on the payload shape.
async fn get_json(state: AppState, uri: &str) -> serde_json::Value {
    let app = create_router(state, false, false);
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri} should return 200");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_version_endpoint_returns_name_and_version() {
    // /version surfaces the crate name and version. git_sha is optional (present
    // only when the build injected MQTTAUDIO_GIT_SHA), so it is not required here.
    let (state, _rx) = create_test_state();
    let json = get_json(state, "/version").await;

    assert_eq!(json["name"], "mqttaudio", "name must be the crate name");
    assert_eq!(
        json["version"],
        env!("CARGO_PKG_VERSION"),
        "version must be the crate version"
    );
}

#[tokio::test]
async fn test_metrics_reports_play_latency_through_the_tracker() {
    // Sprint 11 (D50): /metrics surfaces the first-start latency aggregate. The
    // values flow through the real probe-fold path: a probe is registered, the
    // audio thread's store is simulated (the RT-side store itself is covered by
    // the alloc harness and latency_test against real mixes), the fold runs, and
    // /metrics reports the folded numbers verbatim.
    use mqttaudio::http::{LatencyTracker, PlayLatencyStats};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let stats = Arc::new(PlayLatencyStats::default());
    let tracker = LatencyTracker::new(stats.clone());
    let probe = tracker.new_probe();
    probe.store(123_456, Ordering::Relaxed); // the audio thread's first-mix store
    tracker.fold_fired(); // the reaper tick

    let (mut state, _rx) = create_test_state();
    state.latency = stats;
    let json = get_json(state, "/metrics").await;

    assert_eq!(
        json["latency"]["play_to_first_mix_ns"]["last"], 123_456,
        "last latency must be the folded probe value"
    );
    assert_eq!(
        json["latency"]["play_to_first_mix_ns"]["max"], 123_456,
        "max latency must track the folded value"
    );
    assert_eq!(
        json["latency"]["plays_measured"], 1,
        "one play was measured"
    );
}

#[tokio::test]
async fn test_metrics_endpoint_returns_real_fields() {
    // /metrics reports real telemetry. uptime is derived from a real start_time, so
    // a tiny sleep guarantees it is strictly > 0. The counters are numbers (not
    // placeholders or strings), and the structural fields are present.
    let (state, _rx) = create_test_state();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let json = get_json(state, "/metrics").await;

    let uptime = json["uptime_seconds"]
        .as_f64()
        .expect("uptime_seconds must be a number");
    assert!(
        uptime > 0.0,
        "uptime must be > 0 from a real start_time, got {uptime}"
    );

    // The clip/xrun counters are integers, surfaced verbatim from the shared
    // atomics — never a string, null, or fabricated placeholder.
    assert!(
        json["clips"].is_number(),
        "clips must be a number, got {}",
        json["clips"]
    );
    assert!(
        json["xruns"].is_number(),
        "xruns must be a number, got {}",
        json["xruns"]
    );

    // Activity counts and the output channel count are present and numeric.
    assert!(
        json["active_voices"].is_number(),
        "active_voices must be a number"
    );
    assert!(
        json["active_samples"].is_number(),
        "active_samples must be a number"
    );
    assert!(
        json["active_inputs"].is_number(),
        "active_inputs must be a number"
    );
    assert_eq!(
        json["output_channels"], 2,
        "output_channels from the snapshot"
    );

    // Per-voice ducking is an object (empty when nothing is ducked).
    assert!(
        json["ducking"].is_object(),
        "ducking must be an object, got {}",
        json["ducking"]
    );

    // Fresh state: every counter reads zero, no voices, no ducking.
    assert_eq!(json["clips"], 0);
    assert_eq!(json["xruns"], 0);
    assert_eq!(json["active_voices"], 0);
    assert_eq!(json["ducking"].as_object().unwrap().len(), 0);
}

#[tokio::test]
async fn test_metrics_surfaces_the_real_clip_and_xrun_counters() {
    // The clip and xrun counters are the SAME Arc<AtomicU64> the audio engine
    // increments (Sprint 6 limiter / Sprint 5 cpal error callback). Incrementing
    // them must be visible verbatim in /metrics — proving they are threaded, not
    // hardcoded to 0.
    let (state, _rx) = create_test_state();
    state
        .clip_count
        .store(7, std::sync::atomic::Ordering::Relaxed);
    state.xruns.store(3, std::sync::atomic::Ordering::Relaxed);

    let json = get_json(state, "/metrics").await;
    assert_eq!(json["clips"], 7, "clips must reflect the shared clip_count");
    assert_eq!(
        json["xruns"], 3,
        "xruns must reflect the shared xruns counter"
    );
}

#[tokio::test]
async fn test_metrics_reports_active_voice_and_sample_counts() {
    // active_voices comes from the VoiceManager; active_samples/active_inputs from
    // the control-side status snapshot. Populate both and assert the values.
    let (state, _rx) = create_test_state();
    {
        let mut vm = state.voice_manager.lock();
        vm.add_sample_to_voice("music");
        vm.add_sample_to_voice("sfx");
    }
    {
        let mut snapshot = state.status.write().unwrap();
        snapshot.active_samples = 2;
        snapshot.inputs.push(InputStatus {
            index: 0,
            voice_id: "mic".to_string(),
            volume: 1.0,
            channels: 1,
        });
    }

    let json = get_json(state, "/metrics").await;
    assert_eq!(json["active_voices"], 2, "two voices in the VoiceManager");
    assert_eq!(
        json["active_samples"], 2,
        "two active samples in the snapshot"
    );
    assert_eq!(json["active_inputs"], 1, "one configured input");
}

#[tokio::test]
async fn test_metrics_reports_per_voice_ducking_state() {
    // The control thread records each ducked voice's resolved target multiplier in
    // the shared ducking snapshot. /metrics must surface it per voice.
    let (state, _rx) = create_test_state();
    {
        let mut ducking = state.ducking.write().unwrap();
        ducking.insert("music".to_string(), 0.1);
        ducking.insert("ambience".to_string(), 0.5);
    }

    let json = get_json(state, "/metrics").await;
    let ducking = json["ducking"].as_object().unwrap();
    assert_eq!(ducking.len(), 2, "both ducked voices reported, got {json}");
    assert!(
        (ducking["music"].as_f64().unwrap() - 0.1).abs() < 1e-6,
        "music ducked to 0.1"
    );
    assert!(
        (ducking["ambience"].as_f64().unwrap() - 0.5).abs() < 1e-6,
        "ambience ducked to 0.5"
    );
}

#[tokio::test]
async fn test_voices_endpoint_reports_per_voice_ducking_state() {
    // /status/voices is enriched with the ducked target per voice. A voice that is
    // ducked carries its multiplier; a voice at full volume reports 1.0 (the
    // resting multiplier), so an operator can see the ducking state at a glance.
    let (state, _rx) = create_test_state();
    {
        let mut vm = state.voice_manager.lock();
        vm.add_sample_to_voice("music");
        vm.add_sample_to_voice("narration");
    }
    {
        // narration is the primary (full volume); music is ducked to 0.1.
        let mut ducking = state.ducking.write().unwrap();
        ducking.insert("music".to_string(), 0.1);
    }

    let json = get_json(state, "/status/voices").await;
    let voices = json["voices"].as_array().unwrap();
    let by_id = |name: &str| -> serde_json::Value {
        voices
            .iter()
            .find(|v| v["id"] == name)
            .unwrap_or_else(|| panic!("voice {name} should be present in {json}"))
            .clone()
    };

    let music = by_id("music");
    let music_duck = music["ducking_multiplier"]
        .as_f64()
        .expect("a ducked voice must report its multiplier");
    assert!(
        (music_duck - 0.1).abs() < 1e-6,
        "music should report the ducked target 0.1, got {music_duck}"
    );

    let narration = by_id("narration");
    let narration_duck = narration["ducking_multiplier"]
        .as_f64()
        .expect("an unducked voice reports the resting multiplier");
    assert!(
        (narration_duck - 1.0).abs() < 1e-6,
        "an unducked voice reports 1.0, got {narration_duck}"
    );
}
