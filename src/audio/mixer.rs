// ABOUTME: Real-time audio mixer performing sample mixing and channel routing.
// ABOUTME: Runs in audio callback thread with strict real-time constraints.

use crate::audio::types::DecodedBuffer;
use std::sync::Arc;

/// Active sample being played
pub struct ActiveSample {
    /// Pre-decoded audio buffer (shared, immutable)
    pub buffer: Arc<DecodedBuffer>,

    /// Current playback position (in frames)
    pub position: usize,

    /// Per-sample volume (0.0 - 1.0)
    pub volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,
}

impl ActiveSample {
    /// Create a new active sample with default stereo mapping
    pub fn new(buffer: Arc<DecodedBuffer>, volume: f32) -> Self {
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels)
            .map(|ch| (ch, ch))
            .collect();

        Self {
            buffer,
            position: 0,
            volume,
            channel_map,
        }
    }

    /// Create a new active sample with custom channel mapping
    #[allow(dead_code)] // Used in Phase 7 for channel routing
    pub fn new_with_mapping(
        buffer: Arc<DecodedBuffer>,
        volume: f32,
        channel_map: Vec<(usize, usize)>,
    ) -> Self {
        Self {
            buffer,
            position: 0,
            volume,
            channel_map,
        }
    }

    /// Check if this sample has finished playing
    pub fn is_finished(&self) -> bool {
        self.position >= self.buffer.frames
    }
}

/// Mixer state shared between engine and audio callback
pub struct MixerState {
    /// List of currently playing samples
    pub active_samples: Vec<ActiveSample>,

    /// Number of output channels
    pub output_channels: usize,
}

impl MixerState {
    #[allow(dead_code)] // Used directly in engine.rs for now
    pub fn new(output_channels: usize) -> Self {
        Self {
            active_samples: Vec::new(),
            output_channels,
        }
    }
}

/// Mix all active samples into the output buffer
///
/// This function runs in the audio callback thread and must never:
/// - Allocate memory
/// - Block on I/O
/// - Acquire locks
/// - Do expensive computation
pub fn mix_audio(output: &mut [f32], state: &mut MixerState) {
    // Zero the output buffer
    output.fill(0.0);

    let frames = output.len() / state.output_channels;

    // Mix each active sample into the output
    for sample in &mut state.active_samples {
        mix_sample_into_output(sample, output, frames, state.output_channels);

        // Advance playback position
        sample.position += frames;
    }

    // Apply saturation to prevent clipping
    for s in output.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

/// Mix a single sample into the output buffer
fn mix_sample_into_output(
    sample: &ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
) {
    for frame_idx in 0..frames {
        let src_position = sample.position + frame_idx;

        // Check if we've reached the end of the sample
        if src_position >= sample.buffer.frames {
            break;
        }

        // Apply channel mapping and mix into output
        for &(src_ch, dest_ch) in &sample.channel_map {
            // Bounds check
            if src_ch >= sample.buffer.channels || dest_ch >= output_channels {
                continue;
            }

            let src_idx = src_position * sample.buffer.channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            // Mix with volume applied
            output[dest_idx] += sample.buffer.data[src_idx] * sample.volume;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_buffer(frames: usize, channels: usize, value: f32) -> Arc<DecodedBuffer> {
        let data = vec![value; frames * channels];
        Arc::new(DecodedBuffer::new(data, channels, 48000))
    }

    #[test]
    fn test_single_sample_mixing() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(buffer, 1.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Should have mixed 0.5 into all channels
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_multiple_samples_additive() {
        let buffer1 = create_test_buffer(10, 2, 0.3);
        let buffer2 = create_test_buffer(10, 2, 0.4);

        let sample1 = ActiveSample::new(buffer1, 1.0);
        let sample2 = ActiveSample::new(buffer2, 1.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample1);
        state.active_samples.push(sample2);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should have added 0.3 + 0.4 ≈ 0.7 (with floating point tolerance)
        for &s in &output {
            assert!((s - 0.7).abs() < 0.0001);
        }
    }

    #[test]
    fn test_volume_control() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(buffer, 0.5); // 50% volume

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 0.5 = 0.5
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_saturation() {
        let buffer1 = create_test_buffer(10, 2, 0.8);
        let buffer2 = create_test_buffer(10, 2, 0.8);

        let sample1 = ActiveSample::new(buffer1, 1.0);
        let sample2 = ActiveSample::new(buffer2, 1.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample1);
        state.active_samples.push(sample2);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be clamped to 1.0 (not 1.6)
        for &s in &output {
            assert_eq!(s, 1.0);
        }
    }

    #[test]
    fn test_channel_mapping() {
        // Stereo buffer with different values per channel
        let data = vec![0.3, 0.7, 0.3, 0.7]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        // Map: L→1, R→3 (4-channel output)
        let channel_map = vec![(0, 1), (1, 3)];
        let sample = ActiveSample::new_with_mapping(buffer, 1.0, channel_map);

        let mut state = MixerState::new(4);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 8]; // 2 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Frame 0
        assert_eq!(output[0], 0.0); // Channel 0: unmapped
        assert_eq!(output[1], 0.3); // Channel 1: L
        assert_eq!(output[2], 0.0); // Channel 2: unmapped
        assert_eq!(output[3], 0.7); // Channel 3: R

        // Frame 1
        assert_eq!(output[4], 0.0);
        assert_eq!(output[5], 0.3);
        assert_eq!(output[6], 0.0);
        assert_eq!(output[7], 0.7);
    }

    #[test]
    fn test_sample_completion() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let sample = ActiveSample::new(buffer, 1.0);

        assert!(!sample.is_finished());

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 5 frames - should exactly finish
        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_partial_sample_playback() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let mut sample = ActiveSample::new(buffer, 1.0);
        sample.position = 8; // Start near the end

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 5 frames, but only 2 remaining
        let mut output = vec![0.0f32; 10]; // 5 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First 2 frames should have audio
        assert_eq!(output[0], 0.5);
        assert_eq!(output[1], 0.5);
        assert_eq!(output[2], 0.5);
        assert_eq!(output[3], 0.5);

        // Remaining should be silence
        for &s in &output[4..] {
            assert_eq!(s, 0.0);
        }
    }
}
