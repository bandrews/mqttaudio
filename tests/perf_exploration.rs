// ABOUTME: Quick performance exploration tests.
// ABOUTME: Run with: cargo test --release --test perf_exploration -- --nocapture

use std::io::Write;
use std::time::Instant;
use tempfile::TempDir;

/// Generate a WAV file with non-compressible audio content.
fn generate_wav_file(path: &std::path::Path, duration_secs: u32, sample_rate: u32, channels: u16) {
    let frames = (duration_secs * sample_rate) as usize;
    let mut data = Vec::with_capacity(frames * channels as usize);

    // Multiple inharmonic sine waves
    let frequencies = [440.0, 553.0, 697.0, 877.0, 1103.0];
    let phases: Vec<f32> = frequencies.iter().enumerate()
        .map(|(i, _)| i as f32 * 0.7)
        .collect();

    for frame in 0..frames {
        let t = frame as f32 / sample_rate as f32;
        let mut sample: f32 = frequencies.iter().zip(phases.iter())
            .map(|(f, p)| ((t * f * std::f32::consts::TAU + p).sin() * 0.15))
            .sum();
        sample = sample.clamp(-1.0, 1.0);
        for _ in 0..channels {
            data.push(sample);
        }
    }

    // Write WAV
    let mut file = std::fs::File::create(path).unwrap();
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = data.len() as u32 * 2;
    let file_size = 36 + data_size;

    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();
    for &sample in &data {
        let i16_sample = (sample * 32767.0) as i16;
        file.write_all(&i16_sample.to_le_bytes()).unwrap();
    }
}

#[tokio::test]
async fn explore_load_times() {
    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    println!("\n=== Performance Exploration ===\n");

    // Test durations
    let durations = [5, 30, 60, 300]; // 5s, 30s, 1min, 5min

    for &duration in &durations {
        let file_path_48k = temp_dir.path().join(format!("test_48k_{}s.wav", duration));
        let file_path_44k = temp_dir.path().join(format!("test_44k_{}s.wav", duration));

        // Generate test files
        println!("Generating {}s test files...", duration);
        generate_wav_file(&file_path_48k, duration, 48000, 2);
        generate_wav_file(&file_path_44k, duration, 44100, 2);

        let file_size_mb = std::fs::metadata(&file_path_48k).unwrap().len() as f64 / (1024.0 * 1024.0);

        // Test without resampling (48kHz -> 48kHz)
        {
            let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
            let _ = cache_mgr.clear_all();

            let start = Instant::now();
            let buffer = cache_mgr.get_or_load(file_path_48k.to_str().unwrap(), 48000).await.unwrap();
            let elapsed = start.elapsed();

            println!(
                "{}s @ 48kHz (no resample): {:>8.2}ms | {:.1}MB | {} frames",
                duration,
                elapsed.as_secs_f64() * 1000.0,
                file_size_mb,
                buffer.frames
            );
        }

        // Test with resampling (44.1kHz -> 48kHz)
        {
            let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
            let _ = cache_mgr.clear_all();

            let start = Instant::now();
            let buffer = cache_mgr.get_or_load(file_path_44k.to_str().unwrap(), 48000).await.unwrap();
            let elapsed = start.elapsed();

            println!(
                "{}s @ 44.1kHz (resample):  {:>8.2}ms | {:.1}MB | {} frames",
                duration,
                elapsed.as_secs_f64() * 1000.0,
                file_size_mb,
                buffer.frames
            );
        }

        // Test hot load (cache hit)
        {
            let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
            // Pre-load
            let _ = cache_mgr.get_or_load(file_path_48k.to_str().unwrap(), 48000).await;

            let start = Instant::now();
            let _ = cache_mgr.get_or_load(file_path_48k.to_str().unwrap(), 48000).await.unwrap();
            let elapsed = start.elapsed();

            println!(
                "{}s @ 48kHz (cache hit):   {:>8.2}ms",
                duration,
                elapsed.as_secs_f64() * 1000.0
            );
        }

        println!();
    }

    // Memory usage test
    println!("=== Memory Usage Test ===\n");
    {
        let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
        let _ = cache_mgr.clear_all();

        for &duration in &[60, 120, 180] {
            let file_path = temp_dir.path().join(format!("mem_{}s.wav", duration));
            generate_wav_file(&file_path, duration, 48000, 2);

            let _ = cache_mgr.get_or_load(file_path.to_str().unwrap(), 48000).await;

            let stats = cache_mgr.memory_stats();
            let mb = stats.size_bytes as f64 / (1024.0 * 1024.0);
            println!(
                "After loading {}s file: {:.2} MB in {} entries",
                duration, mb, stats.entry_count
            );
        }
    }

    println!("\n=== Summary ===");
    println!("Target: <100ms cold start for immersive game triggers");
    println!("Current: See timing above - this is what we need to improve");
}
