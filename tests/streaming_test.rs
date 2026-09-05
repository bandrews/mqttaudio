#[derive(Clone)]
struct TestCacheOptions {
    max_memory_mb: u32,
    disk_enabled: bool,
    revalidate_after_seconds: u64,
}
impl Default for TestCacheOptions {
    fn default() -> Self {
        Self {
            max_memory_mb: 0,
            disk_enabled: true,
            revalidate_after_seconds: 300,
        }
    }
}
fn cache_with_options(
    path: std::path::PathBuf,
    options: TestCacheOptions,
) -> Result<mqttaudio::cache::CacheManager, mqttaudio::cache::disk::CacheError> {
    mqttaudio::cache::CacheManager::with_disk_cache(
        path,
        Default::default(),
        if options.max_memory_mb == 0 {
            mqttaudio::config::MemoryCap::Unlimited
        } else {
            mqttaudio::config::MemoryCap::Bytes(options.max_memory_mb as usize * 1024 * 1024)
        },
        Vec::new(),
        options.revalidate_after_seconds,
        options.disk_enabled,
    )
}
// ABOUTME: Integration tests for streaming audio playback (HTTP and local files).
// ABOUTME: Verifies StreamingBuffer, StreamingDecoder, and CacheManager streaming APIs.

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

/// Start a server that serves one `/cue.wav` body and counts every request it receives,
/// so a test can assert a replay hit disk (no extra GET).
async fn start_counting_server(
    body: Vec<u8>,
) -> (
    u16,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    oneshot::Sender<()>,
) {
    use axum::{extract::State, response::IntoResponse, routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let count = Arc::new(AtomicUsize::new(0));
    let body = Arc::new(body);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    async fn serve(
        State((count, body)): State<(Arc<AtomicUsize>, Arc<Vec<u8>>)>,
    ) -> impl IntoResponse {
        count.fetch_add(1, Ordering::SeqCst);
        (
            [(axum::http::header::CONTENT_TYPE, "audio/wav")],
            (*body).clone(),
        )
    }

    let app = Router::new()
        .route("/cue.wav", get(serve))
        .with_state((count.clone(), body));

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    (port, count, shutdown_tx)
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

    assert!(
        buffer.is_complete(),
        "Buffer should be complete after waiting"
    );
    assert!(buffer.frames() > 0, "Should have loaded frames");
    assert_eq!(buffer.channels(), 2);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_failed_streaming_load_retries_on_next_play() {
    // A URL whose decode fails must not poison later plays of the same URL:
    // once the content is fixed on the server, the next play should succeed.
    let temp_dir = TempDir::new().unwrap();
    let bad_path = temp_dir.path().join("retry_test.wav");
    std::fs::write(&bad_path, b"this is not audio data at all, just text").unwrap();

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/retry_test.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    // Wait for the decode to fail
    let errored = |buffer: &SampleBuffer| match buffer {
        SampleBuffer::Streaming(buf) => buf.read().unwrap().has_error(),
        SampleBuffer::Complete(_) => false,
    };
    let mut attempts = 0;
    while !errored(&buffer) && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }
    assert!(
        errored(&buffer),
        "Decode of garbage data should have failed"
    );

    // Fix the file on the server, then play the same URL again
    generate_test_wav(&bad_path, 0.3);

    let buffer2 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

    let mut attempts = 0;
    while !buffer2.is_complete() && attempts < 100 {
        assert!(
            !errored(&buffer2),
            "Second play should not have joined the failed load"
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }
    assert!(
        buffer2.is_complete(),
        "Second play should load successfully"
    );
    assert!(buffer2.frames() > 0);

    let _ = shutdown.send(());
}

async fn wait_complete(buffer: &SampleBuffer) {
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }
    assert!(buffer.is_complete(), "Buffer should have completed loading");
}

#[tokio::test]
async fn test_streamed_url_persists_to_disk_cache() {
    // A URL played through the streaming path must land in the disk cache so
    // a restart does not re-download it.
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("persist_stream.wav");
    generate_test_wav(&wav_path, 0.4);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/persist_stream.wav", port);

    let cache_dir = TempDir::new().unwrap();
    {
        let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
        let buffer = cache_manager
            .get_or_load_streaming(&url, 48000)
            .await
            .unwrap();
        wait_complete(&buffer).await;
        cache_manager.cleanup_completed_loads();
        assert_eq!(
            cache_manager.disk_stats().entry_count,
            1,
            "completed stream should be registered on disk"
        );
    }

    // Kill the server: only the disk cache can satisfy the next play
    let _ = shutdown.send(());
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .expect("second play must be served from the disk cache with the server gone");
    assert!(buffer.is_complete());
    assert!(buffer.frames() > 0);
}

#[tokio::test]
async fn test_completed_streams_respect_memory_budget() {
    // Completed streaming loads must enter the size-limited memory cache
    // instead of accumulating unbounded in the active-load side table.
    let temp_dir = TempDir::new().unwrap();
    for i in 0..4 {
        generate_test_wav(&temp_dir.path().join(format!("budget{}.wav", i)), 1.0);
    }

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let cache_dir = TempDir::new().unwrap();

    // 1 MB budget; each 1s stereo 48k file decodes to ~384 KB
    let options = TestCacheOptions {
        max_memory_mb: 1,
        ..Default::default()
    };
    let mut cache_manager = cache_with_options(cache_dir.path().to_path_buf(), options).unwrap();

    for i in 0..4 {
        let url = format!("http://127.0.0.1:{}/budget{}.wav", port, i);
        let buffer = cache_manager
            .get_or_load_streaming(&url, 48000)
            .await
            .unwrap();
        wait_complete(&buffer).await;
    }
    cache_manager.cleanup_completed_loads();

    let stats = cache_manager.memory_stats();
    assert!(
        stats.size_bytes <= 1024 * 1024,
        "memory cache must stay within its budget, got {} bytes",
        stats.size_bytes
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_cache_disabled_writes_nothing_to_disk() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("nodisk.wav");
    generate_test_wav(&wav_path, 0.3);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/nodisk.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let options = TestCacheOptions {
        disk_enabled: false,
        ..Default::default()
    };
    let mut cache_manager = cache_with_options(cache_dir.path().to_path_buf(), options).unwrap();

    // Streaming play works
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    wait_complete(&buffer).await;
    cache_manager.cleanup_completed_loads();

    // Blocking precache works too
    cache_manager.precache(&url, 48000).await.unwrap();

    assert_eq!(cache_manager.disk_stats().entry_count, 0);
    let files_dir = cache_dir.path().join("files");
    let file_count = files_dir
        .exists()
        .then(|| std::fs::read_dir(&files_dir).unwrap().count())
        .unwrap_or(0);
    assert_eq!(file_count, 0, "disabled cache must write no files");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_revalidation_picks_up_changed_file() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("reval.wav");
    generate_test_wav(&wav_path, 0.3);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/reval.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let options = || TestCacheOptions {
        revalidate_after_seconds: 0, // always check
        ..Default::default()
    };

    {
        let mut cache_manager =
            cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
        let buffer = cache_manager
            .get_or_load_streaming(&url, 48000)
            .await
            .unwrap();
        wait_complete(&buffer).await;
        cache_manager.cleanup_completed_loads();
        assert_eq!(cache_manager.disk_stats().entry_count, 1);
    }

    // Replace the file on the server with a longer one, mtime clearly newer
    generate_test_wav(&wav_path, 1.0);
    let f = std::fs::File::options()
        .append(true)
        .open(&wav_path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(30))
        .unwrap();
    drop(f);

    let mut cache_manager = cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    wait_complete(&buffer).await;

    assert!(
        buffer.frames() > 30_000,
        "revalidation should have fetched the longer replacement, got {} frames",
        buffer.frames()
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_revalidation_respects_interval() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("interval.wav");
    generate_test_wav(&wav_path, 0.3);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/interval.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let options = || TestCacheOptions {
        revalidate_after_seconds: 3600,
        ..Default::default()
    };

    {
        let mut cache_manager =
            cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
        let buffer = cache_manager
            .get_or_load_streaming(&url, 48000)
            .await
            .unwrap();
        wait_complete(&buffer).await;
        cache_manager.cleanup_completed_loads();
    }

    generate_test_wav(&wav_path, 1.0);

    let mut cache_manager = cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    assert!(
        buffer.is_complete(),
        "within the interval the cached copy is served directly"
    );
    assert!(
        buffer.frames() < 30_000,
        "within the interval the stale copy is expected, got {} frames",
        buffer.frames()
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn test_revalidation_serves_cache_when_server_down() {
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("offline.wav");
    generate_test_wav(&wav_path, 0.3);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/offline.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let options = || TestCacheOptions {
        revalidate_after_seconds: 0,
        ..Default::default()
    };

    {
        let mut cache_manager =
            cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
        let buffer = cache_manager
            .get_or_load_streaming(&url, 48000)
            .await
            .unwrap();
        wait_complete(&buffer).await;
        cache_manager.cleanup_completed_loads();
    }

    let _ = shutdown.send(());
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let mut cache_manager = cache_with_options(cache_dir.path().to_path_buf(), options()).unwrap();
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .expect("a dead server must not make cached audio unplayable");
    assert!(buffer.is_complete());
    assert!(buffer.frames() > 0);
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
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

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
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

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

    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

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
    let buffer1 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    let buffer2 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    let buffer3 = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

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
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();

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

// ============================================================================
// Local file streaming tests
// ============================================================================

#[tokio::test]
async fn test_local_file_streaming_load() {
    // Test streaming load from local file (not HTTP)
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("local_stream_test.wav");
    generate_test_wav(&wav_path, 1.0); // 1 second file

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Request streaming load of local file
    let buffer = cache_manager
        .get_or_load_streaming(wav_path.to_str().unwrap(), 48000)
        .await
        .unwrap();

    // Local files may load very quickly - could be either streaming or complete
    // Wait for completion
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        attempts += 1;
    }

    assert!(buffer.is_complete(), "Local file should load completely");
    assert!(buffer.frames() > 0, "Should have loaded frames");
    assert_eq!(buffer.channels(), 2);
}

#[tokio::test]
async fn test_local_file_cache_hit() {
    // Test that local files are cached correctly
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("cache_hit_test.wav");
    generate_test_wav(&wav_path, 0.5);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // First load
    let buffer1 = cache_manager
        .get_or_load_streaming(wav_path.to_str().unwrap(), 48000)
        .await
        .unwrap();

    // Wait for complete
    while !buffer1.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    // Cleanup to promote to memory cache
    cache_manager.cleanup_completed_loads();

    // Second load should be a cache hit
    let buffer2 = cache_manager
        .get_or_load_streaming(wav_path.to_str().unwrap(), 48000)
        .await
        .unwrap();

    // Should be Complete (not Streaming) since it's cached
    match &buffer2 {
        SampleBuffer::Complete(_) => {
            // Good - cache hit
        }
        SampleBuffer::Streaming(_) => {
            panic!("Expected cache hit to return Complete buffer");
        }
    }
}

#[tokio::test]
async fn test_local_file_with_resampling() {
    // Test that local file with different sample rate gets resampled
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("resample_test.wav");
    generate_test_wav(&wav_path, 0.5); // 44100 Hz source

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Request at 48000 Hz (different from 44100 source)
    let buffer = cache_manager
        .get_or_load_streaming(wav_path.to_str().unwrap(), 48000)
        .await
        .unwrap();

    // Wait for complete
    while !buffer.is_complete() {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    // Should have been resampled
    // 0.5 seconds at 48kHz stereo = 24000 frames (approximately, due to resampling)
    let frames = buffer.frames();
    assert!(frames > 20000, "Expected ~24000 frames, got {}", frames);
    assert!(frames < 28000, "Expected ~24000 frames, got {}", frames);
}

#[tokio::test]
async fn test_multiple_local_files_concurrent() {
    // Test loading multiple local files concurrently
    let temp_dir = TempDir::new().unwrap();

    // Create multiple test files
    let wav_path1 = temp_dir.path().join("concurrent1.wav");
    let wav_path2 = temp_dir.path().join("concurrent2.wav");
    let wav_path3 = temp_dir.path().join("concurrent3.wav");
    generate_test_wav(&wav_path1, 0.3);
    generate_test_wav(&wav_path2, 0.4);
    generate_test_wav(&wav_path3, 0.5);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // Load all three concurrently
    let buffer1 = cache_manager
        .get_or_load_streaming(wav_path1.to_str().unwrap(), 48000)
        .await
        .unwrap();
    let buffer2 = cache_manager
        .get_or_load_streaming(wav_path2.to_str().unwrap(), 48000)
        .await
        .unwrap();
    let buffer3 = cache_manager
        .get_or_load_streaming(wav_path3.to_str().unwrap(), 48000)
        .await
        .unwrap();

    // Wait for all to complete
    let mut all_complete = false;
    let mut attempts = 0;
    while !all_complete && attempts < 100 {
        all_complete = buffer1.is_complete() && buffer2.is_complete() && buffer3.is_complete();
        if !all_complete {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        attempts += 1;
    }

    assert!(buffer1.is_complete(), "Buffer 1 should be complete");
    assert!(buffer2.is_complete(), "Buffer 2 should be complete");
    assert!(buffer3.is_complete(), "Buffer 3 should be complete");

    // Each should have different frame counts (different durations)
    let f1 = buffer1.frames();
    let f2 = buffer2.frames();
    let f3 = buffer3.frames();
    assert!(f1 < f2, "Shorter duration should have fewer frames");
    assert!(f2 < f3, "Shorter duration should have fewer frames");
}

#[tokio::test]
async fn test_corrupt_packets_do_not_abort_the_decode() {
    // tests/audio/corrupt_440hz_2s.flac is a valid 2s FLAC with a 200-byte
    // burst of flipped bits in the middle (FLAC frames are CRC-checked, so
    // the corruption surfaces as a decode error). Corrupt frames are
    // recoverable per Symphonia's contract, so the file must still play -
    // minus a brief dropout - rather than failing outright.
    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/audio/corrupt_440hz_2s.flac"
    );
    let buffer = cache_manager
        .get_or_load_streaming(path, 44100)
        .await
        .expect("a corrupt packet must not make the whole file unplayable");

    // The fixture corrupts 8 of ~27 frames; with resync losses roughly two
    // thirds of the audio survives. The bar here is "plays with dropouts
    // instead of failing": more than half the 2s file must decode.
    assert!(
        buffer.frames() > 44_100,
        "expected most of the 2s file to decode, got {} frames",
        buffer.frames()
    );
}

#[tokio::test]
async fn test_nonexistent_file_error() {
    // Test error handling for missing files
    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let result = cache_manager
        .get_or_load_streaming("/nonexistent/file/that/does/not/exist.wav", 48000)
        .await;

    assert!(result.is_err(), "Should return error for nonexistent file");
}

#[tokio::test]
async fn http_windowed_producer_buffers_a_remote_file_to_eof() {
    use mqttaudio::audio::streamed_source::spawn_stream_from_source;
    use mqttaudio::cache::http_stream::open_http_stream;
    use std::sync::atomic::Ordering;

    // A remote file is windowed through the production path: a bounded reader (O(channel)
    // compressed bytes, back-pressured) feeds a bounded decoded ring (O(window) samples),
    // then drained to EOF. This is the never-OOM path for big HTTP cues.
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("cue.wav");
    generate_test_wav(&wav_path, 1.0); // 1 second
    let (port, _shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/cue.wav", port);

    // Open headers, then begin the bounded download and run the blocking producer.
    let open = open_http_stream(&url).await.unwrap();
    assert!(
        open.content_length().is_some(),
        "served WAV must advertise a Content-Length"
    );
    let reader = open.into_bounded_reader(None);
    let window_frames = 4800; // 0.1 s ring — far smaller than the file
    let mut handles = tokio::task::spawn_blocking(move || {
        spawn_stream_from_source(reader, url, 48000, ResamplerQuality::Fast, window_frames)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(handles.channels, 2);

    // Drain the bounded ring to EOF on a blocking thread.
    let (total_frames, done) = tokio::task::spawn_blocking(move || {
        let mut scratch = vec![0.0f32; 8192];
        let mut total = 0usize;
        for _ in 0..100_000 {
            let n = handles.consumer.pop_slice(&mut scratch);
            total += n / handles.channels.max(1);
            if handles.producer_done.load(Ordering::Acquire) && handles.consumer.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        (total, handles.producer_done.load(Ordering::Acquire))
    })
    .await
    .unwrap();

    assert!(
        done,
        "remote windowed producer must signal completion at EOF"
    );
    // 1 s resampled 44.1k -> 48k is ~48000 frames; allow resampler-latency slack.
    assert!(
        (44_000..=50_000).contains(&total_frames),
        "expected ~48000 frames from the remote stream, got {total_frames}"
    );
}

#[tokio::test]
async fn cacheable_windowed_play_persists_and_replay_hits_disk() {
    use mqttaudio::cache::http_stream::{open_http_stream, PersistTarget};
    use mqttaudio::cache::CacheManager;
    use std::io::Read as _;
    use std::sync::atomic::Ordering;

    // A WAV served by a request-counting server.
    let wav_bytes = {
        let d = TempDir::new().unwrap();
        let p = d.path().join("src.wav");
        generate_test_wav(&p, 0.5);
        std::fs::read(&p).unwrap()
    };
    let content_length = wav_bytes.len() as u64;
    let (port, count, _shutdown) = start_counting_server(wav_bytes).await;
    let url = format!("http://127.0.0.1:{}/cue.wav", port);

    let cache_dir = TempDir::new().unwrap();
    // A large revalidation window so a freshly-recorded entry never triggers a
    // conditional GET on replay (we are asserting zero extra requests).
    let mut cm = CacheManager::with_options(
        cache_dir.path().to_path_buf(),
        ResamplerQuality::Fast,
        0,
        Vec::new(),
        3600,
    )
    .unwrap();

    // Tee the cacheable windowed download to disk, exactly as the windowed play does.
    let (temp_path, final_path) = cm.windowed_persist_paths(&url).unwrap();
    let open = open_http_stream(&url).await.unwrap();
    assert_eq!(open.content_length(), Some(content_length));
    let content_type = open.content_type().map(|s| s.to_string());
    let (done_tx, done_rx) = oneshot::channel::<u64>();
    let reader = open.into_bounded_reader(Some(PersistTarget {
        temp_path: temp_path.clone(),
        final_path: final_path.clone(),
        done: done_tx,
    }));
    // Draining the raw bytes to EOF lets the download finalize (rename + done signal).
    tokio::task::spawn_blocking(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        while reader.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
    })
    .await
    .unwrap();
    let persisted = done_rx.await.expect("a cacheable download must finalize");
    assert_eq!(persisted, content_length, "the whole file is teed to disk");
    assert!(final_path.exists(), "final cache file must exist");
    assert!(!temp_path.exists(), "temp file must be renamed away");

    // Register it; a replay must then serve from disk with no extra GET.
    cm.record_streamed_download(&url, persisted, None, None, content_type)
        .unwrap();
    assert!(cm.is_cached(&url), "URL must be cached after persisting");
    let gets_after_download = count.load(Ordering::SeqCst);
    assert_eq!(
        gets_after_download, 1,
        "the windowed download is a single GET"
    );

    let buffer = cm.get_or_load_streaming(&url, 48000).await.unwrap();
    // D51: the disk-cached replay decodes progressively; wait for the background
    // decode to produce audio before asserting on it.
    let mut attempts = 0;
    while buffer.frames() == 0 && !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        attempts += 1;
    }
    assert!(
        buffer.frames() > 0,
        "replay must decode audio from the disk file"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        gets_after_download,
        "replay must hit disk with zero extra GETs"
    );
}

// =============================================================================
// Event-driven prebuffer gate (Sprint 12 F3, D53)
// =============================================================================

/// Build a bare StreamHandles for gate tests: no producer thread, the test plays
/// the producer's role through the shared atomics + notify.
fn gate_test_handles() -> mqttaudio::audio::streamed_source::StreamHandles {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
    let (_producer, consumer) = mqttaudio::audio::input::create_ring_buffer(1024);
    mqttaudio::audio::streamed_source::StreamHandles {
        consumer,
        channels: 2,
        producer_done: Arc::new(AtomicBool::new(false)),
        stop_flag: Arc::new(AtomicBool::new(false)),
        frames_buffered: Arc::new(AtomicUsize::new(0)),
        data_notify: Arc::new(tokio::sync::Notify::new()),
    }
}

#[tokio::test(start_paused = true)]
async fn prebuffer_gate_releases_on_threshold_without_poll_quantum() {
    // D53: the gate wakes on the producer's notify the moment the threshold is
    // crossed — under paused time the release instant is EXACTLY the producer's
    // store instant, proving there is no polling quantum left.
    use std::sync::atomic::Ordering;
    let handles = gate_test_handles();
    let fb = handles.frames_buffered.clone();
    let notify = handles.data_notify.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(37)).await;
        fb.store(4800, Ordering::Release);
        notify.notify_waiters();
    });

    let start = tokio::time::Instant::now();
    let filled = handles
        .wait_prebuffer(4800, std::time::Duration::from_millis(200))
        .await;
    assert!(filled, "the gate must report the prebuffer as filled");
    assert_eq!(
        start.elapsed(),
        std::time::Duration::from_millis(37),
        "the gate must release at the producer's instant, not a poll tick"
    );
}

#[tokio::test(start_paused = true)]
async fn prebuffer_gate_starts_anyway_at_the_deadline() {
    // The deadline-start-anyway semantics are preserved exactly: a stalled
    // producer releases the gate at the deadline, reported as unfilled.
    let handles = gate_test_handles();
    let start = tokio::time::Instant::now();
    let filled = handles
        .wait_prebuffer(4800, std::time::Duration::from_millis(200))
        .await;
    assert!(!filled, "a stalled producer must report unfilled");
    assert_eq!(
        start.elapsed(),
        std::time::Duration::from_millis(200),
        "the gate must release exactly at the deadline"
    );
}

#[tokio::test(start_paused = true)]
async fn prebuffer_gate_releases_when_the_producer_finishes_early() {
    // A short file can finish decoding below the prebuffer threshold; there is
    // nothing more to wait for, so the gate releases on producer_done.
    use std::sync::atomic::Ordering;
    let handles = gate_test_handles();
    let done = handles.producer_done.clone();
    let notify = handles.data_notify.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        done.store(true, Ordering::Release);
        notify.notify_waiters();
    });

    let start = tokio::time::Instant::now();
    let filled = handles
        .wait_prebuffer(4800, std::time::Duration::from_millis(200))
        .await;
    assert!(filled, "producer-done must release the gate");
    assert_eq!(start.elapsed(), std::time::Duration::from_millis(12));
}

#[tokio::test]
async fn real_producer_releases_the_gate_through_the_notify() {
    // End-to-end: a real local-file producer must wake the gate (no test doubles
    // on the producer side). Real time, generous deadline; asserts filled=true
    // and that the buffered count actually reached the threshold.
    use std::sync::atomic::Ordering;
    let temp_dir = TempDir::new().unwrap();
    let wav = temp_dir.path().join("gate.wav");
    generate_test_wav(&wav, 2.0);

    let handles = tokio::task::spawn_blocking({
        let path = wav.to_str().unwrap().to_string();
        move || {
            mqttaudio::audio::streamed_source::spawn_local_file_stream(
                path,
                48000,
                ResamplerQuality::Fast,
                48000, // 1s window
                false,
            )
            .unwrap()
        }
    })
    .await
    .unwrap();

    let filled = handles
        .wait_prebuffer(4800, std::time::Duration::from_secs(5))
        .await;
    assert!(filled, "the real producer must fill a 100ms prebuffer");
    assert!(handles.frames_buffered.load(Ordering::Acquire) >= 4800);
    handles.stop_flag.store(true, Ordering::Release);
}

// =============================================================================
// Invalidation abandons in-flight streaming loads (Sprint 12 F2, D52)
// =============================================================================

/// Serve `data` with correct Content-Length but stall after `initial` bytes until
/// `release` is notified — keeps a streaming load reliably in flight while a test
/// invalidates it.
async fn start_stalling_server(
    data: Vec<u8>,
    initial: usize,
) -> (
    u16,
    std::sync::Arc<tokio::sync::Notify>,
    oneshot::Sender<()>,
) {
    use axum::routing::get;
    use axum::Router;

    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let serve_release = release.clone();
    let app = Router::new().route(
        "/file.wav",
        get(move || {
            let data = data.clone();
            let release = serve_release.clone();
            async move {
                let head = axum::body::Bytes::copy_from_slice(&data[..initial]);
                let tail = axum::body::Bytes::copy_from_slice(&data[initial..]);
                let total = data.len();
                let stream = futures::stream::unfold(
                    (Some(head), Some(tail), release),
                    |(head, tail, release)| async move {
                        if let Some(h) = head {
                            return Some((Ok::<_, std::io::Error>(h), (None, tail, release)));
                        }
                        if let Some(t) = tail {
                            release.notified().await;
                            return Some((Ok(t), (None, None, release)));
                        }
                        None
                    },
                );
                axum::response::Response::builder()
                    .header("content-length", total.to_string())
                    .header("content-type", "audio/wav")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });
    (port, release, shutdown_tx)
}

#[tokio::test]
async fn invalidate_abandons_an_in_flight_streaming_load() {
    // D52: `invalidate` (the cache_reload path) must abandon an in-flight
    // streaming load of the same URL — its completion must NOT promote stale
    // content into the freshly invalidated memory cache, and a new play must not
    // join the abandoned stream.
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("stall.wav");
    generate_test_wav(&wav_path, 0.5);
    let data = std::fs::read(&wav_path).unwrap();

    // Stall after 4KB: enough for the header + some audio, far from complete.
    let (port, release, shutdown) = start_stalling_server(data, 4096).await;
    let url = format!("http://127.0.0.1:{}/file.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    let stale = match &buffer {
        SampleBuffer::Streaming(b) => Arc::clone(b),
        SampleBuffer::Complete(_) => panic!("expected an in-flight streaming load"),
    };
    assert!(!buffer.is_complete(), "the stalled load must be in flight");

    // Invalidate while the load is provably in flight.
    cache_manager.invalidate(&url).unwrap();

    // A play arriving after the invalidation must not join the abandoned stream.
    let rejoined = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    match &rejoined {
        SampleBuffer::Streaming(b) => assert!(
            !Arc::ptr_eq(b, &stale),
            "a post-invalidate play must not join the abandoned streaming load"
        ),
        SampleBuffer::Complete(_) => {
            panic!("the second request stalls too; it cannot be complete here")
        }
    }

    // Abandon the second load too, so the decisive assertion below can only be
    // violated by an abandoned load's stale promotion.
    drop(rejoined);
    cache_manager.invalidate(&url).unwrap();

    // Let the abandoned downloads finish, then run the promotion pass.
    release.notify_waiters();
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 100 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(
        buffer.is_complete(),
        "the abandoned load should still finish"
    );
    cache_manager.cleanup_completed_loads();

    assert!(
        !cache_manager.is_in_memory_cache(&url),
        "an abandoned streaming load must not promote stale content after invalidate"
    );

    let _ = shutdown.send(());
}

// =============================================================================
// Progressive cold full-loads (Sprint 12 F1, D51)
// =============================================================================

#[tokio::test]
async fn cold_local_full_load_returns_a_progressive_buffer_immediately() {
    // D51: a cold local full-load returns SampleBuffer::Streaming at once and is
    // playable while the decode fills it — first sound no longer waits for the
    // whole file. The header is parsed synchronously, so channel count and the
    // total-frames estimate are correct immediately (the crossfade-length warning
    // and /status need them).
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("cold_local.wav");
    generate_test_wav(&wav_path, 5.0);

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let buffer = cache_manager
        .get_or_load_streaming(wav_path.to_str().unwrap(), 48000)
        .await
        .unwrap();

    match &buffer {
        SampleBuffer::Streaming(_) => {}
        SampleBuffer::Complete(_) => {
            panic!("a cold local full-load must return a progressive buffer (D51)")
        }
    }
    assert_eq!(
        buffer.channels(),
        2,
        "channels known from the sync header parse"
    );
    assert!(
        buffer.total_frames_or_estimate().is_some(),
        "the total estimate must be available immediately"
    );

    // The decode finishes in the background and the content is intact.
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(buffer.is_complete(), "background decode must finish");
    // 5s at 48k after 44.1k->48k resample.
    let frames = buffer.frames();
    assert!(
        (frames as i64 - 240_000).unsigned_abs() < 4800,
        "decoded length must match the file (~240k frames), got {frames}"
    );
}

#[tokio::test]
async fn stale_promotion_is_skipped_when_the_file_changed_during_the_load() {
    // D51 generation guard: a load whose source file changed underneath it must
    // not publish its (stale) content to the memory cache; the next play decodes
    // the new content fresh.
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("regen.wav");
    generate_test_wav(&wav_path, 1.0);
    let path = wav_path.to_str().unwrap().to_string();

    let cache_dir = TempDir::new().unwrap();
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    let buffer = cache_manager
        .get_or_load_streaming(&path, 48000)
        .await
        .unwrap();
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(buffer.is_complete());

    // The file changes (different length => different size) before the promotion
    // pass runs: the load's generation no longer matches.
    generate_test_wav(&wav_path, 2.0);
    cache_manager.cleanup_completed_loads();
    assert!(
        !cache_manager.is_in_memory_cache(&path),
        "a stale load must not be promoted after the file changed"
    );

    // A fresh play decodes the NEW content.
    let fresh = cache_manager
        .get_or_load_streaming(&path, 48000)
        .await
        .unwrap();
    let mut attempts = 0;
    while !fresh.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    let frames = fresh.frames();
    assert!(
        (frames as i64 - 96_000).unsigned_abs() < 4800,
        "the fresh load must carry the new 2s content (~96k frames), got {frames}"
    );
}

#[tokio::test]
async fn disk_cached_http_full_load_returns_a_progressive_buffer() {
    // D51: a full-load of a disk-cached HTTP URL decodes the cached file
    // progressively instead of blocking on the whole decode.
    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("disk_hit.wav");
    generate_test_wav(&wav_path, 2.0);

    let (port, shutdown) = start_test_server(temp_dir.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/disk_hit.wav", port);

    let cache_dir = TempDir::new().unwrap();
    // Populate the disk cache (download + full decode).
    {
        let mut warm = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
        warm.get_or_load(&url, 48000).await.unwrap();
    }

    // A fresh manager over the same disk cache: memory-cold, disk-warm.
    let mut cache_manager = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
    let buffer = cache_manager
        .get_or_load_streaming(&url, 48000)
        .await
        .unwrap();
    match &buffer {
        SampleBuffer::Streaming(_) => {}
        SampleBuffer::Complete(_) => {
            panic!("a disk-cached HTTP full-load must return a progressive buffer (D51)")
        }
    }

    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(buffer.is_complete());
    assert!(buffer.frames() > 0);
    assert_eq!(buffer.channels(), 2);

    let _ = shutdown.send(());
}

// =============================================================================
// Reused-request HTTP full load (Sprint 12 F5, D55)
// =============================================================================

#[tokio::test]
async fn http_full_load_reuses_the_probe_request_and_persists() {
    // D55: when the windowing probe decides full-load, the already-open response
    // is decoded directly — ONE request total — and a cacheable download is teed
    // to the disk cache so a replay needs no network at all.
    use mqttaudio::cache::http_stream::{open_http_stream, PersistTarget};
    use std::sync::atomic::Ordering;

    let temp_dir = TempDir::new().unwrap();
    let wav_path = temp_dir.path().join("small.wav");
    generate_test_wav(&wav_path, 0.5);
    let body = std::fs::read(&wav_path).unwrap();
    let content_length = body.len() as u64;

    let (port, count, shutdown) = start_counting_server(body).await;
    let url = format!("http://127.0.0.1:{}/cue.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cm = CacheManager::new(cache_dir.path().to_path_buf()).unwrap();

    // The probe open (the only GET).
    let open = open_http_stream(&url).await.unwrap();
    assert_eq!(open.content_length(), Some(content_length));
    let content_type = open.content_type().map(|s| s.to_string());

    // Persist tee, as the play path builds it for a cacheable source.
    let (temp_path, final_path) = cm.windowed_persist_paths(&url).unwrap();
    let (done_tx, done_rx) = oneshot::channel::<u64>();
    let reader = open.into_bounded_reader(Some(PersistTarget {
        temp_path,
        final_path,
        done: done_tx,
    }));

    let estimated = Some((content_length / 4) as usize);
    let buffer = cm.start_streaming_load_from_reader(&url, reader, 48000, estimated);
    let mut attempts = 0;
    while !buffer.is_complete() && attempts < 200 {
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(
        buffer.is_complete(),
        "the reused-request decode must finish"
    );
    assert!(buffer.frames() > 0);
    assert_eq!(buffer.channels(), 2);

    // The completed load promotes to the memory cache like any streaming load.
    cm.cleanup_completed_loads();
    assert!(cm.is_in_memory_cache(&url), "completion must promote");

    // The download finalized to disk; registering it makes the replay disk-warm.
    let persisted = done_rx.await.expect("a cacheable download must finalize");
    assert_eq!(persisted, content_length);
    cm.record_streamed_download(&url, persisted, None, None, content_type)
        .unwrap();
    assert!(cm.is_cached(&url));

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "the full-load play must cost exactly one GET"
    );
    let _ = shutdown.send(());
}
