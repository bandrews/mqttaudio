// ABOUTME: Real-time audio mixer performing sample mixing and channel routing.
// ABOUTME: Runs in audio callback thread with strict real-time constraints.

use crate::audio::types::DecodedBuffer;
use crate::audio::ducking::DuckingEngine;
use crate::audio::bass_management::BassManagement;
use ringbuf::HeapConsumer;
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
    #[cfg(test)]
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
    /// Unique internal sample ID (assigned by VoiceManager)
    pub id: u64,

    /// User-provided sample identifier for targeting commands
    pub sample_id: Option<String>,

    /// Voice ID this sample belongs to
    pub voice_id: String,

    /// Source file path (for targeting by filename)
    pub file_path: String,

    /// Pre-decoded audio buffer (shared, immutable)
    pub buffer: Arc<DecodedBuffer>,

    /// Current playback position (in frames, integer part)
    pub position: usize,

    /// Fractional part of playback position for sub-sample interpolation
    fractional_position: f64,

    /// Per-sample volume (0.0 - 1.0)
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0)
    pub voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,

    /// Fade state (in/out/none)
    pub fade_state: FadeState,

    /// Playback speed multiplier (1.0 = normal, 2.0 = double speed, 0.5 = half speed)
    pub speed: f32,
}

impl ActiveSample {
    /// Create a new active sample with default stereo mapping
    pub fn new(
        id: u64,
        voice_id: String,
        buffer: Arc<DecodedBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
    ) -> Self {
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels)
            .map(|ch| (ch, ch))
            .collect();

        Self {
            id,
            sample_id: None,
            voice_id,
            file_path,
            buffer,
            position: 0,
            fractional_position: 0.0,
            volume,
            voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
        }
    }

    /// Create a new active sample with user-provided sample ID
    pub fn new_with_id(
        id: u64,
        voice_id: String,
        buffer: Arc<DecodedBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
        sample_id: Option<String>,
    ) -> Self {
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels)
            .map(|ch| (ch, ch))
            .collect();

        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            buffer,
            position: 0,
            fractional_position: 0.0,
            volume,
            voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
        }
    }

    /// Create a new active sample with custom channel mapping
    pub fn new_with_mapping(
        id: u64,
        voice_id: String,
        buffer: Arc<DecodedBuffer>,
        volume: f32,
        voice_volume: f32,
        channel_map: Vec<(usize, usize)>,
        file_path: String,
        sample_id: Option<String>,
    ) -> Self {
        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            buffer,
            position: 0,
            fractional_position: 0.0,
            volume,
            voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
        }
    }

    /// Set the fade state for this sample
    pub fn set_fade(&mut self, fade_state: FadeState) {
        self.fade_state = fade_state;
    }

    /// Set the playback speed (clamped to 0.1 - 4.0)
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.1, 4.0);
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

    /// Advance the playback position by the given number of output frames,
    /// accounting for playback speed. Returns the effective number of source
    /// frames consumed.
    fn advance_position(&mut self, output_frames: usize) -> usize {
        let advance = output_frames as f64 * self.speed as f64;
        let new_pos = self.position as f64 + self.fractional_position + advance;

        self.position = new_pos as usize;
        self.fractional_position = new_pos.fract();

        // Return effective frames consumed
        advance.ceil() as usize
    }

    /// Get the current precise position as a float for interpolation
    fn precise_position(&self) -> f64 {
        self.position as f64 + self.fractional_position
    }
}

/// Active live input (microphone) being mixed
pub struct LiveInput {
    /// Voice ID this input belongs to (for ducking)
    pub voice_id: String,

    /// Ring buffer consumer for receiving audio samples
    pub consumer: HeapConsumer<f32>,

    /// Number of input channels
    pub input_channels: usize,

    /// Per-input volume (0.0 - 1.0)
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0)
    pub voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,
}

impl LiveInput {
    /// Create a new live input with the given routing
    pub fn new(
        voice_id: String,
        consumer: HeapConsumer<f32>,
        input_channels: usize,
        volume: f32,
        channel_map: Vec<(usize, usize)>,
    ) -> Self {
        Self {
            voice_id,
            consumer,
            input_channels,
            volume,
            voice_volume: 1.0,
            channel_map,
        }
    }

