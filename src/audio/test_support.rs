// ABOUTME: Offline render harness and signal-analysis helpers for audio tests.
// ABOUTME: Drives mix_audio deterministically and measures the resulting buffer.

use crate::audio::bass_management::BassManagement;
use crate::audio::ducking::DuckingEngine;
use crate::audio::mixer::{mix_audio, ActiveSample, LiveInput, MixerState};
use crate::audio::types::DecodedBuffer;
use std::sync::Arc;

/// Wrap interleaved PCM in a `Complete` buffer for use in a scene.
pub fn decoded(data: Vec<f32>, channels: usize, sample_rate: u32) -> Arc<DecodedBuffer> {
    Arc::new(DecodedBuffer::new(data, channels, sample_rate))
}

/// Generate an interleaved sine tone `frames` long with every channel identical.
pub fn sine(
    freq: f32,
    sample_rate: u32,
    frames: usize,
    channels: usize,
    amplitude: f32,
) -> Vec<f32> {
    let mut data = Vec::with_capacity(frames * channels);
    for n in 0..frames {
        let t = n as f32 / sample_rate as f32;
        let s = (t * freq * std::f32::consts::TAU).sin() * amplitude;
        for _ in 0..channels {
            data.push(s);
        }
    }
    data
}

/// Builder for a `MixerState` scene with samples, live inputs, ducking, and
/// bass management wired up. Lets a test describe a mix and then render it.
pub struct SceneBuilder {
    output_channels: usize,
    samples: Vec<ActiveSample>,
    live_inputs: Vec<LiveInput>,
    ducking_engine: Option<DuckingEngine>,
    bass_management: Option<BassManagement>,
}

impl SceneBuilder {
    pub fn new(output_channels: usize) -> Self {
        Self {
            output_channels,
            samples: Vec::new(),
            live_inputs: Vec::new(),
            ducking_engine: None,
            bass_management: None,
        }
    }

    pub fn sample(mut self, sample: ActiveSample) -> Self {
        self.samples.push(sample);
        self
    }

    pub fn live_input(mut self, input: LiveInput) -> Self {
        self.live_inputs.push(input);
        self
    }

    pub fn ducking(mut self, engine: DuckingEngine) -> Self {
        self.ducking_engine = Some(engine);
        self
    }

    pub fn bass_management(mut self, bm: BassManagement) -> Self {
        self.bass_management = Some(bm);
        self
    }

    pub fn build(self) -> MixerState {
        MixerState {
            active_samples: self.samples,
            live_inputs: self.live_inputs,
            output_channels: self.output_channels,
            ducking_engine: self.ducking_engine,
            bass_management: self.bass_management,
        }
    }
}

/// Drive `mix_audio` block-by-block, mirroring the real callback cadence
/// (including the finished-sample cleanup the callback performs), and
/// concatenate the output into a single interleaved buffer.
pub fn render(state: &mut MixerState, frames_per_block: usize, blocks: usize) -> Vec<f32> {
    let channels = state.output_channels;
    let mut out = Vec::with_capacity(frames_per_block * channels * blocks);
    let mut block = vec![0.0f32; frames_per_block * channels];
    for _ in 0..blocks {
        // mix_audio zeroes the block before mixing.
        mix_audio(&mut block, state);
        state.active_samples.retain(|s| !s.is_finished());
        out.extend_from_slice(&block);
    }
    out
}

/// Iterate one channel's samples out of an interleaved buffer.
fn channel_iter(buf: &[f32], channel: usize, channels: usize) -> impl Iterator<Item = f32> + '_ {
    buf.iter().skip(channel).step_by(channels).copied()
}

/// Root-mean-square level of one channel across all frames.
pub fn rms(buf: &[f32], channel: usize, channels: usize) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for v in channel_iter(buf, channel, channels) {
        sum += (v as f64) * (v as f64);
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

/// Peak absolute sample value across the whole interleaved buffer.
pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

/// Single-bin Goertzel magnitude for one channel at `freq`, normalized so a
/// full-scale sine at the exact bin reads ~1.0. A coarse band-energy probe.
pub fn band_energy(
    buf: &[f32],
    sample_rate: u32,
    channel: usize,
    channels: usize,
    freq: f32,
) -> f32 {
    let samples: Vec<f32> = channel_iter(buf, channel, channels).collect();
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let k = (freq * n as f32 / sample_rate as f32).round();
    let w = std::f32::consts::TAU * k / n as f32;
    let coeff = 2.0 * w.cos();
    let mut s_prev = 0.0f32;
    let mut s_prev2 = 0.0f32;
    for x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2;
    power.max(0.0).sqrt() / (n as f32 / 2.0)
}

/// Largest absolute difference between consecutive samples of one channel — a
/// simple click / discontinuity detector.
pub fn max_inter_sample_delta(buf: &[f32], channel: usize, channels: usize) -> f32 {
    let mut prev: Option<f32> = None;
    let mut max = 0.0f32;
    for v in channel_iter(buf, channel, channels) {
        if let Some(p) = prev {
            max = max.max((v - p).abs());
        }
        prev = Some(v);
    }
    max
}
