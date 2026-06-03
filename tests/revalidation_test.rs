// ABOUTME: Lane A test that disk-cache revalidation issues a conditional GET.
// ABOUTME: When due, an unchanged file yields 304 and refreshes last_validated.

use mqttaudio::cache::disk::DiskCache;
use std::path::PathBuf;
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
