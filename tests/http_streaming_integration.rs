// ABOUTME: Integration tests for HTTP streaming audio playback.
// ABOUTME: Verifies HttpStreamReader works with StreamingDecoder end-to-end.

use mqttaudio::audio::streaming_decoder::StreamingDecoder;
use mqttaudio::cache::http_stream::start_http_stream;
use mqttaudio::config::ResamplerQuality;
use std::io::Write;
use std::path::PathBuf;
use symphonia::core::probe::Hint;
use tempfile::TempDir;
use tokio::sync::oneshot;

/// Generate a simple WAV file for testing
fn generate_test_wav(path: &std::path::Path, duration_secs: f32) {
    let sample_rate = 44100u32;
    let channels = 2u16;
    let frames = (duration_secs * sample_rate as f32) as usize;

    let mut samples = Vec::with_capacity(frames * channels as usize);
    for frame in 0..frames {
        let t = frame as f32 / sample_rate as f32;
        let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
        for _ in 0..channels {
            samples.push(sample);
        }
    }

    // Write WAV file
    let mut file = std::fs::File::create(path).unwrap();

    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = samples.len() as u32 * 2;
    let file_size = 36 + data_size;

    // RIFF header
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();

    // fmt chunk
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bits_per_sample.to_le_bytes()).unwrap();

    // data chunk
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();

    for &sample in &samples {
        let i16_sample = (sample * 32767.0) as i16;
        file.write_all(&i16_sample.to_le_bytes()).unwrap();
    }
}

/// Start a simple HTTP server serving files from a directory
async fn start_test_server(serve_dir: PathBuf) -> (u16, oneshot::Sender<()>) {
    use axum::Router;
    use tower_http::services::ServeDir;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let app = Router::new().nest_service("/", ServeDir::new(serve_dir));

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    // Give server time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    (port, shutdown_tx)
}

