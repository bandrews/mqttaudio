// ABOUTME: Lane A test that disk-cache revalidation issues a conditional GET.
// ABOUTME: When due, an unchanged file yields 304 and refreshes last_validated.

use mqttaudio::cache::disk::DiskCache;
use mqttaudio::cache::CacheManager;
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
async fn revalidate_stale_http_drops_changed_memory_entries() {
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

    // Republish a different clip; an out-of-band freshness pass re-downloads it and
    // drops the stale decoded entry so the next play re-decodes the fresh file. Write
    // the bytes (rather than fs::copy, which preserves the source mtime) so the served
    // file's Last-Modified advances and the conditional GET sees a change.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(&served, std::fs::read(long).unwrap()).unwrap();
    let refreshed = cm.revalidate_stale_http(Duration::from_secs(0)).await;
    assert_eq!(refreshed, 1, "the changed entry should be refreshed");
    assert!(
        !cm.is_resident(&url),
        "the stale decoded entry must be dropped after a content change"
    );

    let _ = shutdown.send(());
}
