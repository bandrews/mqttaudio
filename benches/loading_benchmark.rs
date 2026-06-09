// ABOUTME: Performance benchmarks for audio loading and caching.
// ABOUTME: Measures cold/hot load times, HTTP streaming, and memory usage.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::{Duration, Instant};
use tempfile::TempDir;

mod test_support;
use test_support::{generate_wav_file, TestHttpServer};

/// Benchmark cold loading from local filesystem (no cache)
fn bench_cold_load_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_load_file");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Test various file durations
    for duration_secs in [5, 30, 60, 300] {
        let file_path = temp_dir.path().join(format!("test_{}s.wav", duration_secs));
        generate_wav_file(&file_path, duration_secs, 48000, 2);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}s", duration_secs)),
            &duration_secs,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        // Create fresh cache manager each iteration
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        let mut cache_mgr = rt.block_on(async {
                            mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                        });

                        // Clear caches
                        let _ = cache_mgr.clear_all();

                        let start = Instant::now();
                        rt.block_on(async {
                            cache_mgr
                                .get_or_load(file_path.to_str().unwrap(), 48000)
                                .await
                                .unwrap()
                        });
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }

    group.finish();
}

/// Benchmark hot loading (memory cache hit)
fn bench_hot_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("hot_load");

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Generate test file
    let file_path = temp_dir.path().join("test_hot.wav");
    generate_wav_file(&file_path, 60, 48000, 2); // 1 minute

    // Pre-load into cache
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut cache_mgr =
        rt.block_on(async { mqttaudio::cache::CacheManager::new(cache_dir).unwrap() });

    rt.block_on(async {
        cache_mgr
            .get_or_load(file_path.to_str().unwrap(), 48000)
            .await
            .unwrap()
    });

    group.bench_function("60s_cached", |b| {
        b.iter(|| {
            rt.block_on(async {
                cache_mgr
                    .get_or_load(file_path.to_str().unwrap(), 48000)
                    .await
                    .unwrap()
            })
        });
    });

    group.finish();
}

/// Benchmark HTTP loading with embedded server
fn bench_cold_load_http(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_load_http");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Generate test file
    let file_path = temp_dir.path().join("test_http.wav");
    generate_wav_file(&file_path, 30, 48000, 2); // 30 seconds

    // Start embedded HTTP server
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async { TestHttpServer::start(temp_dir.path().to_path_buf()).await });

    let url = format!("{}/test_http.wav", server.base_url());

    group.bench_function("30s_http", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let mut cache_mgr = rt.block_on(async {
                    mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                });
                let _ = cache_mgr.clear_all();

                let start = Instant::now();
                rt.block_on(async { cache_mgr.get_or_load(&url, 48000).await.unwrap() });
                total += start.elapsed();
            }
            total
        });
    });

    group.finish();

    rt.block_on(async { server.shutdown().await });
}

/// Benchmark precache timing
fn bench_precache(c: &mut Criterion) {
    let mut group = c.benchmark_group("precache");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Test various durations
    for duration_secs in [60, 300, 900] {
        // 1min, 5min, 15min
        let file_path = temp_dir
            .path()
            .join(format!("precache_{}s.wav", duration_secs));
        generate_wav_file(&file_path, duration_secs, 48000, 2);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}s", duration_secs)),
            &duration_secs,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        let mut cache_mgr = rt.block_on(async {
                            mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                        });
                        let _ = cache_mgr.clear_all();

                        let start = Instant::now();
                        rt.block_on(async {
                            cache_mgr
                                .precache(file_path.to_str().unwrap(), 48000)
                                .await
                                .unwrap()
                        });
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }

    group.finish();
}

/// Measure time to first sample (critical latency metric)
fn bench_time_to_first_sample(c: &mut Criterion) {
    let mut group = c.benchmark_group("time_to_first_sample");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Generate a large file (5 minutes)
    let file_path = temp_dir.path().join("first_sample_test.wav");
    generate_wav_file(&file_path, 300, 48000, 2);

    group.bench_function("300s_file_cold", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let mut cache_mgr = rt.block_on(async {
                    mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                });
                let _ = cache_mgr.clear_all();

                let start = Instant::now();
                let buffer = rt.block_on(async {
                    cache_mgr
                        .get_or_load(file_path.to_str().unwrap(), 48000)
                        .await
                        .unwrap()
                });
                // The full-load path decodes the entire file before the first sample;
                // its cost grows with the file's length. The windowed path instead
                // starts after a small prebuffer regardless of length — measured and
                // bounded by the windowed_time_to_first_sample_is_low_and_precedes_full_decode
                // test in src/audio/streamed_source.rs.
                total += start.elapsed();

                // Verify we got data
                assert!(buffer.frames > 0);
            }
            total
        });
    });

    group.finish();
}

