// ABOUTME: Real-time audio mixer performing sample mixing and channel routing.
// ABOUTME: Runs in audio callback thread with strict real-time constraints.

use crate::audio::types::DecodedBuffer;
use std::sync::Arc;

/// Fade state for audio samples
#[derive(Debug, Clone, PartialEq)]
pub enum FadeState {
    /// No fading
    None,
    /// Fading in: (frames_elapsed, total_fade_frames)
    In { elapsed: usize, duration: usize },
    /// Fading out: (frames_elapsed, total_fade_frames)
    Out { elapsed: usize, duration: usize },
}

impl FadeState {
    /// Create a new fade in state
    pub fn fade_in(duration_ms: u32, sample_rate: u32) -> Self {
        let duration_frames = ((duration_ms as f32 / 1000.0) * sample_rate as f32) as usize;
        FadeState::In {
            elapsed: 0,
            duration: duration_frames,
        }
    }

    /// Create a new fade out state
    pub fn fade_out(duration_ms: u32, sample_rate: u32) -> Self {
        let duration_frames = ((duration_ms as f32 / 1000.0) * sample_rate as f32) as usize;
        FadeState::Out {
            elapsed: 0,
            duration: duration_frames,
        }
    }

    /// Calculate fade multiplier for the current position
    /// Returns a value between 0.0 and 1.0
    pub fn multiplier(&self) -> f32 {
        match self {
            FadeState::None => 1.0,
            FadeState::In { elapsed, duration } => {
                if *duration == 0 {
                    return 1.0;
                }
                (*elapsed as f32 / *duration as f32).min(1.0)
            }
            FadeState::Out { elapsed, duration } => {
                if *duration == 0 {
                    return 0.0;
                }
                (1.0 - (*elapsed as f32 / *duration as f32)).max(0.0)
            }
        }
    }

    /// Advance fade state by one frame
    pub fn advance(&mut self) {
        match self {
            FadeState::In { elapsed, duration } => {
                if *elapsed < *duration {
                    *elapsed += 1;
                }
            }
            FadeState::Out { elapsed, duration } => {
                if *elapsed < *duration {
                    *elapsed += 1;
                }
            }
            FadeState::None => {}
        }
    }

    /// Check if fade is complete
    pub fn is_complete(&self) -> bool {
        match self {
            FadeState::None => true,
            FadeState::In { elapsed, duration } => elapsed >= duration,
            FadeState::Out { elapsed, duration } => elapsed >= duration,
        }
    }
}

/// Active sample being played
pub struct ActiveSample {
    /// Unique sample ID
    pub id: u64,

    /// Voice ID this sample belongs to
    pub voice_id: String,

    /// Pre-decoded audio buffer (shared, immutable)
    pub buffer: Arc<DecodedBuffer>,

    /// Current playback position (in frames)
    pub position: usize,

    /// Per-sample volume (0.0 - 1.0)
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0)
    pub voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,

    /// Fade state (in/out/none)
    pub fade_state: FadeState,
}

impl ActiveSample {
    /// Create a new active sample with default stereo mapping
    pub fn new(id: u64, voice_id: String, buffer: Arc<DecodedBuffer>, volume: f32, voice_volume: f32) -> Self {
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels)
            .map(|ch| (ch, ch))
            .collect();

