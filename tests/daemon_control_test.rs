// ABOUTME: Real-daemon regression coverage for command responsiveness and load cancellation.
// ABOUTME: Requires an output device (or a configured ALSA test PCM) and opens no microphone.

use axum::{routing::get, Router};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "opens the system output device; run with --ignored for device validation"]
async fn slow_load_does_not_block_stop_and_cancelled_audio_never_starts() {
    let temp = tempfile::tempdir().unwrap();
    let seen = Arc::new(tokio::sync::Notify::new());
    let signal = seen.clone();
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_address = server.local_addr().unwrap();
    let source = tokio::spawn(async move {
        axum::serve(
            server,
            Router::new().route(
                "/slow.wav",
                get(move || {
                    let signal = signal.clone();
                    async move {
                        signal.notify_one();
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        include_bytes!("audio/test_440hz_2s.wav").as_slice()
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config_path = temp.path().join("config.json");
    std::fs::write(
        &config_path,
        json!({
            "http": {"enabled": true, "port": port},
            "audio": {"channels": 2, "sample_rate": 48000},
            "cache": {"directory": temp.path().join("cache"), "enabled": false},
            "security": {"allowed_directories": [temp.path()]}
        })
        .to_string(),
    )
    .unwrap();
    let log = std::fs::File::create(temp.path().join("daemon.log")).unwrap();
    let mut daemon = tokio::process::Command::new(env!("CARGO_BIN_EXE_mqttaudio"))
        .env("MQTTAUDIO_CONFIG", &config_path)
        .kill_on_drop(true)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let ready_by = Instant::now() + Duration::from_secs(10);
    loop {
        if client.get(format!("{base}/health")).send().await.is_ok() {
            break;
        }
        if daemon.try_wait().unwrap().is_some() || Instant::now() >= ready_by {
            panic!(
                "daemon failed to start: {}",
                std::fs::read_to_string(temp.path().join("daemon.log")).unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let play_client = client.clone();
    let play_url = format!("{base}/play");
    let play = tokio::spawn(async move {
        play_client
            .post(play_url)
            .json(&json!({"file": format!("http://{source_address}/slow.wav")}))
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(3), seen.notified())
        .await
        .unwrap();
    let fast_file = temp.path().join("fast.wav");
    std::fs::write(&fast_file, include_bytes!("audio/test_440hz_2s.wav")).unwrap();
    let fast_started = Instant::now();
    let fast = client
        .post(format!("{base}/play"))
        .json(&json!({"file": fast_file, "volume": 0.0}))
        .send()
        .await
        .unwrap();
    assert_eq!(fast.status(), reqwest::StatusCode::OK);
    assert!(
        fast_started.elapsed() < Duration::from_millis(750),
        "one slow URL delayed an independent local play"
    );
    let started = Instant::now();
    let stop = client.post(format!("{base}/stopall")).send().await.unwrap();
    assert_eq!(stop.status(), reqwest::StatusCode::OK);
    assert!(
        started.elapsed() < Duration::from_millis(750),
        "stopall waited on audio loading"
    );
    assert_eq!(play.await.unwrap().status(), reqwest::StatusCode::CONFLICT);
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let samples: serde_json::Value = client
        .get(format!("{base}/status/samples"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        samples["samples"].as_array().unwrap().len(),
        0,
        "cancelled load started playing"
    );
    let missing = client
        .post(format!("{base}/play"))
        .json(&json!({"file": temp.path().join("missing.wav")}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    let forbidden = client
        .post(format!("{base}/play"))
        .json(&json!({"file": "/etc/passwd"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);
    let unknown_input = client
        .post(format!("{base}/input/mute"))
        .json(&json!({"input": "missing", "mute": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_input.status(), reqwest::StatusCode::NOT_FOUND);
    daemon.kill().await.unwrap();
    source.abort();
}
