// ABOUTME: Lane A test that disk-cache revalidation issues a conditional GET.
// ABOUTME: When due, an unchanged file yields 304 and refreshes last_validated.

use mqttaudio::cache::disk::{CacheEntry, DiskCache};
use mqttaudio::cache::{refresh_stale_http, CacheManager};
use mqttaudio::config::ResamplerQuality;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::oneshot;

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
    tokio::time::sleep(Duration::from_millis(50)).await;
    (port, shutdown_tx)
}

#[tokio::test]
async fn revalidation_refreshes_only_when_due() {
    let serve = TempDir::new().unwrap();
    std::fs::write(serve.path().join("x.wav"), b"some audio bytes here").unwrap();
    let (port, shutdown) = start_test_server(serve.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/x.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache = DiskCache::new(cache_dir.path().to_path_buf()).unwrap();

    // Populate the disk cache (captures Last-Modified / ETag from the server).
    cache.download_and_cache(&url).await.unwrap();
    assert!(cache.is_cached(&url));

    // Within the TTL: no revalidation, last_validated unchanged.
    let baseline = cache.get_entry(&url).unwrap().last_validated.clone();
    cache
        .revalidate_if_due(&url, Duration::from_secs(99_999))
        .await
        .unwrap();
    assert_eq!(
        baseline,
        cache.get_entry(&url).unwrap().last_validated,
        "should not revalidate inside the TTL window"
    );

    // Due (TTL 0): the conditional GET runs and refreshes last_validated. The file
    // is unchanged so the cached copy stays valid.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cache
        .revalidate_if_due(&url, Duration::from_secs(0))
        .await
        .unwrap();
    assert_ne!(
        baseline,
        cache.get_entry(&url).unwrap().last_validated,
        "a due revalidation should refresh last_validated"
    );
    assert!(cache.is_cached(&url));

    let _ = shutdown.send(());
}

#[tokio::test(start_paused = true)]
async fn revalidating_against_an_unresponsive_server_is_bounded_and_throttled() {
    // A server that accepts the connection but never answers must not stall the
    // caller (plays and the freshness tick hold the cache lock meanwhile), and a
    // failed check is not retried until the revalidation window passes again.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/x.wav",
        silent.local_addr().unwrap().port()
    );
    let cache_dir = TempDir::new().unwrap();
    let mut cache = DiskCache::new(cache_dir.path().to_path_buf()).unwrap();
    let local_file = DiskCache::cache_filename_for_url(&url);
    std::fs::write(cache.files_dir().join(&local_file), b"cached").unwrap();
    cache.put_entry(
        url.clone(),
        CacheEntry {
            local_file,
            etag: None,
            last_modified: None,
            last_validated: "2020-01-01T00:00:00+00:00".to_string(),
            file_size: 6,
            content_type: None,
        },
    );

    let started = tokio::time::Instant::now();
    let first = tokio::time::timeout(
        Duration::from_secs(60),
        cache.revalidate_if_due(&url, Duration::ZERO),
    )
    .await;
    assert!(
        first.is_ok(),
        "an unresponsive server must not stall revalidation"
    );
    assert!(started.elapsed() <= Duration::from_secs(10));

    let retry_started = tokio::time::Instant::now();
    cache
        .revalidate_if_due(&url, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(
        retry_started.elapsed(),
        Duration::ZERO,
        "a failed check waits for the revalidation window before retrying"
    );
    assert!(cache.is_cached(&url), "the cached copy stays in service");
}

#[tokio::test]
async fn revalidate_if_due_reports_whether_content_changed() {
    let serve = TempDir::new().unwrap();
    let file = serve.path().join("x.wav");
    std::fs::write(&file, b"original audio bytes").unwrap();
    let (port, shutdown) = start_test_server(serve.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/x.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cache = DiskCache::new(cache_dir.path().to_path_buf()).unwrap();
    cache.download_and_cache(&url).await.unwrap();

    // Unchanged + due: the conditional GET yields 304, so nothing was re-downloaded.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let changed = cache
        .revalidate_if_due(&url, Duration::from_secs(0))
        .await
        .unwrap();
    assert!(!changed, "an unchanged file must report not-changed (304)");

    // Republish the file (new content + mtime — sleep past the 1 s Last-Modified
    // granularity): a due revalidation re-downloads and reports changed.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(
        &file,
        b"a totally different and rather longer set of audio bytes",
    )
    .unwrap();
    let changed = cache
        .revalidate_if_due(&url, Duration::from_secs(0))
        .await
        .unwrap();
    assert!(
        changed,
        "a modified file must report changed (re-downloaded)"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn the_freshness_pass_drops_changed_memory_entries() {
    let short = "tests/audio/test_440hz_2s.wav";
    let long = "tests/audio/test_beep_5s.wav";
    if !Path::new(short).exists() || !Path::new(long).exists() {
        eprintln!("skipping: test WAVs not found");
        return;
    }
    let serve = TempDir::new().unwrap();
    let served = serve.path().join("clip.wav");
    std::fs::copy(short, &served).unwrap();
    let (port, shutdown) = start_test_server(serve.path().to_path_buf()).await;
    let url = format!("http://127.0.0.1:{}/clip.wav", port);

    let cache_dir = TempDir::new().unwrap();
    let mut cm =
        CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast).unwrap();

    // Load over HTTP: downloaded to disk, decoded, and resident in memory.
    cm.get_or_load(&url, 48000).await.unwrap();
    assert!(
        cm.is_resident(&url),
        "the HTTP clip should be resident after loading"
    );
    let cm = std::sync::Arc::new(tokio::sync::Mutex::new(cm));

    // Republish a different clip; an out-of-band freshness pass re-downloads it and
    // drops the stale decoded entry so the next play re-decodes the fresh file. Write
    // the bytes (rather than fs::copy, which preserves the source mtime) so the served
    // file's Last-Modified advances and the conditional GET sees a change.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(&served, std::fs::read(long).unwrap()).unwrap();
    let refreshed = refresh_stale_http(&cm, Duration::from_secs(0)).await;
    assert_eq!(refreshed, 1, "the changed entry should be refreshed");
    assert!(
        !cm.lock().await.is_resident(&url),
        "the stale decoded entry must be dropped after a content change"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn the_freshness_pass_does_not_hold_the_cache_while_the_server_answers() {
    // A slow server must not stall plays, cache commands or /status: the pass takes
    // the cache only to pick its checks and to record what they found.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = std::fs::read("tests/audio/test_440hz_2s.wav").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/clip.wav", listener.local_addr().unwrap());
    let (checked_tx, checked_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        // The first request (the load) is answered; the second (the check) is held
        // open without an answer until the test has looked at the cache.
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(&body).await;
        drop(socket);
        let (_held, _) = listener.accept().await.unwrap();
        let _ = checked_rx.await;
    });

    let cache_dir = TempDir::new().unwrap();
    let mut cm =
        CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast).unwrap();
    cm.get_or_load(&url, 48000).await.unwrap();
    let cm = std::sync::Arc::new(tokio::sync::Mutex::new(cm));
    let pass = tokio::spawn({
        let cm = cm.clone();
        async move { refresh_stale_http(&cm, Duration::ZERO).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        cm.try_lock().is_ok(),
        "the cache must stay available while a check waits on the server"
    );
    let _ = checked_tx.send(());
    assert_eq!(
        pass.await.unwrap(),
        0,
        "an unanswered check changes nothing"
    );
    assert!(
        cm.lock().await.is_resident(&url),
        "the cached copy stays in service"
    );
}