/// Benchmark the local-file header probe used by the auto-windowing decision
/// (Sprint 11 baseline; Sprint 12 caches its result keyed by path+mtime+size).
fn bench_probe_local_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("probe_local_file");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("probe_60s.wav");
    generate_wav_file(&file_path, 60, 48000, 2);
    let path = file_path.to_str().unwrap().to_string();

    group.bench_function("60s_wav_header", |b| {
        b.iter(|| {
            mqttaudio::cache::strategy::probe_local_file(
                &path,
                48000,
                mqttaudio::config::ResamplerQuality::Fast,
            )
            .unwrap()
        });
    });

    group.finish();
}

/// Benchmark a windowed play's prebuffer-ready time: producer spawn + ring fill to
/// the default 100ms prebuffer, polled at the control loop's 5ms quantum (Sprint 11
/// baseline; Sprint 12 replaces the poll with an event-driven gate).
fn bench_windowed_prebuffer_ready(c: &mut Criterion) {
    use std::sync::atomic::Ordering;

    let mut group = c.benchmark_group("windowed_prebuffer_ready");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(10));

    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("windowed_300s.wav");
    generate_wav_file(&file_path, 300, 48000, 2);
    let path = file_path.to_str().unwrap().to_string();

    const PREBUFFER_FRAMES: usize = 48000 / 10; // 100ms at 48k (config default)
    const WINDOW_FRAMES: usize = 48000 / 2; // 500ms window (config default)

    group.bench_function("300s_wav_100ms_prebuffer", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let handles = mqttaudio::audio::streamed_source::spawn_local_file_stream(
                    path.clone(),
                    48000,
                    mqttaudio::config::ResamplerQuality::Fast,
                    WINDOW_FRAMES,
                    false,
                )
                .unwrap();
                // The control loop's gate: poll frames_buffered every 5ms.
                while handles.frames_buffered.load(Ordering::Acquire) < PREBUFFER_FRAMES
                    && !handles.producer_done.load(Ordering::Acquire)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                total += start.elapsed();
                handles.stop_flag.store(true, Ordering::Release);
            }
            total
        });
    });

    group.finish();
}

/// Memory usage tracking
fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");
    group.sample_size(10);

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Generate multiple files
    let mut file_paths = Vec::new();
    for i in 0..5 {
        let file_path = temp_dir.path().join(format!("mem_test_{}.wav", i));
        generate_wav_file(&file_path, 180, 48000, 2); // 3 minutes each
        file_paths.push(file_path);
    }

    group.bench_function("5x_180s_files", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let mut cache_mgr = rt.block_on(async {
                    mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                });
                let _ = cache_mgr.clear_all();

                let start = Instant::now();
                for path in &file_paths {
                    rt.block_on(async {
                        cache_mgr
                            .get_or_load(path.to_str().unwrap(), 48000)
                            .await
                            .unwrap()
                    });
                }
                total += start.elapsed();

                // Report memory usage
                let stats = cache_mgr.memory_stats();
                let mb = stats.size_bytes as f64 / (1024.0 * 1024.0);
                eprintln!(
                    "Memory usage: {:.2} MB for {} entries",
                    mb, stats.entry_count
                );
            }
            total
        });
    });

    group.finish();
}

/// Benchmark resampling overhead (44.1kHz -> 48kHz)
fn bench_resampling(c: &mut Criterion) {
    let mut group = c.benchmark_group("resampling");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_secs(5));

    let temp_dir = TempDir::new().unwrap();
    let cache_dir = temp_dir.path().join("cache");

    // Test with files at 44100Hz (common for MP3s)
    for duration_secs in [30, 60, 300] {
        let file_path = temp_dir
            .path()
            .join(format!("resample_{}s.wav", duration_secs));
        // Generate at 44100 Hz - will require resampling to 48000 Hz
        generate_wav_file(&file_path, duration_secs, 44100, 2);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}s_44k_to_48k", duration_secs)),
            &duration_secs,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        let mut cache_mgr = rt.block_on(async {
                            mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                        });
                        let _ = cache_mgr.clear_all();

                        let start = Instant::now();
                        rt.block_on(async {
                            // Target 48000 Hz - forces resampling
                            cache_mgr
                                .get_or_load(file_path.to_str().unwrap(), 48000)
                                .await
                                .unwrap()
                        });
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }

    // Also test no-resample baseline for comparison
    for duration_secs in [30, 60, 300] {
        let file_path = temp_dir
            .path()
            .join(format!("no_resample_{}s.wav", duration_secs));
        // Generate at 48000 Hz - no resampling needed
        generate_wav_file(&file_path, duration_secs, 48000, 2);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}s_no_resample", duration_secs)),
            &duration_secs,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        let mut cache_mgr = rt.block_on(async {
                            mqttaudio::cache::CacheManager::new(cache_dir.clone()).unwrap()
                        });
                        let _ = cache_mgr.clear_all();

                        let start = Instant::now();
                        rt.block_on(async {
                            cache_mgr
                                .get_or_load(file_path.to_str().unwrap(), 48000)
                                .await
                                .unwrap()
                        });
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_cold_load_file,
    bench_hot_load,
    bench_cold_load_http,
    bench_precache,
    bench_time_to_first_sample,
    bench_probe_local_file,
    bench_windowed_prebuffer_ready,
    bench_memory_usage,
    bench_resampling,
);

criterion_main!(benches);
