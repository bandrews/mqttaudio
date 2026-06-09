// ABOUTME: Integration test for the WebSocket log-streaming endpoint (/ws).
// ABOUTME: Upgrades a real connection, asserts the welcome frame, and a broadcast log frame.

use futures_util::{SinkExt, StreamExt};
use mqttaudio::cache::CacheManager;
use mqttaudio::http::{create_router, AppState, LogBroadcaster, StatusSnapshot};
use mqttaudio::voice::VoiceManager;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Build an AppState and return it alongside the command receiver (kept alive)
/// and a clone of the shared log broadcaster (so the test can broadcast after
/// the client has subscribed).
fn build_state() -> (AppState, mpsc::Receiver<String>, Arc<LogBroadcaster>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>(100);
    let status = Arc::new(RwLock::new(StatusSnapshot {
        active_samples: 0,
        output_channels: 2,
        samples: Vec::new(),
        inputs: Vec::new(),
    }));
    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));
    let cache_manager = Arc::new(tokio::sync::Mutex::new(
        CacheManager::new(std::env::temp_dir().join("mqttaudio_ws_test_cache")).unwrap(),
    ));
    let log_broadcaster = Arc::new(LogBroadcaster::new());

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
        log_broadcaster: log_broadcaster.clone(),
        telemetry_enabled: Arc::new(AtomicBool::new(false)),
        output_meters: Arc::new(Vec::new()),
        state_broadcaster: Arc::new(LogBroadcaster::new()),
        config_json: Arc::new(serde_json::json!({})),
        latency: Arc::new(mqttaudio::http::PlayLatencyStats::default()),
    };

    (state, cmd_rx, log_broadcaster)
}

/// Receive the next text frame, failing the test on timeout/close/non-text.
async fn next_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a WebSocket frame")
            .expect("WebSocket stream ended unexpectedly")
            .expect("WebSocket frame was an error");
        match msg {
            Message::Text(t) => return t,
            // Ignore protocol frames the server/runtime may interleave.
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_websocket_welcome_and_broadcast() {
    let (state, _cmd_rx, broadcaster) = build_state();
    // websocket_enabled = true wires up the /ws route (routes.rs only mounts /ws
    // when this flag is set).
    let app = create_router(state, false, true);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws, _resp) = connect_async(&url).await.expect("failed to connect to /ws");

    // 1) Welcome frame: {"type":"connected", "message":..., "version":...}.
    let welcome_raw = next_text(&mut ws).await;
    let welcome: serde_json::Value =
        serde_json::from_str(&welcome_raw).expect("welcome frame must be JSON");
    assert_eq!(welcome["type"], "connected", "welcome type, got {welcome}");
    assert_eq!(
        welcome["version"],
        env!("CARGO_PKG_VERSION"),
        "welcome must carry the crate version, got {welcome}"
    );
    assert!(
        welcome["message"].as_str().is_some_and(|m| !m.is_empty()),
        "welcome must carry a message, got {welcome}"
    );

    // Having received the welcome, the connection has subscribed to the
    // broadcaster (the welcome is sent immediately after subscribe()), so a
    // broadcast now will be delivered to this client.
    broadcaster.broadcast("hello from the daemon".to_string());

    // 2) Log frame: {"type":"log","message":"hello from the daemon"}.
    let log_raw = next_text(&mut ws).await;
    let log: serde_json::Value = serde_json::from_str(&log_raw).expect("log frame must be JSON");
    assert_eq!(log["type"], "log", "log frame type, got {log}");
    assert_eq!(
        log["message"], "hello from the daemon",
        "log frame must carry the broadcast message verbatim, got {log}"
    );

    // Close cleanly so the spawned server's per-connection task ends.
    let _ = ws.send(Message::Close(None)).await;
}

#[tokio::test]
async fn test_websocket_route_absent_when_disabled() {
    // When websocket_enabled = false, /ws is not mounted, so the upgrade fails.
    let (state, _cmd_rx, _broadcaster) = build_state();
    let app = create_router(state, false, false);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let result = connect_async(&url).await;
    assert!(
        result.is_err(),
        "/ws must not be reachable when websocket_enabled is false"
    );
}
