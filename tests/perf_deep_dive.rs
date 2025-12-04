// ABOUTME: Deep dive into performance bottlenecks.
// ABOUTME: Run with: cargo test --release --test perf_deep_dive -- --nocapture

use std::io::Write;
use std::time::Instant;
use tempfile::TempDir;
use std::process::Command;

/// Generate a WAV file
fn generate_wav_file(path: &std::path::Path, duration_secs: u32, sample_rate: u32, channels: u16) {
    let frames = (duration_secs * sample_rate) as usize;
    let mut data = Vec::with_capacity(frames * channels as usize);

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

/// Convert WAV to MP3 using ffmpeg (if available)
fn convert_to_mp3(wav_path: &std::path::Path, mp3_path: &std::path::Path) -> bool {
    Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(wav_path)
        .args(["-codec:a", "libmp3lame", "-b:a", "192k"])
        .arg(mp3_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Convert WAV to OGG using ffmpeg (if available)
fn convert_to_ogg(wav_path: &std::path::Path, ogg_path: &std::path::Path) -> bool {
    Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(wav_path)
        .args(["-codec:a", "libvorbis", "-q:a", "6"])
        .arg(ogg_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn explore_decode_overhead() {
    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    println!("\n=== Decode Overhead: WAV vs Compressed ===\n");

    let duration = 60; // 1 minute test file

    // Generate WAV at 48kHz (no resample needed)
    let wav_path = temp_dir.path().join("test.wav");
    generate_wav_file(&wav_path, duration, 48000, 2);

    let mp3_path = temp_dir.path().join("test.mp3");
    let ogg_path = temp_dir.path().join("test.ogg");

    let has_ffmpeg = convert_to_mp3(&wav_path, &mp3_path);
    if has_ffmpeg {
        convert_to_ogg(&wav_path, &ogg_path);
    }

    // Test WAV decode (no resample)
    {
        let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
        let _ = cache_mgr.clear_all();

        let start = Instant::now();
        let _ = cache_mgr.get_or_load(wav_path.to_str().unwrap(), 48000).await.unwrap();
        let elapsed = start.elapsed();

        println!("WAV decode (48kHz, no resample): {:>8.2}ms", elapsed.as_secs_f64() * 1000.0);
    }

    if has_ffmpeg {
        // Test MP3 decode (no resample - MP3 is 48kHz from ffmpeg default)
        {
            let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
            let _ = cache_mgr.clear_all();

            let start = Instant::now();
            let _ = cache_mgr.get_or_load(mp3_path.to_str().unwrap(), 48000).await.unwrap();
            let elapsed = start.elapsed();

            println!("MP3 decode (48kHz, no resample): {:>8.2}ms", elapsed.as_secs_f64() * 1000.0);
        }

        // Test OGG decode (no resample)
        {
            let mut cache_mgr = mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap();
            let _ = cache_mgr.clear_all();

            let start = Instant::now();
            let _ = cache_mgr.get_or_load(ogg_path.to_str().unwrap(), 48000).await.unwrap();
            let elapsed = start.elapsed();

            println!("OGG decode (48kHz, no resample): {:>8.2}ms", elapsed.as_secs_f64() * 1000.0);
        }
    } else {
        println!("(ffmpeg not available - skipping MP3/OGG tests)");
    }

    println!();
}

#[tokio::test]
async fn explore_resample_breakdown() {
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

    println!("\n=== Resampling Breakdown ===\n");

    let duration_secs = 60;
    let sample_rate = 44100;
    let channels = 2;
    let frames = duration_secs * sample_rate;
    let total_samples = frames * channels;

    // Generate test data
    let input: Vec<f32> = (0..total_samples)
        .map(|i| ((i as f32 * 0.01).sin() * 0.5))
        .collect();

    println!("Input: {} frames, {} channels, {}s @ {}Hz\n", frames, channels, duration_secs, sample_rate);

    // Test different quality settings
    let configs = [
        ("Current (sinc=256, oversample=256)", 256, 256),
        ("High (sinc=256, oversample=128)", 256, 128),
        ("Medium (sinc=128, oversample=128)", 128, 128),
        ("Fast (sinc=64, oversample=64)", 64, 64),
        ("Fastest (sinc=32, oversample=32)", 32, 32),
    ];

    for (name, sinc_len, oversample) in configs {
        let ratio = 48000.0 / 44100.0;

        let params = SincInterpolationParameters {
            sinc_len,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: oversample,
            window: WindowFunction::BlackmanHarris2,
        };

        // Time the full operation including de-interleave
        let start = Instant::now();

        // De-interleave
        let mut input_channels: Vec<Vec<f32>> = vec![vec![0.0; frames as usize]; channels as usize];
        for (frame_idx, frame) in input.chunks(channels as usize).enumerate() {
            for (ch_idx, &sample) in frame.iter().enumerate() {
                input_channels[ch_idx][frame_idx] = sample;
            }
        }
        let deinterleave_time = start.elapsed();

        // Create resampler
        let resample_start = Instant::now();
        let mut resampler = SincFixedIn::<f32>::new(
            ratio,
            2.0,
            params,
            frames as usize,
            channels as usize,
        ).unwrap();

        // Resample
        let output_channels = resampler.process(&input_channels, None).unwrap();
        let resample_time = resample_start.elapsed();

        // Re-interleave
        let reinterleave_start = Instant::now();
        let output_frames = output_channels[0].len();
        let mut output = Vec::with_capacity(output_frames * channels as usize);
        for frame_idx in 0..output_frames {
            for ch_idx in 0..channels as usize {
                output.push(output_channels[ch_idx][frame_idx]);
            }
        }
        let reinterleave_time = reinterleave_start.elapsed();

        let total_time = start.elapsed();

        println!("{}", name);
        println!("  De-interleave:  {:>6.2}ms", deinterleave_time.as_secs_f64() * 1000.0);
        println!("  Resample:       {:>6.2}ms", resample_time.as_secs_f64() * 1000.0);
        println!("  Re-interleave:  {:>6.2}ms", reinterleave_time.as_secs_f64() * 1000.0);
        println!("  TOTAL:          {:>6.2}ms", total_time.as_secs_f64() * 1000.0);
        println!();
    }

    println!("=== Conclusion ===");
    println!("The 'oversample' factor has a huge impact on performance.");
    println!("Consider reducing from 256 to 64-128 for a major speedup.");
}

#[test]
fn measure_interleave_overhead() {
    println!("\n=== Interleave/De-interleave Overhead ===\n");

    let frames = 60 * 48000; // 1 minute at 48kHz
    let channels = 2;

    let input: Vec<f32> = (0..frames * channels)
        .map(|i| (i as f32 * 0.001).sin())
        .collect();

    // Measure de-interleave
    let start = Instant::now();
    let mut deinterleaved: Vec<Vec<f32>> = vec![vec![0.0; frames]; channels];
    for (frame_idx, frame) in input.chunks(channels).enumerate() {
        for (ch_idx, &sample) in frame.iter().enumerate() {
            deinterleaved[ch_idx][frame_idx] = sample;
        }
    }
    let deinterleave_time = start.elapsed();

    // Measure re-interleave
    let start = Instant::now();
    let mut output = Vec::with_capacity(frames * channels);
    for frame_idx in 0..frames {
        for ch_idx in 0..channels {
            output.push(deinterleaved[ch_idx][frame_idx]);
        }
    }
    let reinterleave_time = start.elapsed();

    println!("For {}s stereo audio:", frames / 48000);
    println!("  De-interleave: {:.2}ms", deinterleave_time.as_secs_f64() * 1000.0);
    println!("  Re-interleave: {:.2}ms", reinterleave_time.as_secs_f64() * 1000.0);
    println!("  Total overhead: {:.2}ms", (deinterleave_time + reinterleave_time).as_secs_f64() * 1000.0);
}
