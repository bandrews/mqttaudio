// ABOUTME: Test support utilities for loading benchmarks.
// ABOUTME: Provides synthetic audio generation and embedded HTTP server.

use std::io::Write;
use std::path::PathBuf;
use tokio::sync::oneshot;

/// Generate a WAV file with non-compressible audio content.
/// Uses multiple inharmonic sine waves to defeat audio compression.
pub fn generate_wav_file(path: &std::path::Path, duration_secs: u32, sample_rate: u32, channels: u16) {
    let samples = generate_test_audio(duration_secs as f32, sample_rate, channels as usize);
    write_wav_file(path, &samples, sample_rate, channels);
}

/// Generate non-compressible audio data for benchmarking.
/// Multiple inharmonic sine waves defeat most audio compression.
pub fn generate_test_audio(duration_seconds: f32, sample_rate: u32, channels: usize) -> Vec<f32> {
    let frames = (duration_seconds * sample_rate as f32) as usize;
    let mut data = Vec::with_capacity(frames * channels);

    // Inharmonic frequencies defeat audio compression
    // These are deliberately not musical harmonics
    let frequencies = [
        440.0, 553.0, 697.0, 877.0, 1103.0,
        1388.0, 1746.0, 2198.0, 2767.0, 3480.0,
    ];

    // Different phases for each frequency
    let phases: Vec<f32> = frequencies.iter().enumerate()
        .map(|(i, _)| i as f32 * 0.7)
        .collect();

    for frame in 0..frames {
        let t = frame as f32 / sample_rate as f32;

        // Sum of inharmonic sine waves
        let mut sample: f32 = frequencies.iter().zip(phases.iter())
            .map(|(f, p)| ((t * f * std::f32::consts::TAU + p).sin() * 0.08))
            .sum();

        // Add some pseudo-random noise based on frame position
        // This further defeats compression
        let noise = ((frame as f32 * 12345.6789).sin() * 0.02) as f32;
        sample += noise;

        // Clamp to valid range
        sample = sample.clamp(-1.0, 1.0);

        // Write same sample to all channels
        for _ in 0..channels {
            data.push(sample);
        }
    }

    data
}

/// Write samples to a WAV file
fn write_wav_file(path: &std::path::Path, samples: &[f32], sample_rate: u32, channels: u16) {
    let mut file = std::fs::File::create(path).unwrap();

    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = samples.len() as u32 * 2; // 16-bit samples
    let file_size = 36 + data_size;

    // RIFF header
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();

    // fmt chunk
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap(); // chunk size
    file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM format
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bits_per_sample.to_le_bytes()).unwrap();

    // data chunk
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();

    // Write samples as 16-bit PCM
    for &sample in samples {
        let i16_sample = (sample * 32767.0) as i16;
        file.write_all(&i16_sample.to_le_bytes()).unwrap();
    }
}

/// Embedded HTTP server for benchmarking HTTP loading
pub struct TestHttpServer {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TestHttpServer {
    /// Start the test HTTP server serving files from the given directory
    pub async fn start(serve_dir: PathBuf) -> Self {
        use axum::Router;
        use tower_http::services::ServeDir;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Find available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Build router with static file serving
        let app = Router::new()
            .nest_service("/", ServeDir::new(serve_dir));

        // Spawn server task
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

        Self {
            port,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Get the base URL for this server
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Shutdown the server
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Give server time to shutdown
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_test_audio() {
        let samples = generate_test_audio(1.0, 48000, 2);
        assert_eq!(samples.len(), 48000 * 2);

        // Verify samples are in valid range
        for &s in &samples {
            assert!(s >= -1.0 && s <= 1.0);
        }
    }

    #[test]
    fn test_generate_wav_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.wav");

        generate_wav_file(&path, 1, 48000, 2);

        assert!(path.exists());
        let metadata = std::fs::metadata(&path).unwrap();
        // 1 second * 48000 Hz * 2 channels * 2 bytes + 44 bytes header
        let expected_size = 48000 * 2 * 2 + 44;
        assert_eq!(metadata.len(), expected_size as u64);
    }

    #[tokio::test]
    async fn test_http_server() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap();

        let server = TestHttpServer::start(temp_dir.path().to_path_buf()).await;

        let url = format!("{}/test.txt", server.base_url());
        let response = reqwest::get(&url).await.unwrap();
        assert!(response.status().is_success());

        let body = response.text().await.unwrap();
        assert_eq!(body, "hello world");

        server.shutdown().await;
    }
}
