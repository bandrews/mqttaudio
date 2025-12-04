// ABOUTME: Integration tests for the HTTP REST API.
// ABOUTME: Tests endpoints, authentication, and command routing.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mqttaudio::audio::mixer::MixerState;
use mqttaudio::cache::CacheManager;
use mqttaudio::http::{create_router, AppState, LogBroadcaster};
use mqttaudio::voice::VoiceManager;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tower::util::ServiceExt;

/// Create a test AppState with mock components.
fn create_test_state() -> (AppState, mpsc::Receiver<String>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>(100);

    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels: 2,
        ducking_engine: None,
        bass_management: None,
    }));

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));
    let cache_manager = Arc::new(Mutex::new(
        CacheManager::new(std::env::temp_dir().join("mqttaudio_test_cache")).unwrap(),
    ));

    let state = AppState {
        cmd_tx,
        mixer_state,
        voice_manager,
        cache_manager,
        auth_token: None,
        log_broadcaster: Arc::new(LogBroadcaster::new()),
    };

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