    /// Get the combined volume (input volume * voice volume)
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
    }
}

/// Mixer state shared between engine and audio callback
pub struct MixerState {
    /// List of currently playing samples
    pub active_samples: Vec<ActiveSample>,

    /// List of active live inputs (microphones)
    pub live_inputs: Vec<LiveInput>,

    /// Number of output channels
    pub output_channels: usize,

    /// Ducking engine for automatic voice volume reduction
    pub ducking_engine: Option<DuckingEngine>,

    /// Bass management for LFE extraction and crossover filtering
    pub bass_management: Option<BassManagement>,
}

impl MixerState {
    #[cfg(test)]
    pub fn new(output_channels: usize) -> Self {
        Self {
            active_samples: Vec::new(),
            live_inputs: Vec::new(),
            output_channels,
            ducking_engine: None,
            bass_management: None,
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
        // Get ducking multiplier for this sample's voice
        let ducking_multiplier = if let Some(ref mut engine) = state.ducking_engine {
            engine.get_multiplier(&sample.voice_id, frames)
        } else {
            1.0  // No ducking
        };

        mix_sample_into_output(sample, output, frames, state.output_channels, ducking_multiplier);

        // Advance playback position (accounting for speed)
        sample.advance_position(frames);
    }

    // Mix each live input into the output
    for input in &mut state.live_inputs {
        // Get ducking multiplier for this input's voice
        let ducking_multiplier = if let Some(ref mut engine) = state.ducking_engine {
            engine.get_multiplier(&input.voice_id, frames)
        } else {
            1.0  // No ducking
        };

        mix_live_input_into_output(input, output, frames, state.output_channels, ducking_multiplier);
    }

    // Apply bass management (LFE extraction and crossover filtering)
    if let Some(ref mut bm) = state.bass_management {
        bm.process(output, state.output_channels);
    }

    // Apply saturation to prevent clipping
    for s in output.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

/// Mix a single sample into the output buffer with linear interpolation for speed control
fn mix_sample_into_output(
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    ducking_multiplier: f32,
) {
    let base_volume = sample.combined_volume();
    let speed = sample.speed as f64;

    // Calculate current precise position (integer + fractional parts)
    let mut src_pos = sample.precise_position();

    for frame_idx in 0..frames {
        let src_frame = src_pos as usize;

        // Check if we've reached the end of the sample
        if src_frame >= sample.buffer.frames {
            break;
        }

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let final_volume = base_volume * fade_multiplier * ducking_multiplier;

        // Calculate fractional part for interpolation
        let frac = (src_pos - src_frame as f64) as f32;

        // Apply channel mapping and mix into output
        for &(src_ch, dest_ch) in &sample.channel_map {
            // Bounds check
            if src_ch >= sample.buffer.channels || dest_ch >= output_channels {
                continue;
            }

            let src_idx = src_frame * sample.buffer.channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            // Get current sample value
            let sample_val = sample.buffer.data[src_idx];

            // Interpolate with next sample if available and speed != 1.0
            let interpolated_val = if frac > 0.0 && src_frame + 1 < sample.buffer.frames {
                let next_idx = (src_frame + 1) * sample.buffer.channels + src_ch;
                let next_val = sample.buffer.data[next_idx];
                sample_val * (1.0 - frac) + next_val * frac
            } else {
                sample_val
            };

            // Mix with combined volume and fade applied
            output[dest_idx] += interpolated_val * final_volume;
        }

        // Advance fade state
        sample.fade_state.advance();

        // Advance source position by speed
        src_pos += speed;
    }
}

/// Mix a live input (microphone) into the output buffer
fn mix_live_input_into_output(
    input: &mut LiveInput,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    ducking_multiplier: f32,
) {
    let final_volume = input.combined_volume() * ducking_multiplier;
    let input_channels = input.input_channels;

    // Read available samples from the ring buffer
    // Process frame by frame to handle underruns gracefully
    for frame_idx in 0..frames {
        // Read one frame worth of samples
        let samples_needed = input_channels;
        let samples_available = input.consumer.len();

        if samples_available < samples_needed {
            // Underrun - not enough samples for a complete frame
            // Leave remaining output as silence (already zeroed)
            break;
        }

        // Read the entire frame from the ring buffer
        let mut frame_samples = [0.0f32; 16]; // Support up to 16 input channels
        for ch in 0..input_channels.min(16) {
            if let Some(sample) = input.consumer.pop() {
                frame_samples[ch] = sample;
            }
        }

        // Apply channel mapping
        for &(src_ch, dest_ch) in &input.channel_map {
            if src_ch >= input_channels || dest_ch >= output_channels || src_ch >= 16 {
                continue;
            }

            let dest_idx = frame_idx * output_channels + dest_ch;
            output[dest_idx] += frame_samples[src_ch] * final_volume;
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

    const TEST_FILE: &str = "test.wav";

    #[test]
    fn test_single_sample_mixing() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

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

        let sample1 = ActiveSample::new(1, "test".to_string(), buffer1, 1.0, 1.0, TEST_FILE.to_string());
        let sample2 = ActiveSample::new(2, "test".to_string(), buffer2, 1.0, 1.0, TEST_FILE.to_string());

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 0.5, 1.0, TEST_FILE.to_string()); // 50% sample volume

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 0.5, TEST_FILE.to_string()); // 50% voice volume

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 0.5, 0.4, TEST_FILE.to_string()); // 50% sample * 40% voice

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

        let sample1 = ActiveSample::new(1, "test".to_string(), buffer1, 1.0, 1.0, TEST_FILE.to_string());
        let sample2 = ActiveSample::new(2, "test".to_string(), buffer2, 1.0, 1.0, TEST_FILE.to_string());

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None);

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
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

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
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None);

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
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

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
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

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
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

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

    // === LiveInput Tests ===

    fn create_test_ring_buffer_with_data(data: &[f32]) -> HeapConsumer<f32> {
        use ringbuf::HeapRb;
        let rb = HeapRb::<f32>::new(data.len() + 100);
        let (mut producer, consumer) = rb.split();
        producer.push_slice(data);
        consumer
    }

    #[test]
    fn test_live_input_basic_mixing() {
        // Create ring buffer with stereo data (10 frames)
        let data: Vec<f32> = (0..20).map(|i| if i % 2 == 0 { 0.5 } else { 0.3 }).collect();
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            2, // stereo input
            1.0,
            vec![(0, 0), (1, 1)], // 1:1 mapping
        );

        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.5); // Left
        assert_eq!(output[1], 0.3); // Right

        // Check last frame
        assert_eq!(output[18], 0.5);
        assert_eq!(output[19], 0.3);
    }

    #[test]
    fn test_live_input_volume() {
        // Create ring buffer with mono data
        let data = vec![1.0f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            1, // mono input
            0.5, // 50% volume
            vec![(0, 0)],
        );

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 10]; // 10 frames * 1 channel
        mix_audio(&mut output, &mut state);

        for &s in &output {
            assert_eq!(s, 0.5); // 1.0 * 0.5 volume
        }
    }

    #[test]
    fn test_live_input_channel_routing() {
        // Create ring buffer with mono data
        let data = vec![0.7f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        // Route mono input to channels 2 and 3 (4-channel output)
        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            1,
            1.0,
            vec![(0, 2), (0, 3)],
        );

        let mut state = MixerState::new(4);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 40]; // 10 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.0); // Channel 0
        assert_eq!(output[1], 0.0); // Channel 1
        assert_eq!(output[2], 0.7); // Channel 2
        assert_eq!(output[3], 0.7); // Channel 3
    }

    #[test]
    fn test_live_input_underrun() {
        // Create ring buffer with only 5 frames of data but request 10
        let data = vec![0.8f32; 10]; // 5 stereo frames
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            2,
            1.0,
            vec![(0, 0), (1, 1)],
        );

        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20]; // 10 frames requested
        mix_audio(&mut output, &mut state);

        // First 5 frames should have audio
        for &s in &output[..10] {
            assert_eq!(s, 0.8);
        }

        // Remaining 5 frames should be silence (underrun)
        for &s in &output[10..] {
            assert_eq!(s, 0.0);
        }
    }

    #[test]
    fn test_live_input_mixed_with_samples() {
        // Create a sample and a live input, verify they mix together
        let buffer = create_test_buffer(10, 2, 0.3);
        let sample = ActiveSample::new(1, "sfx".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        let data = vec![0.4f32; 20]; // 10 stereo frames
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            2,
            1.0,
            vec![(0, 0), (1, 1)],
        );

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 0.3 (sample) + 0.4 (input) = 0.7
        for &s in &output {
            assert!((s - 0.7).abs() < 0.0001);
        }
    }

    #[test]
    fn test_live_input_voice_volume() {
        let data = vec![1.0f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        let mut live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            1,
            0.5, // 50% input volume
            vec![(0, 0)],
        );
        live_input.voice_volume = 0.6; // 60% voice volume

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        // Combined: 1.0 * 0.5 * 0.6 = 0.3
        for &s in &output {
            assert!((s - 0.3).abs() < 0.0001);
        }
    }

    #[test]
    fn test_multiple_live_inputs() {
        // Two microphones mixing to same outputs
        let data1 = vec![0.2f32; 10];
        let consumer1 = create_test_ring_buffer_with_data(&data1);
        let live_input1 = LiveInput::new("mic1".to_string(), consumer1, 1, 1.0, vec![(0, 0)]);

        let data2 = vec![0.3f32; 10];
        let consumer2 = create_test_ring_buffer_with_data(&data2);
        let live_input2 = LiveInput::new("mic2".to_string(), consumer2, 1, 1.0, vec![(0, 0)]);

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input1);
        state.live_inputs.push(live_input2);

        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        // Both inputs mix additively: 0.2 + 0.3 = 0.5
        for &s in &output {
            assert!((s - 0.5).abs() < 0.0001);
        }
    }

    // === Sample Identification Tests ===

    #[test]
    fn test_sample_stores_file_path() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
        );

        assert_eq!(sample.file_path, "/path/to/audio.wav");
    }

    #[test]
    fn test_sample_with_user_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_id(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
            Some("my-sound-1".to_string()),
        );

        assert_eq!(sample.sample_id, Some("my-sound-1".to_string()));
        assert_eq!(sample.file_path, "/path/to/audio.wav");
    }

    #[test]
    fn test_sample_without_user_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
        );

        assert_eq!(sample.sample_id, None);
    }

    #[test]
    fn test_sample_with_mapping_stores_file_path() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            vec![(0, 0), (1, 1)],
            "/path/to/audio.wav".to_string(),
            None,
        );

        assert_eq!(sample.file_path, "/path/to/audio.wav");
        assert_eq!(sample.sample_id, None);
    }

    #[test]
    fn test_sample_with_mapping_and_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            vec![(0, 2), (1, 3)],
            "/path/to/surround.wav".to_string(),
            Some("surround-effect".to_string()),
        );

        assert_eq!(sample.file_path, "/path/to/surround.wav");
        assert_eq!(sample.sample_id, Some("surround-effect".to_string()));
        assert_eq!(sample.channel_map, vec![(0, 2), (1, 3)]);
    }

    // === Seek Tests ===

    #[test]
    fn test_seek_to_position() {
        // Create a buffer at 48000 Hz with 48000 frames (1 second)
        let data = vec![0.5f32; 48000 * 2]; // 1 second stereo
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Initial position is 0
        assert_eq!(sample.position, 0);

        // Seek to 500ms (should be frame 24000)
        let position_ms: u64 = 500;
        let target_frame = ((position_ms * sample.buffer.sample_rate as u64) / 1000) as usize;
        sample.position = target_frame;

        assert_eq!(sample.position, 24000);
    }

    #[test]
    fn test_seek_to_start() {
        let data = vec![0.5f32; 48000 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Advance position
        sample.position = 10000;

        // Seek to 0ms
        let position_ms: u64 = 0;
        let target_frame = ((position_ms * sample.buffer.sample_rate as u64) / 1000) as usize;
        sample.position = target_frame;

        assert_eq!(sample.position, 0);
    }

    #[test]
    fn test_seek_clamps_to_buffer_end() {
        let data = vec![0.5f32; 48000 * 2]; // 1 second stereo at 48kHz
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Try to seek to 2 seconds (beyond buffer)
        let position_ms: u64 = 2000;
        let target_frame = ((position_ms * sample.buffer.sample_rate as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames.saturating_sub(1));

        // Should be clamped to last valid frame
        assert_eq!(sample.position, 47999); // frames - 1
    }

    #[test]
    fn test_seek_with_different_sample_rates() {
        // Test seek calculation at 44100 Hz
        let data = vec![0.5f32; 44100 * 2]; // 1 second stereo at 44.1kHz
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 44100));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Seek to 1000ms (should be frame 44100)
        let position_ms: u64 = 1000;
        let target_frame = ((position_ms * sample.buffer.sample_rate as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames.saturating_sub(1));

        // At 44100 Hz, 1000ms = 44100 frames, but clamped to 44099
        assert_eq!(sample.position, 44099);
    }

    #[test]
    fn test_seek_preserves_playback_state() {
        let data = vec![0.5f32; 48000 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 0.7, 0.8, TEST_FILE.to_string());
        sample.fade_state = FadeState::In { elapsed: 100, duration: 200 };

        // Seek to 250ms
        let position_ms: u64 = 250;
        let target_frame = ((position_ms * sample.buffer.sample_rate as u64) / 1000) as usize;
        sample.position = target_frame;

        // Volume and fade state should be preserved
        assert_eq!(sample.volume, 0.7);
        assert_eq!(sample.voice_volume, 0.8);
        assert!(matches!(sample.fade_state, FadeState::In { elapsed: 100, duration: 200 }));
        assert_eq!(sample.position, 12000); // 250ms at 48kHz
    }

    #[test]
    fn test_seek_then_mix() {
        // Create buffer with distinct values at different positions
        let mut data = vec![0.0f32; 100 * 2]; // 100 stereo frames
        // First 50 frames: 0.1
        for i in 0..100 {
            data[i] = 0.1;
        }
        // Last 50 frames: 0.9
        for i in 100..200 {
            data[i] = 0.9;
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Seek to frame 50 (the start of the 0.9 section)
        sample.position = 50;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 10]; // 5 stereo frames
        mix_audio(&mut output, &mut state);

        // Should get 0.9 values (from the second half)
        for &s in &output {
            assert_eq!(s, 0.9);
        }
    }

    // === Speed Control Tests ===

    #[test]
    fn test_speed_default_is_normal() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_speed_double_plays_twice_as_fast() {
        // Create buffer with 100 frames of constant value
        let data = vec![0.5f32; 100 * 2]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());
        sample.speed = 2.0;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames at 2x speed should consume 100 input frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // After mixing 50 output frames at 2x, effective position should be 100
        // (meaning sample should be finished)
        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_speed_half_plays_twice_as_slow() {
        let data = vec![0.5f32; 100 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());
        sample.speed = 0.5;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames at 0.5x speed should consume 25 input frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // After mixing 50 output frames at 0.5x, effective position should be 25
        // Sample should NOT be finished (only 1/4 through)
        assert!(!state.active_samples[0].is_finished());
        // Check position is roughly 25
        assert_eq!(state.active_samples[0].position, 25);
    }

    #[test]
    fn test_speed_interpolation() {
        // Create buffer with linear ramp: 0.0, 0.2, 0.4, 0.6, 0.8, 1.0
        let data = vec![0.0, 0.0, 0.2, 0.2, 0.4, 0.4, 0.6, 0.6, 0.8, 0.8, 1.0, 1.0]; // 6 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.speed = 0.5;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // At 0.5x speed, reading 2 output frames should cover 1 input frame
        // and we should see interpolated values
        let mut output = vec![0.0f32; 4]; // 2 stereo frames
        mix_audio(&mut output, &mut state);

        // First output frame: position 0 -> value 0.0
        assert!((output[0] - 0.0).abs() < 0.01);
        // Second output frame: position 0.5 -> interpolate between 0.0 and 0.2 = 0.1
        assert!((output[2] - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_speed_set_via_method() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        sample.set_speed(1.5);
        assert_eq!(sample.speed, 1.5);

        sample.set_speed(0.0); // Should clamp to minimum
        assert!(sample.speed >= 0.1); // Some reasonable minimum

        sample.set_speed(10.0); // Should clamp to maximum
        assert!(sample.speed <= 4.0); // Some reasonable maximum
    }
}