        Self {
            id,
            voice_id,
            buffer,
            position: 0,
            volume,
            voice_volume,
            channel_map,
            fade_state: FadeState::None,
        }
    }

    /// Create a new active sample with custom channel mapping
    #[allow(dead_code)] // Used in Phase 7 for channel routing
    pub fn new_with_mapping(
        id: u64,
        voice_id: String,
        buffer: Arc<DecodedBuffer>,
        volume: f32,
        voice_volume: f32,
        channel_map: Vec<(usize, usize)>,
    ) -> Self {
        Self {
            id,
            voice_id,
            buffer,
            position: 0,
            volume,
            voice_volume,
            channel_map,
            fade_state: FadeState::None,
        }
    }

    /// Set the fade state for this sample
    pub fn set_fade(&mut self, fade_state: FadeState) {
        self.fade_state = fade_state;
    }

    /// Check if this sample has finished playing
    /// A sample is finished if it reached the end OR if fade out is complete
    pub fn is_finished(&self) -> bool {
        // Reached end of buffer
        if self.position >= self.buffer.frames {
            return true;
        }

        // Fade out complete
        match self.fade_state {
            FadeState::Out { elapsed, duration } => elapsed >= duration,
            _ => false,
        }
    }

    /// Get the combined volume (sample volume * voice volume)
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
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
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
) {
    let base_volume = sample.combined_volume();

    for frame_idx in 0..frames {
        let src_position = sample.position + frame_idx;

        // Check if we've reached the end of the sample
        if src_position >= sample.buffer.frames {
            break;
        }

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let final_volume = base_volume * fade_multiplier;

        // Apply channel mapping and mix into output
        for &(src_ch, dest_ch) in &sample.channel_map {
            // Bounds check
            if src_ch >= sample.buffer.channels || dest_ch >= output_channels {
                continue;
            }

            let src_idx = src_position * sample.buffer.channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            // Mix with combined volume and fade applied
            output[dest_idx] += sample.buffer.data[src_idx] * final_volume;
        }

        // Advance fade state
        sample.fade_state.advance();
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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);

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

        let sample1 = ActiveSample::new(1, "test".to_string(), buffer1, 1.0, 1.0);
        let sample2 = ActiveSample::new(2, "test".to_string(), buffer2, 1.0, 1.0);

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 0.5, 1.0); // 50% sample volume

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
    fn test_voice_volume() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 0.5); // 50% voice volume

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 1.0 * 0.5 = 0.5
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_combined_volume() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 0.5, 0.4); // 50% sample * 40% voice

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 0.5 * 0.4 = 0.2
        for &s in &output {
            assert!((s - 0.2).abs() < 0.0001);
        }
    }

    #[test]
    fn test_saturation() {
        let buffer1 = create_test_buffer(10, 2, 0.8);
        let buffer2 = create_test_buffer(10, 2, 0.8);

        let sample1 = ActiveSample::new(1, "test".to_string(), buffer1, 1.0, 1.0);
        let sample2 = ActiveSample::new(2, "test".to_string(), buffer2, 1.0, 1.0);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map);

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);

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
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);
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

    #[test]
    fn test_mono_to_multichannel() {
        // Mono source to 8-channel output
        let data = vec![0.5; 10]; // 10 frames, 1 channel
        let buffer = Arc::new(DecodedBuffer::new(data, 1, 48000));

        // Route mono to channels 4 and 5
        let channel_map = vec![(0, 4), (0, 5)];
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map);

        let mut state = MixerState::new(8);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 80]; // 10 frames * 8 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.0); // Channel 0: silence
        assert_eq!(output[1], 0.0); // Channel 1: silence
        assert_eq!(output[2], 0.0); // Channel 2: silence
        assert_eq!(output[3], 0.0); // Channel 3: silence
        assert_eq!(output[4], 0.5); // Channel 4: mono signal
        assert_eq!(output[5], 0.5); // Channel 5: mono signal
        assert_eq!(output[6], 0.0); // Channel 6: silence
        assert_eq!(output[7], 0.0); // Channel 7: silence

        // Check second frame
        assert_eq!(output[12], 0.5); // Frame 1, Channel 4
        assert_eq!(output[13], 0.5); // Frame 1, Channel 5
    }

    #[test]
    fn test_quad_to_stereo_downmix() {
        // 4-channel source to stereo output (channels 0 and 1)
        let mut data = Vec::new();
        for _ in 0..10 {  // 10 frames
            data.push(0.1); // Front left
            data.push(0.2); // Front right
            data.push(0.3); // Rear left
            data.push(0.4); // Rear right
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 4, 48000));

        // Mix surround channels to stereo
        let channel_map = vec![
            (0, 0), // Front left → Left
            (1, 1), // Front right → Right
            (2, 0), // Rear left → Left (mix)
            (3, 1), // Rear right → Right (mix)
        ];
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame: L should be 0.1 + 0.3 = 0.4, R should be 0.2 + 0.4 = 0.6
        assert!((output[0] - 0.4).abs() < 0.0001);
        assert!((output[1] - 0.6).abs() < 0.0001);
    }

    #[test]
    fn test_sparse_channel_routing() {
        // Stereo to channels 6 and 9 (leaving gaps)
        let data = vec![0.3, 0.7, 0.3, 0.7]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let channel_map = vec![(0, 6), (1, 9)];
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map);

        let mut state = MixerState::new(12);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 24]; // 2 frames * 12 channels
        mix_audio(&mut output, &mut state);

        // Frame 0
        for ch in 0..12 {
            let idx = ch;
            if ch == 6 {
                assert_eq!(output[idx], 0.3);
            } else if ch == 9 {
                assert_eq!(output[idx], 0.7);
            } else {
                assert_eq!(output[idx], 0.0);
            }
        }
    }

    #[test]
    fn test_out_of_bounds_channel_routing() {
        // Test that out-of-bounds channel mappings are safely ignored
        let data = vec![0.5, 0.5, 0.5, 0.5]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        // Map to invalid destinations
        let channel_map = vec![
            (0, 0),   // Valid
            (1, 100), // Out of bounds (only 4 output channels)
            (99, 2),  // Invalid source
        ];
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map);

        let mut state = MixerState::new(4);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 8]; // 2 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Only channel 0 should have audio (the valid mapping)
        assert_eq!(output[0], 0.5); // Frame 0, Ch 0
        assert_eq!(output[1], 0.0); // Frame 0, Ch 1
        assert_eq!(output[2], 0.0); // Frame 0, Ch 2
        assert_eq!(output[3], 0.0); // Frame 0, Ch 3
    }

    // === Fade Tests ===

    #[test]
    fn test_fade_state_creation() {
        let fade_in = FadeState::fade_in(1000, 48000); // 1 second at 48kHz
        assert_eq!(fade_in, FadeState::In { elapsed: 0, duration: 48000 });

        let fade_out = FadeState::fade_out(500, 44100); // 0.5 seconds at 44.1kHz
        assert_eq!(fade_out, FadeState::Out { elapsed: 0, duration: 22050 });
    }

    #[test]
    fn test_fade_in_multiplier() {
        // At start: 0/10 = 0.0
        let fade_start = FadeState::In { elapsed: 0, duration: 10 };
        assert_eq!(fade_start.multiplier(), 0.0);

        // At 30%: 3/10 = 0.3
        let fade_30 = FadeState::In { elapsed: 3, duration: 10 };
        assert!((fade_30.multiplier() - 0.3).abs() < 0.01);

        // At 50%: 5/10 = 0.5
        let fade_50 = FadeState::In { elapsed: 5, duration: 10 };
        assert_eq!(fade_50.multiplier(), 0.5);

        // At 100%: 10/10 = 1.0
        let fade_100 = FadeState::In { elapsed: 10, duration: 10 };
        assert_eq!(fade_100.multiplier(), 1.0);

        // Beyond 100%: clamped to 1.0
        let fade_over = FadeState::In { elapsed: 15, duration: 10 };
        assert_eq!(fade_over.multiplier(), 1.0);
    }

    #[test]
    fn test_fade_out_multiplier() {
        // At start: 1 - (0/10) = 1.0
        let fade_start = FadeState::Out { elapsed: 0, duration: 10 };
        assert_eq!(fade_start.multiplier(), 1.0);

        // At 30%: 1 - (3/10) = 0.7
        let fade_30 = FadeState::Out { elapsed: 3, duration: 10 };
        assert!((fade_30.multiplier() - 0.7).abs() < 0.01);

        // At 50%: 1 - (5/10) = 0.5
        let fade_50 = FadeState::Out { elapsed: 5, duration: 10 };
        assert_eq!(fade_50.multiplier(), 0.5);

        // At 100%: 1 - (10/10) = 0.0
        let fade_100 = FadeState::Out { elapsed: 10, duration: 10 };
        assert_eq!(fade_100.multiplier(), 0.0);

        // Beyond 100%: clamped to 0.0
        let fade_over = FadeState::Out { elapsed: 15, duration: 10 };
        assert_eq!(fade_over.multiplier(), 0.0);
    }

    #[test]
    fn test_fade_state_advance() {
        let mut fade = FadeState::In { elapsed: 0, duration: 5 };

        assert!(!fade.is_complete());
        fade.advance();
        assert_eq!(fade, FadeState::In { elapsed: 1, duration: 5 });

        for _ in 0..4 {
            fade.advance();
        }
        assert_eq!(fade, FadeState::In { elapsed: 5, duration: 5 });
        assert!(fade.is_complete());

        // Advancing past completion stays at max
        fade.advance();
        assert_eq!(fade, FadeState::In { elapsed: 5, duration: 5 });
    }

    #[test]
    fn test_fade_in_mixing() {
        // Create a 10-frame buffer at 48kHz
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);

        // Set 10-frame fade in (one frame per iteration)
        sample.set_fade(FadeState::In { elapsed: 0, duration: 10 });

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Process 10 frames
        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame should be silent (0/10 = 0.0)
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.0);

        // Frame at position 5 should be ~0.5 (5/10 = 0.5)
        let frame5_l = output[10]; // Frame 5, channel 0
        let frame5_r = output[11]; // Frame 5, channel 1
        assert!((frame5_l - 0.5).abs() < 0.1);
        assert!((frame5_r - 0.5).abs() < 0.1);

        // Last frame should be close to 1.0 (9/10 = 0.9)
        let last_l = output[18]; // Frame 9, channel 0
        let last_r = output[19]; // Frame 9, channel 1
        assert!((last_l - 0.9).abs() < 0.1);
        assert!((last_r - 0.9).abs() < 0.1);
    }

    #[test]
    fn test_fade_out_mixing() {
        // Create a 10-frame buffer
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);

        // Set 10-frame fade out
        sample.set_fade(FadeState::Out { elapsed: 0, duration: 10 });

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame should be full volume (1 - 0/10 = 1.0)
        assert_eq!(output[0], 1.0);
        assert_eq!(output[1], 1.0);

        // Frame at position 5 should be ~0.5 (1 - 5/10 = 0.5)
        let frame5_l = output[10];
        let frame5_r = output[11];
        assert!((frame5_l - 0.5).abs() < 0.1);
        assert!((frame5_r - 0.5).abs() < 0.1);

        // Last frame should be close to 0.1 (1 - 9/10 = 0.1)
        let last_l = output[18];
        let last_r = output[19];
        assert!((last_l - 0.1).abs() < 0.1);
        assert!((last_r - 0.1).abs() < 0.1);
    }

    #[test]
    fn test_fade_out_completes_sample() {
        // Create a long buffer
        let buffer = create_test_buffer(100, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0);

        // Set short fade out
        sample.set_fade(FadeState::Out { elapsed: 0, duration: 5 });

        assert!(!sample.is_finished());

        // Manually advance to completion
        for _ in 0..5 {
            sample.fade_state.advance();
        }

        // Sample should be marked as finished even though buffer has more frames
        assert!(sample.is_finished());
    }

    #[test]
    fn test_zero_duration_fade() {
        let fade_in = FadeState::In { elapsed: 0, duration: 0 };
        assert_eq!(fade_in.multiplier(), 1.0); // Instant full volume

        let fade_out = FadeState::Out { elapsed: 0, duration: 0 };
        assert_eq!(fade_out.multiplier(), 0.0); // Instant silence
    }
}