#[tokio::test]
async fn test_http_stream_with_streaming_decoder() {
    // Setup: create temp dir and test WAV file
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("test.wav");
    generate_test_wav(&wav_path, 0.5); // 0.5 seconds

    // Start HTTP server
    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/test.wav", port);

    // Start HTTP stream
    let reader = start_http_stream(&url).await.unwrap();

    // Give the download a moment to start buffering
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create StreamingDecoder from the HTTP stream reader
    let mut hint = Hint::new();
    hint.with_extension("wav");

    let decoder = StreamingDecoder::new(reader, Some(&hint), None, ResamplerQuality::Fast).unwrap();

    assert_eq!(decoder.channels(), 2);
    assert_eq!(decoder.sample_rate(), 44100);

    // Decode all chunks
    let mut total_samples = 0;
    let mut chunk_count = 0;
    for chunk_result in decoder {
        let chunk = chunk_result.unwrap();
        total_samples += chunk.len();
        chunk_count += 1;
    }

    // 0.5 seconds at 44100 Hz stereo = 44100 samples
    assert!(
        total_samples > 40000,
        "Expected ~44100 samples, got {}",
        total_samples
    );
    assert!(
        total_samples < 50000,
        "Expected ~44100 samples, got {}",
        total_samples
    );
    assert!(chunk_count >= 1, "Expected at least 1 chunk");

    // Cleanup
    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_http_stream_with_resampling() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("test_resample.wav");
    generate_test_wav(&wav_path, 0.5);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/test_resample.wav", port);

    let reader = start_http_stream(&url).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut hint = Hint::new();
    hint.with_extension("wav");

    // Resample from 44100 to 48000
    let decoder =
        StreamingDecoder::new(reader, Some(&hint), Some(48000), ResamplerQuality::Fast).unwrap();

    assert_eq!(decoder.sample_rate(), 48000);
    assert_eq!(decoder.source_sample_rate(), 44100);

    let mut total_samples = 0;
    for chunk_result in decoder {
        let chunk = chunk_result.unwrap();
        total_samples += chunk.len();
    }

    // 0.5 seconds at 48000 Hz stereo = 48000 samples
    assert!(
        total_samples > 44000,
        "Expected ~48000 samples, got {}",
        total_samples
    );
    assert!(
        total_samples < 52000,
        "Expected ~48000 samples, got {}",
        total_samples
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_http_stream_progress_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("progress_test.wav");
    generate_test_wav(&wav_path, 1.0); // 1 second = larger file

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/progress_test.wav", port);

    let reader = start_http_stream(&url).await.unwrap();

    // Check initial progress
    let (_downloaded, total) = reader.progress();
    // May have already downloaded some or all (it's a small file)
    assert!(total.is_some(), "Expected Content-Length header");

    // Wait for download to complete
    while !reader.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    let (downloaded_final, _) = reader.progress();
    assert!(downloaded_final > 0, "Should have downloaded bytes");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_http_stream_404_error() {
    let temp_dir = TempDir::new().unwrap();

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/nonexistent.wav", port);

    let result = start_http_stream(&url).await;
    assert!(result.is_err(), "Expected error for 404");

    let _ = shutdown.send(());
}

// ============================================================================
// CacheManager streaming tests
// ============================================================================

use mqttaudio::audio::streaming::SampleBuffer;
use mqttaudio::cache::CacheManager;

#[tokio::test]
async fn test_cache_manager_streaming_load() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("stream_test.wav");
    generate_test_wav(&wav_path, 0.5);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/stream_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Request streaming load
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Should return a streaming buffer (since it's HTTP and not cached)
    match &buffer {
        SampleBuffer::Streaming(_) => {
            // Good - got streaming buffer
        }
        SampleBuffer::Complete(_) => {
            panic!("Expected streaming buffer for uncached HTTP URL");
        }
    }

    // Wait for load to complete
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    assert!(buffer.is_complete(), "Buffer should be complete after waiting");
    assert!(buffer.frames() > 0, "Should have loaded frames");
    assert_eq!(buffer.channels(), 2);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_cache_manager_memory_cache_hit() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("cache_test.wav");
    generate_test_wav(&wav_path, 0.3);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/cache_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // First load - should be streaming
    let buffer1 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Wait for complete
    while !buffer1.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    // Clean up completed loads to promote to memory cache
    cache_manager.cleanup_completed_loads();

    // Second load - should be a memory cache hit (Complete)
    let buffer2 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    match &buffer2 {
        SampleBuffer::Complete(_) => {
            // Good - got cached buffer
        }
        SampleBuffer::Streaming(_) => {
            panic!("Expected complete buffer for cached URL");
        }
    }

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_cache_manager_concurrent_requests() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("concurrent_test.wav");
    generate_test_wav(&wav_path, 0.5);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/concurrent_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // First request starts the load
    let buffer1 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Second request for same URL should return same streaming buffer
    let buffer2 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Both should be streaming (same active load)
    match (&buffer1, &buffer2) {
        (SampleBuffer::Streaming(b1), SampleBuffer::Streaming(b2)) => {
            // Should be the same Arc (same buffer)
            assert!(Arc::ptr_eq(b1, b2), "Should share same streaming buffer");
        }
        _ => {
            panic!("Expected both to be streaming buffers");
        }
    }

    // Wait for load to complete before cleanup
    let mut attempts = 0;
    while !buffer1.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_cache_manager_is_loading() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("loading_test.wav");
    generate_test_wav(&wav_path, 0.5);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/loading_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Before loading
    assert!(!cache_manager.is_loading(&url));

    // Start loading
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Should be loading (or possibly already done if fast)
    // Just verify the method doesn't panic
    let _is_loading = cache_manager.is_loading(&url);

    // Can also check progress
    if let Some((frames, _total)) = cache_manager.loading_progress(&url) {
        // frames is usize, always >= 0
        let _ = frames;
    }

    // Wait for load to complete before cleanup
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    let _ = shutdown.send(());
}

use std::sync::Arc;
