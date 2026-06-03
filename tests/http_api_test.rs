// ABOUTME: Integration tests for the HTTP REST API.
// ABOUTME: Tests endpoints, authentication, and command routing.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mqttaudio::cache::CacheManager;
use mqttaudio::http::{create_router, AppState, LogBroadcaster, SampleStatus, StatusSnapshot};
use mqttaudio::voice::VoiceManager;
use parking_lot::Mutex;
use std::sync::atomic::AtomicU64;
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
        auth_token: None,
        require_auth: false,
        log_broadcaster: Arc::new(LogBroadcaster::new()),
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
