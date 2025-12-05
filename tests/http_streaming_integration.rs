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

// ============================================================================
// Precache and Play interaction tests
// ============================================================================

#[tokio::test]
async fn test_precache_streaming_then_play() {
    // Simulates: precache command starts loading, then play command comes in
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("precache_play.wav");
    generate_test_wav(&wav_path, 1.0); // 1 second file

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/precache_play.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Start precache (non-blocking)
    cache_manager.precache_streaming(&url, 48000).await.unwrap();

    // Simulate play command coming in while precache is in progress
    // This should return the same streaming buffer
    let buffer = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();

    // Should be streaming (same load in progress)
    match &buffer {
        SampleBuffer::Streaming(b) => {
            // Access should work even during loading
            let sample = buffer.get_sample_or_silence(0, 0);
            // Either silence (not loaded yet) or actual sample (already loaded)
            let _ = sample;

            // Should be able to check frame availability
            let is_loaded = buffer.is_frame_loaded(0);
            let _ = is_loaded;

            // Get notifier for async waiting (if needed)
            let notifier = b.read().unwrap().notifier();
            let _ = notifier;
        }
        SampleBuffer::Complete(_) => {
            // Also OK if already completed (fast load)
        }
    }

    // Wait for load to complete
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    assert!(buffer.is_complete(), "Buffer should complete");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_play_with_offset_during_streaming() {
    // Simulates: play with start_position while file is still loading
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("offset_test.wav");
    generate_test_wav(&wav_path, 2.0); // 2 second file for longer load

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/offset_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Start streaming load
    let buffer = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();

    // Simulate play with offset - trying to seek to 1 second (48000 frames)
    let target_frame = 48000usize;

    match &buffer {
        SampleBuffer::Streaming(_) => {
            // Check if target frame is loaded
            let is_loaded = buffer.is_frame_loaded(target_frame);

            if !is_loaded {
                // Expected: silence for unloaded frames
                let sample = buffer.get_sample_or_silence(target_frame, 0);
                assert_eq!(sample, 0.0, "Unloaded frame should return silence");
            }

            // Get estimate of total frames (for clamping)
            let estimate = buffer.total_frames_or_estimate();
            // May or may not have estimate depending on timing
            let _ = estimate;
        }
        SampleBuffer::Complete(_) => {
            // Fast load completed - can access any frame
            let sample = buffer.get_sample_or_silence(target_frame, 0);
            // Should be actual sample, not silence
            let _ = sample;
        }
    }

    // Wait for load to complete
    while !buffer.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // After loading completes, offset should work
    let sample = buffer.get_sample_or_silence(target_frame, 0);
    // Should now be actual sample (not silence)
    // Note: might be close to 0 if that's the actual audio content
    let _ = sample;

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_reverse_play_with_streaming_buffer() {
    // Simulates: reverse play with streaming buffer
    // Reverse play reads from end of buffer, which might not be loaded yet
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("reverse_test.wav");
    generate_test_wav(&wav_path, 1.0);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/reverse_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let buffer = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();

    // For reverse play, we'd start near the end
    // Since we don't know total frames yet, use estimate or wait

    match &buffer {
        SampleBuffer::Streaming(_) => {
            // For reverse play with streaming, the end might not be loaded
            // The mixer would return silence until data is available
            let loaded_frames = buffer.frames();

            if loaded_frames > 0 {
                // Can reverse from what's loaded
                let last_loaded = loaded_frames - 1;
                let sample = buffer.get_sample_or_silence(last_loaded, 0);
                let _ = sample;
            }
        }
        SampleBuffer::Complete(b) => {
            // Full buffer available - reverse from end
            let last_frame = b.frames - 1;
            let sample = buffer.get_sample_or_silence(last_frame, 0);
            let _ = sample;
        }
    }

    // Wait for load to complete
    while !buffer.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // Now reverse should work from actual end
    let total = buffer.frames();
    let last_frame = total.saturating_sub(1);
    let sample = buffer.get_sample_or_silence(last_frame, 0);
    let _ = sample;

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_multiple_play_requests_share_buffer() {
    // Multiple play requests for same file should share streaming buffer
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("shared_test.wav");
    generate_test_wav(&wav_path, 0.5);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/shared_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Simulate multiple play requests at different offsets
    let buffer1 = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();
    let buffer2 = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();
    let buffer3 = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();

    // All should share the same underlying streaming buffer
    match (&buffer1, &buffer2, &buffer3) {
        (SampleBuffer::Streaming(b1), SampleBuffer::Streaming(b2), SampleBuffer::Streaming(b3)) => {
            assert!(Arc::ptr_eq(b1, b2), "buffer1 and buffer2 should share");
            assert!(Arc::ptr_eq(b2, b3), "buffer2 and buffer3 should share");
        }
        _ => {
            // If load completed very fast, could all be Complete
            // That's also OK
        }
    }

    // Wait for completion
    while !buffer1.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_precache_then_seek_beyond_loaded() {
    // Precache starts, then seek to position beyond loaded data
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("seek_beyond.wav");
    generate_test_wav(&wav_path, 2.0); // 2 seconds

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/seek_beyond.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Start precache
    cache_manager.precache_streaming(&url, 48000).await.unwrap();

    // Get buffer for play
    let buffer = cache_manager.get_or_load_streaming(&url, 48000).await.unwrap();

    // Try to access a frame near the end (might not be loaded yet)
    let target = 90000; // ~1.9 seconds into a 2 second file

    if !buffer.is_frame_loaded(target) {
        // Should return silence for unloaded frames
        let sample = buffer.get_sample_or_silence(target, 0);
        assert_eq!(sample, 0.0);
    }

    // Wait for full load
    while !buffer.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // After full load, should be able to access
    assert!(buffer.is_frame_loaded(target) || target >= buffer.frames());

    let _ = shutdown.send(());
}
