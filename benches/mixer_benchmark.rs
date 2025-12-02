// ABOUTME: Performance benchmarks for audio mixer operations.
// ABOUTME: Validates that critical audio callback code meets timing requirements.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use mqttaudio::audio::mixer::{ActiveSample, FadeState, MixerState, mix_audio};
use mqttaudio::audio::types::DecodedBuffer;
use std::sync::Arc;

/// Create a test audio buffer with given properties
fn create_test_buffer(channels: usize, frames: usize) -> Arc<DecodedBuffer> {
    let data_len = channels * frames;
    let mut data = Vec::with_capacity(data_len);

    // Fill with sine wave-ish test data
    for i in 0..data_len {
        data.push((i as f32 * 0.01).sin() * 0.5);
    }

    Arc::new(DecodedBuffer {
        data,
        channels,
        sample_rate: 48000,
        frames,
    })
}

/// Create a mixer state with N active samples
fn create_mixer_state(num_samples: usize, output_channels: usize) -> MixerState {
    let mut state = MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels,
        ducking_engine: None,
        bass_management: None,
    };

    let buffer = create_test_buffer(2, 48000); // 1 second of stereo audio

    for i in 0..num_samples {
        let sample = ActiveSample::new(
            i as u64,
            format!("voice_{}", i),
            buffer.clone(),
            0.8,
            1.0,
        );
        state.active_samples.push(sample);
    }

    state
}

/// Create a mixer state with specific channel routing
fn create_mixer_state_with_routing(
    num_samples: usize,
    src_channels: usize,
    dest_channels: usize,
    channel_map: Vec<(usize, usize)>,
) -> MixerState {
    let mut state = MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels: dest_channels,
        ducking_engine: None,
        bass_management: None,
    };

    let buffer = create_test_buffer(src_channels, 48000);

    for i in 0..num_samples {
        let sample = ActiveSample::new_with_mapping(
            i as u64,
            format!("voice_{}", i),
            buffer.clone(),
            0.8,
            1.0,
            channel_map.clone(),
        );
        state.active_samples.push(sample);
    }

    state
}

/// Benchmark: Mix single sample (baseline)
fn bench_mix_single_sample(c: &mut Criterion) {
    let mut state = create_mixer_state(1, 2);
    let mut output = vec![0.0f32; 512 * 2]; // 512 frames, stereo

    c.bench_function("mix_1_sample_512_frames_stereo", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0; // Reset position
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });
}

/// Benchmark: Mix multiple samples (scaling test)
fn bench_mix_multiple_samples(c: &mut Criterion) {
    let mut group = c.benchmark_group("mix_multiple_samples");

    for num_samples in [1, 5, 10, 20].iter() {
        let mut state = create_mixer_state(*num_samples, 2);
        let mut output = vec![0.0f32; 512 * 2];

        group.bench_with_input(
            BenchmarkId::from_parameter(num_samples),
            num_samples,
            |b, _| {
                b.iter(|| {
                    // Reset positions
                    for sample in &mut state.active_samples {
                        sample.position = 0;
                    }
                    mix_audio(black_box(&mut output), black_box(&mut state));
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: Channel mapping performance
fn bench_channel_mapping(c: &mut Criterion) {
    let mut group = c.benchmark_group("channel_mapping");

    // Test stereo to mono (downmix)
    let mut state = create_mixer_state_with_routing(1, 2, 1, vec![(0, 0), (1, 0)]);
    let mut output = vec![0.0f32; 512 * 1];
    group.bench_function("stereo_to_mono", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    // Test stereo to 8-channel (sparse routing)
    let mut state = create_mixer_state_with_routing(
        1,
        2,
        8,
        vec![(0, 2), (1, 5)], // L→ch2, R→ch5
    );
    let mut output = vec![0.0f32; 512 * 8];
    group.bench_function("stereo_to_8ch_sparse", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    // Test quad to 8-channel (complex routing)
    let mut state = create_mixer_state_with_routing(
        1,
        4,
        8,
        vec![(0, 0), (1, 1), (2, 6), (3, 7)],
    );
    let mut output = vec![0.0f32; 512 * 8];
    group.bench_function("quad_to_8ch", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    group.finish();
}

/// Benchmark: Fade envelope performance
fn bench_fading(c: &mut Criterion) {
    let mut group = c.benchmark_group("fading");

    // Fade in
    let mut state = create_mixer_state(1, 2);
    state.active_samples[0].fade_state = FadeState::fade_in(2000, 48000); // 2 second fade
    let mut output = vec![0.0f32; 512 * 2];
    group.bench_function("fade_in", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            state.active_samples[0].fade_state = FadeState::fade_in(2000, 48000);
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    // Fade out
    let mut state = create_mixer_state(1, 2);
    state.active_samples[0].fade_state = FadeState::fade_out(2000, 48000);
    let mut output = vec![0.0f32; 512 * 2];
    group.bench_function("fade_out", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            state.active_samples[0].fade_state = FadeState::fade_out(2000, 48000);
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    // No fade (baseline)
    let mut state = create_mixer_state(1, 2);
    let mut output = vec![0.0f32; 512 * 2];
    group.bench_function("no_fade", |b| {
        b.iter(|| {
            state.active_samples[0].position = 0;
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });

    group.finish();
}

/// Benchmark: High channel count scenarios
fn bench_multichannel(c: &mut Criterion) {
    let mut group = c.benchmark_group("multichannel_output");

    for channels in [2, 8, 16].iter() {
        let mut state = create_mixer_state(5, *channels);
        let mut output = vec![0.0f32; 512 * channels];

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}ch_5_samples", channels)),
            channels,
            |b, _| {
                b.iter(|| {
                    for sample in &mut state.active_samples {
                        sample.position = 0;
                    }
                    mix_audio(black_box(&mut output), black_box(&mut state));
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: Buffer sizes (latency vs performance tradeoff)
fn bench_buffer_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_sizes");

    for frames in [128, 256, 512, 1024].iter() {
        let mut state = create_mixer_state(10, 2);
        let mut output = vec![0.0f32; frames * 2];

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_frames", frames)),
            frames,
            |b, _| {
                b.iter(|| {
                    for sample in &mut state.active_samples {
                        sample.position = 0;
                    }
                    mix_audio(black_box(&mut output), black_box(&mut state));
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: Realistic scenario - background music + effects
fn bench_realistic_scenario(c: &mut Criterion) {
    // 2 looping background tracks + 5 one-shot effects
    let mut state = create_mixer_state(7, 8);

    // Background tracks have lower volume
    state.active_samples[0].volume = 0.3;
    state.active_samples[1].volume = 0.3;

    // Effects at full volume
    for i in 2..7 {
        state.active_samples[i].volume = 1.0;
    }

    let mut output = vec![0.0f32; 512 * 8];

    c.bench_function("realistic_7_samples_8ch", |b| {
        b.iter(|| {
            for sample in &mut state.active_samples {
                sample.position = 0;
            }
            mix_audio(black_box(&mut output), black_box(&mut state));
        })
    });
}

criterion_group!(
    benches,
    bench_mix_single_sample,
    bench_mix_multiple_samples,
    bench_channel_mapping,
    bench_fading,
    bench_multichannel,
    bench_buffer_sizes,
    bench_realistic_scenario,
);

criterion_main!(benches);
