// ABOUTME: Real-time audio mixer performing sample mixing and channel routing.
// ABOUTME: Runs in audio callback thread with strict real-time constraints.

use crate::audio::streaming::SampleBuffer;
use crate::audio::ducking::DuckingEngine;
use crate::audio::bass_management::BassManagement;
use crate::audio::pitch_correction::PitchCorrector;
use ringbuf::HeapConsumer;

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

    /// Audio buffer - either complete (cached) or streaming (loading)
    pub buffer: SampleBuffer,

    /// Current playback position (in frames, integer part)
    pub position: usize,

    /// Fractional part of playback position for sub-sample interpolation
    fractional_position: f64,

    /// Per-sample volume (0.0 - 1.0)
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0) - current smoothed value
    pub voice_volume: f32,

    /// Target voice volume for smooth ramping (0.0 - 1.0)
    pub target_voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,

    /// Fade state (in/out/none)
    pub fade_state: FadeState,

    /// Playback speed multiplier (1.0 = normal, 2.0 = double speed, 0.5 = half speed)
    pub speed: f32,

    /// Pitch corrector for time-stretching without pitch change
    pub pitch_corrector: Option<PitchCorrector>,

    /// Loop playback continuously
    pub loop_mode: bool,

    /// Number of samples to crossfade at loop boundaries (0 = disabled)
    pub crossfade_samples: usize,
}

impl ActiveSample {
    /// Create a new active sample with default stereo mapping
    pub fn new(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
    ) -> Self {
        let buffer = buffer.into();
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels())
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
            target_voice_volume: voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode: false,
            crossfade_samples: 0,
        }
    }

    /// Create a new active sample with user-provided sample ID
    pub fn new_with_id(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
        sample_id: Option<String>,
        loop_mode: bool,
        crossfade_samples: usize,
    ) -> Self {
        let buffer = buffer.into();
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels())
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
            target_voice_volume: voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode,
            crossfade_samples,
        }
    }

    /// Create a new active sample with custom channel mapping
    pub fn new_with_mapping(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        channel_map: Vec<(usize, usize)>,
        file_path: String,
        sample_id: Option<String>,
        loop_mode: bool,
        crossfade_samples: usize,
    ) -> Self {
        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            buffer: buffer.into(),
            position: 0,
            fractional_position: 0.0,
            volume,
            voice_volume,
            target_voice_volume: voice_volume,
            channel_map,
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode,
            crossfade_samples,
        }
    }

    /// Set the fade state for this sample
    pub fn set_fade(&mut self, fade_state: FadeState) {
        self.fade_state = fade_state;
    }

    /// Set the playback speed.
    ///
    /// Without pitch correction: supports -100.0 to 100.0 (negative = reverse)
    /// With pitch correction: supports 0.05 to 8.0 (no reverse)
    ///
    /// Returns true if the speed was set, false if it was rejected
    /// (e.g., negative speed with pitch correction enabled).
    pub fn set_speed(&mut self, speed: f32) -> bool {
        if self.pitch_corrector.is_some() {
            // Pitch correction: 0.05 to 8.0, no reverse
            if speed < 0.0 {
                tracing::warn!(
                    "Negative speed ({}) not supported with pitch correction, ignoring",
                    speed
                );
                return false;
            }
            self.speed = speed.clamp(0.05, 8.0);
            if let Some(ref mut pc) = self.pitch_corrector {
                pc.set_speed(self.speed);
            }
        } else {
            // No pitch correction: -100.0 to 100.0, but not zero
            if speed.abs() < 0.01 {
                // Treat very small speeds as minimum
                self.speed = if speed < 0.0 { -0.01 } else { 0.01 };
            } else {
                self.speed = speed.clamp(-100.0, 100.0);
            }
        }
        true
    }

    /// Enable pitch correction (preserves pitch when changing speed).
    /// Creates a new PitchCorrector if one doesn't exist.
    pub fn enable_pitch_correction(&mut self) {
        if self.pitch_corrector.is_none() {
            let mut pc = PitchCorrector::new(
                self.buffer.channels(),
                self.buffer.sample_rate(),
            );
            pc.set_speed(self.speed);
            self.pitch_corrector = Some(pc);
        }
    }

    /// Set speed and pitch correction mode together.
    /// This handles the correct order of operations: pitch correction mode
    /// must be changed before setting speed (negative speeds are rejected
    /// while pitch correction is enabled).
    ///
    /// Returns true if the speed was set, false if rejected.
    pub fn set_speed_with_mode(&mut self, speed: f32, pitch_correction: bool) -> bool {
        // Change pitch correction mode FIRST
        if pitch_correction {
            self.enable_pitch_correction();
        } else {
            self.disable_pitch_correction();
        }
        // Then set speed
        self.set_speed(speed)
    }

    /// Disable pitch correction (pitch follows speed).
    pub fn disable_pitch_correction(&mut self) {
        self.pitch_corrector = None;
    }

    /// Check if pitch correction is enabled.
    #[allow(dead_code)]
    pub fn has_pitch_correction(&self) -> bool {
        self.pitch_corrector.is_some()
    }

    /// Check if this sample has finished playing
    /// A sample is finished if it reached the end (or start for reverse) OR if fade out is complete
    /// Looping samples only finish when fade out is complete
    pub fn is_finished(&self) -> bool {
        // Fade out complete - always finishes, even for looping samples
        match self.fade_state {
            FadeState::Out { elapsed, duration } => {
                if elapsed >= duration {
                    return true;
                }
            }
            _ => {}
        }

        // Looping samples never finish from buffer position
        if self.loop_mode {
            return false;
        }

        // For forward playback, finished when position >= frames
        // For reverse playback, finished when position is 0 (or we've gone negative)
        if self.speed >= 0.0 {
            if self.position >= self.buffer.frames() {
                return true;
            }
        } else {
            // Reverse playback: finished when we've reached/passed the start
            // The fractional position going negative indicates we've exhausted the buffer
            if self.position == 0 && self.fractional_position <= 0.0 {
                return true;
            }
        }

        false
    }

    /// Get the combined volume (sample volume * voice volume)
    #[allow(dead_code)]
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
    }

    /// Set target voice volume for smooth ramping
    pub fn set_target_voice_volume(&mut self, target: f32) {
        self.target_voice_volume = target.clamp(0.0, 1.0);
    }

    /// Advance voice volume toward target by one frame.
    /// Uses linear ramping with a fixed rate that provides ~20ms transition at 44.1kHz.
    /// Returns true if volume is still ramping, false if at target.
    pub fn advance_voice_volume(&mut self) -> bool {
        const RAMP_RATE: f32 = 0.001; // ~22ms for full 0-1 transition at 44.1kHz

        if (self.voice_volume - self.target_voice_volume).abs() < RAMP_RATE {
            self.voice_volume = self.target_voice_volume;
            false
        } else if self.voice_volume < self.target_voice_volume {
            self.voice_volume += RAMP_RATE;
            true
        } else {
            self.voice_volume -= RAMP_RATE;
            true
        }
    }

    /// Advance the playback position by the given number of output frames,
    /// accounting for playback speed (including negative for reverse).
    /// Handles looping by wrapping position to other end of buffer.
    /// Returns the effective number of source frames consumed (absolute value).
    fn advance_position(&mut self, output_frames: usize) -> usize {
        let advance = output_frames as f64 * self.speed as f64;
        let new_pos = self.position as f64 + self.fractional_position + advance;
        let buffer_frames = self.buffer.frames();

        if self.loop_mode && buffer_frames > 0 {
            // Handle looping
            if new_pos < 0.0 {
                // Reverse playback wrapped past start - loop to end
                let wrapped = new_pos % buffer_frames as f64 + buffer_frames as f64;
                self.position = wrapped as usize % buffer_frames;
                self.fractional_position = wrapped.fract();
            } else if new_pos >= buffer_frames as f64 {
                // Forward playback wrapped past end - loop to start
                let wrapped = new_pos % buffer_frames as f64;
                self.position = wrapped as usize;
                self.fractional_position = wrapped.fract();
            } else {
                self.position = new_pos as usize;
                self.fractional_position = new_pos.fract();
            }
        } else {
            // Non-looping behavior
            if new_pos < 0.0 {
                // Reverse playback reached the start
                self.position = 0;
                self.fractional_position = 0.0;
            } else {
                self.position = new_pos as usize;
                self.fractional_position = new_pos.fract();
            }
        }

        // Return effective frames consumed (absolute value)
        advance.abs().ceil() as usize
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

    /// Voice-level volume (0.0 - 1.0) - current smoothed value
    pub voice_volume: f32,

    /// Target voice volume for smooth ramping (0.0 - 1.0)
    pub target_voice_volume: f32,

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
            target_voice_volume: 1.0,
            channel_map,
        }
    }

    /// Get the combined volume (input volume * voice volume)
    #[allow(dead_code)]
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
    }

    /// Set target voice volume for smooth ramping
    #[allow(dead_code)]
    pub fn set_target_voice_volume(&mut self, target: f32) {
        self.target_voice_volume = target.clamp(0.0, 1.0);
    }

    /// Advance voice volume toward target by one frame.
    /// Uses linear ramping with a fixed rate that provides ~20ms transition at 44.1kHz.
    pub fn advance_voice_volume(&mut self) {
        const RAMP_RATE: f32 = 0.001; // ~22ms for full 0-1 transition at 44.1kHz

        if (self.voice_volume - self.target_voice_volume).abs() < RAMP_RATE {
            self.voice_volume = self.target_voice_volume;
        } else if self.voice_volume < self.target_voice_volume {
            self.voice_volume += RAMP_RATE;
        } else {
            self.voice_volume -= RAMP_RATE;
        }
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
    // Use pitch-corrected path if pitch corrector is enabled
    // Note: pitch correction requires Complete buffers (direct data slice access)
    if sample.pitch_corrector.is_some() && sample.buffer.is_complete() {
        mix_sample_with_pitch_correction(sample, output, frames, output_channels, ducking_multiplier);
        return;
    }

    let speed = sample.speed as f64;
    let is_reverse = speed < 0.0;
    let buffer_frames = sample.buffer.frames();
    let buffer_channels = sample.buffer.channels();
    let loop_mode = sample.loop_mode;
    let sample_volume = sample.volume;

    // Calculate current precise position (integer + fractional parts)
    let mut src_pos = sample.precise_position();

    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops)
        sample.advance_voice_volume();

        // Handle looping wrap-around
        if loop_mode && buffer_frames > 0 {
            if src_pos < 0.0 {
                // Wrap from start to end
                src_pos = src_pos % buffer_frames as f64 + buffer_frames as f64;
            } else if src_pos >= buffer_frames as f64 {
                // Wrap from end to start
                src_pos = src_pos % buffer_frames as f64;
            }
        } else {
            // Check bounds based on direction (non-looping)
            if is_reverse {
                // For reverse playback, stop when we've gone past the start
                if src_pos < 0.0 {
                    break;
                }
            } else {
                // For forward playback, stop when we've reached the end
                if src_pos as usize >= buffer_frames {
                    break;
                }
            }
        }

        let src_frame = src_pos as usize;

        // Safety check: ensure we're within bounds
        if src_frame >= buffer_frames {
            if loop_mode {
                src_pos += speed;
                continue;
            }
            break;
        }

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let base_volume = sample_volume * sample.voice_volume;
        let final_volume = base_volume * fade_multiplier * ducking_multiplier;

        // Calculate fractional part for interpolation
        let frac = (src_pos - src_frame as f64).abs() as f32;

        // Apply channel mapping and mix into output
        for &(src_ch, dest_ch) in &sample.channel_map {
            // Bounds check
            if src_ch >= buffer_channels || dest_ch >= output_channels {
                continue;
            }

            let dest_idx = frame_idx * output_channels + dest_ch;

            // Get current sample value (returns silence for streaming buffers if unavailable)
            let sample_val = sample.buffer.get_sample_or_silence(src_frame, src_ch);

            // Interpolate with adjacent sample if available
            let interpolated_val = if frac > 0.001 {
                if is_reverse {
                    // For reverse, interpolate with previous sample (lower index)
                    // When looping, wrap to end of buffer
                    let prev_frame = if src_frame > 0 {
                        src_frame - 1
                    } else if loop_mode {
                        buffer_frames - 1
                    } else {
                        src_frame // No interpolation possible
                    };
                    if prev_frame != src_frame {
                        let prev_val = sample.buffer.get_sample_or_silence(prev_frame, src_ch);
                        sample_val * (1.0 - frac) + prev_val * frac
                    } else {
                        sample_val
                    }
                } else {
                    // For forward, interpolate with next sample (higher index)
                    // When looping, wrap to start of buffer
                    let next_frame = if src_frame + 1 < buffer_frames {
                        src_frame + 1
                    } else if loop_mode {
                        0
                    } else {
                        src_frame // No interpolation possible
                    };
                    if next_frame != src_frame {
                        let next_val = sample.buffer.get_sample_or_silence(next_frame, src_ch);
                        sample_val * (1.0 - frac) + next_val * frac
                    } else {
                        sample_val
                    }
                }
            } else {
                sample_val
            };

            // Apply loop crossfade by blending samples from end and beginning
            let blended_val = if loop_mode && sample.crossfade_samples > 0 && buffer_frames > sample.crossfade_samples * 2 {
                let cf_samples = sample.crossfade_samples;

                if is_reverse {
                    // Reverse playback: crossfade when approaching start (frame 0)
                    if src_frame < cf_samples {
                        // Blend current position (near start) with end of buffer
                        // progress goes from 0.0 (at cf_samples-1) to 1.0 (at frame 0)
                        let progress = 1.0 - (src_frame as f32 / cf_samples as f32);
                        let blend_frame = buffer_frames - cf_samples + src_frame;
                        let blend_val = sample.buffer.get_sample_or_silence(blend_frame, src_ch);
                        interpolated_val * (1.0 - progress) + blend_val * progress
                    } else {
                        interpolated_val
                    }
                } else {
                    // Forward playback: crossfade when approaching end
                    let crossfade_start = buffer_frames - cf_samples;
                    if src_frame >= crossfade_start {
                        // Blend current position (near end) with start of buffer
                        // progress goes from 0.0 (at crossfade_start) to 1.0 (at buffer_frames-1)
                        let frames_into_crossfade = src_frame - crossfade_start;
                        let progress = frames_into_crossfade as f32 / cf_samples as f32;
                        let blend_frame = frames_into_crossfade;
                        let blend_val = sample.buffer.get_sample_or_silence(blend_frame, src_ch);
                        interpolated_val * (1.0 - progress) + blend_val * progress
                    } else {
                        interpolated_val
                    }
                }
            } else {
                interpolated_val
            };

            // Mix with combined volume and fade applied
            output[dest_idx] += blended_val * final_volume;
        }

        // Advance fade state
        sample.fade_state.advance();

        // Advance source position by speed (negative speed moves backward)
        src_pos += speed;
    }
}

/// Mix a sample with pitch correction using time-stretching.
/// This preserves pitch when speed != 1.0.
/// Note: This function requires a Complete buffer (not streaming).
fn mix_sample_with_pitch_correction(
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    ducking_multiplier: f32,
) {
    // Pitch correction requires direct slice access, only available for Complete buffers
    let decoded_buffer = match sample.buffer.as_complete() {
        Some(buf) => buf,
        None => return, // Streaming buffer - skip pitch correction
    };

    let sample_volume = sample.volume;
    let speed = sample.speed;
    let src_channels = decoded_buffer.channels;

    // Calculate how many input frames we need (scaled by speed)
    let input_frames_needed = ((frames as f32 * speed).ceil() as usize).max(1);

    // Calculate how many input frames are available
    let available_frames = decoded_buffer.frames.saturating_sub(sample.position);
    let input_frames = input_frames_needed.min(available_frames);

    if input_frames == 0 {
        return;
    }

    // Extract input samples from the buffer (interleaved)
    let input_start = sample.position * src_channels;
    let input_end = (sample.position + input_frames) * src_channels;
    let input_slice = &decoded_buffer.data[input_start..input_end];

    // Create output buffer for the stretcher (interleaved, same channel count as source)
    let output_samples = frames * src_channels;
    let mut stretched = vec![0.0f32; output_samples];

    // Process through the pitch corrector
    if let Some(ref mut pc) = sample.pitch_corrector {
        pc.process(input_slice, &mut stretched);
    }

    // Apply volume, fade, ducking and channel mapping
    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops)
        sample.advance_voice_volume();

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let base_volume = sample_volume * sample.voice_volume;
        let final_volume = base_volume * fade_multiplier * ducking_multiplier;

        // Apply channel mapping and mix into output
        for &(src_ch, dest_ch) in &sample.channel_map {
            // Bounds check
            if src_ch >= src_channels || dest_ch >= output_channels {
                continue;
            }

            let src_idx = frame_idx * src_channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            if src_idx < stretched.len() {
                output[dest_idx] += stretched[src_idx] * final_volume;
            }
        }

        // Advance fade state
        sample.fade_state.advance();
    }

    // Note: position is advanced by advance_position() in mix_audio
}

/// Mix a live input (microphone) into the output buffer
fn mix_live_input_into_output(
    input: &mut LiveInput,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    ducking_multiplier: f32,
) {
    let input_channels = input.input_channels;
    let base_volume = input.volume;

    // Read available samples from the ring buffer
    // Process frame by frame to handle underruns gracefully
    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops)
        input.advance_voice_volume();

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

        // Calculate volume per-frame to handle smooth ramping
        let final_volume = base_volume * input.voice_volume * ducking_multiplier;

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
    use crate::audio::types::DecodedBuffer;
    use std::sync::Arc;

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None, false, 0);

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

    // === Loop Tests ===

    #[test]
    fn test_looping_sample_does_not_finish_at_end() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.loop_mode = true;

        // Position at the end of buffer
        sample.position = 4;

        // A looping sample should not be marked as finished even at the end
        assert!(!sample.is_finished());

        // Move past the end
        sample.position = 5;
        assert!(!sample.is_finished());
    }

    #[test]
    fn test_looping_sample_wraps_forward() {
        // Create buffer with identifiable pattern: first frame = 0.1, last frame = 0.9
        let mut data = vec![0.1, 0.1]; // Frame 0
        data.extend(vec![0.2, 0.2]);   // Frame 1
        data.extend(vec![0.3, 0.3]);   // Frame 2
        data.extend(vec![0.4, 0.4]);   // Frame 3
        data.extend(vec![0.5, 0.5]);   // Frame 4
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.loop_mode = true;
        sample.position = 3; // Start near end

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 4 frames - should wrap around to beginning
        let mut output = vec![0.0f32; 8]; // 4 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Frame 0 from position 3 (0.4)
        assert!((output[0] - 0.4).abs() < 0.01);
        // Frame 1 from position 4 (0.5)
        assert!((output[2] - 0.5).abs() < 0.01);
        // Frame 2 from position 0 (wrapped, 0.1)
        assert!((output[4] - 0.1).abs() < 0.01);
        // Frame 3 from position 1 (0.2)
        assert!((output[6] - 0.2).abs() < 0.01);

        // Sample should not be finished
        assert!(!state.active_samples[0].is_finished());
    }

    #[test]
    fn test_looping_sample_finishes_on_fade_out() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.loop_mode = true;

        // Not finished yet
        assert!(!sample.is_finished());

        // Apply a fade out that will complete immediately
        sample.fade_state = FadeState::Out { elapsed: 100, duration: 100 };

        // Now should be finished (fade out complete)
        assert!(sample.is_finished());
    }

    #[test]
    fn test_looping_reverse_wraps_to_end() {
        // Create buffer with identifiable pattern
        let mut data = vec![0.1, 0.1]; // Frame 0
        data.extend(vec![0.2, 0.2]);   // Frame 1
        data.extend(vec![0.3, 0.3]);   // Frame 2
        data.extend(vec![0.4, 0.4]);   // Frame 3
        data.extend(vec![0.5, 0.5]);   // Frame 4
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.loop_mode = true;
        sample.speed = -1.0; // Reverse playback
        sample.position = 1; // Start near beginning

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 4 frames - should wrap around to end
        let mut output = vec![0.0f32; 8]; // 4 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Frame 0 from position 1 (0.2)
        assert!((output[0] - 0.2).abs() < 0.01);
        // Frame 1 from position 0 (0.1)
        assert!((output[2] - 0.1).abs() < 0.01);
        // Frame 2 from position 4 (wrapped from end, 0.5)
        assert!((output[4] - 0.5).abs() < 0.01);
        // Frame 3 from position 3 (0.4)
        assert!((output[6] - 0.4).abs() < 0.01);

        // Sample should not be finished
        assert!(!state.active_samples[0].is_finished());
    }

    #[test]
    fn test_non_looping_sample_finishes_at_end() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        sample.loop_mode = false;

        sample.position = 5;
        assert!(sample.is_finished());
    }

    #[test]
    fn test_loop_mode_defaults_to_false() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());
        assert!(!sample.loop_mode);
    }

    #[test]
    fn test_mono_to_multichannel() {
        // Mono source to 8-channel output
        let data = vec![0.5; 10]; // 10 frames, 1 channel
        let buffer = Arc::new(DecodedBuffer::new(data, 1, 48000));

        // Route mono to channels 4 and 5
        let channel_map = vec![(0, 4), (0, 5)];
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None, false, 0);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None, false, 0);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None, false, 0);

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
        let sample = ActiveSample::new_with_mapping(1, "test".to_string(), buffer, 1.0, 1.0, channel_map, TEST_FILE.to_string(), None, false, 0);

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
        // Set both current and target volume to avoid ramping during test
        live_input.voice_volume = 0.6; // 60% voice volume
        live_input.target_voice_volume = 0.6;

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
            false,
            0,
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
            false,
            0,
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
            false,
            0,
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
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
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
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
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
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames().saturating_sub(1));

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
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames().saturating_sub(1));

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
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
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

        // Without pitch correction: -100 to 100 range
        sample.set_speed(1.5);
        assert_eq!(sample.speed, 1.5);

        sample.set_speed(0.0); // Should clamp to minimum (0.01)
        assert!((sample.speed - 0.01).abs() < 0.001);

        sample.set_speed(150.0); // Should clamp to 100.0
        assert!((sample.speed - 100.0).abs() < 0.001);

        sample.set_speed(-50.0); // Negative for reverse
        assert_eq!(sample.speed, -50.0);

        sample.set_speed(-150.0); // Should clamp to -100.0
        assert!((sample.speed - (-100.0)).abs() < 0.001);
    }

    // === Pitch Correction Tests ===

    #[test]
    fn test_pitch_correction_enable_disable() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        assert!(!sample.has_pitch_correction());

        sample.enable_pitch_correction();
        assert!(sample.has_pitch_correction());

        sample.disable_pitch_correction();
        assert!(!sample.has_pitch_correction());
    }

    #[test]
    fn test_pitch_correction_updates_speed() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        sample.set_speed(2.0);
        sample.enable_pitch_correction();

        // Verify the pitch corrector was created with the current speed
        assert!(sample.pitch_corrector.is_some());
        if let Some(ref pc) = sample.pitch_corrector {
            assert_eq!(pc.speed(), 2.0);
        }

        // Now set speed again and verify pitch corrector is updated
        sample.set_speed(0.5);
        if let Some(ref pc) = sample.pitch_corrector {
            assert_eq!(pc.speed(), 0.5);
        }
    }

    #[test]
    fn test_pitch_correction_mixing() {
        // Create buffer with 200 frames of constant value
        let data = vec![0.5f32; 200 * 2]; // 200 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Enable pitch correction at 2x speed
        sample.set_speed(2.0);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // With pitch correction, at 2x speed, we consume 100 input frames
        // to produce 50 output frames
        // Position should be 100 (the stretcher consumed that much)
        assert_eq!(state.active_samples[0].position, 100);
    }

    #[test]
    fn test_pitch_correction_slow_speed() {
        let data = vec![0.5f32; 100 * 2]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Enable pitch correction at 0.5x speed
        sample.set_speed(0.5);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // At 0.5x speed, we consume 25 input frames to produce 50 output frames
        assert_eq!(state.active_samples[0].position, 25);
    }

    #[test]
    fn test_pitch_correction_output_not_silent() {
        // Ensure the pitch-corrected path produces some output
        // Use a larger buffer because the stretcher has latency
        let data = vec![0.8f32; 48000 * 2]; // 48000 stereo frames (1 second at 48kHz)
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        sample.set_speed(1.5);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Run several passes to warm up the stretcher and accumulate output
        // (stretcher has latency, so first few frames may be silent)
        let mut all_output = Vec::new();
        for _ in 0..20 {
            let mut output = vec![0.0f32; 512 * 2];
            mix_audio(&mut output, &mut state);
            all_output.extend_from_slice(&output);
        }

        // After warming up, output should have some non-zero values
        // (may not be exactly 0.8 due to stretcher processing)
        let has_audio = all_output.iter().any(|&s| s.abs() > 0.01);
        assert!(has_audio, "Output should have audio content");
    }

    // === Reverse Playback Tests ===

    #[test]
    fn test_reverse_playback_basic() {
        // Create buffer with distinct values: frame 0 = 0.1, frame 1 = 0.3, frame 2 = 0.5, etc.
        let mut data = Vec::new();
        for i in 0..10 {
            let val = 0.1 + (i as f32 * 0.1); // 0.1, 0.2, 0.3, ...
            data.push(val); // L
            data.push(val); // R
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Start at end of buffer for reverse playback
        sample.position = 9; // Last frame
        sample.set_speed(-1.0); // Reverse at normal speed

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 5 frames
        let mut output = vec![0.0f32; 10]; // 5 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First output should be from position 9 (value ~1.0)
        assert!((output[0] - 1.0).abs() < 0.1, "First frame should be ~1.0, got {}", output[0]);
        // Position should move backwards
        assert_eq!(state.active_samples[0].position, 4);
    }

    #[test]
    fn test_reverse_playback_finishes_at_start() {
        let data = vec![0.5f32; 20]; // 10 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Start at position 5, play backwards
        sample.position = 5;
        sample.set_speed(-1.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 10 frames (more than we have going backwards)
        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Sample should be finished (reached start)
        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_reverse_double_speed() {
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Start at position 50, play backwards at 2x
        sample.position = 50;
        sample.set_speed(-2.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 20 output frames at -2x speed should consume 40 source frames
        let mut output = vec![0.0f32; 40];
        mix_audio(&mut output, &mut state);

        // Position should be 50 - 40 = 10
        assert_eq!(state.active_samples[0].position, 10);
    }

    #[test]
    fn test_reverse_half_speed() {
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Start at position 50, play backwards at 0.5x
        sample.position = 50;
        sample.set_speed(-0.5);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 20 output frames at -0.5x speed should consume 10 source frames
        let mut output = vec![0.0f32; 40];
        mix_audio(&mut output, &mut state);

        // Position should be 50 - 10 = 40
        assert_eq!(state.active_samples[0].position, 40);
    }

    #[test]
    fn test_reverse_with_volume_and_fade() {
        let data = vec![1.0f32; 20]; // 10 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 0.5, 1.0, TEST_FILE.to_string());

        sample.position = 9;
        sample.set_speed(-1.0);
        sample.set_fade(FadeState::In { elapsed: 5, duration: 10 }); // 50% fade

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 4]; // 2 frames
        mix_audio(&mut output, &mut state);

        // First frame: 1.0 * 0.5 (volume) * 0.5 (fade) = 0.25
        assert!((output[0] - 0.25).abs() < 0.1, "Expected ~0.25, got {}", output[0]);
    }

    #[test]
    fn test_pitch_correction_rejects_negative_speed() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        sample.enable_pitch_correction();

        // Try to set negative speed - should be rejected
        let result = sample.set_speed(-1.0);
        assert!(!result, "set_speed should return false for negative speed with pitch correction");

        // Speed should remain at 1.0 (the default)
        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_pitch_correction_speed_limits() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        sample.enable_pitch_correction();

        // Test lower bound (0.05)
        sample.set_speed(0.01);
        assert!((sample.speed - 0.05).abs() < 0.001);

        // Test upper bound (8.0)
        sample.set_speed(20.0);
        assert!((sample.speed - 8.0).abs() < 0.001);

        // Test normal range
        sample.set_speed(3.5);
        assert_eq!(sample.speed, 3.5);
    }

    #[test]
    fn test_forward_then_reverse() {
        // Test switching between forward and reverse playback
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer.clone(), 1.0, 1.0, TEST_FILE.to_string());

        // Start at position 50
        sample.position = 50;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix forward 10 frames
        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);
        assert_eq!(state.active_samples[0].position, 60);

        // Switch to reverse
        state.active_samples[0].set_speed(-1.0);

        // Mix reverse 10 frames
        mix_audio(&mut output, &mut state);
        assert_eq!(state.active_samples[0].position, 50);
    }

    // === set_speed_with_mode Tests ===
    // These test the actual method that command handling uses

    #[test]
    fn test_set_speed_with_mode_pitch_corrected_to_reverse() {
        // Regression test: using set_speed_with_mode to switch from
        // pitch-corrected to negative speed in a single call
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Start with pitch correction enabled at 1.5x
        sample.set_speed_with_mode(1.5, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 1.5);

        // Switch to reverse with no pitch correction - this is what
        // the command handler does when receiving:
        // {"command": "speed", "message": {"speed": -1.5, "pitch_correction": false}}
        let result = sample.set_speed_with_mode(-1.5, false);

        // Should succeed in a single call
        assert!(result, "set_speed_with_mode should handle pitch-corrected to reverse");
        assert_eq!(sample.speed, -1.5);
        assert!(!sample.has_pitch_correction());
    }

    #[test]
    fn test_set_speed_with_mode_reverse_to_pitch_corrected() {
        // Test switching from reverse playback to pitch-corrected
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Start in reverse
        sample.set_speed_with_mode(-2.0, false);
        assert_eq!(sample.speed, -2.0);
        assert!(!sample.has_pitch_correction());

        // Switch to pitch-corrected
        let result = sample.set_speed_with_mode(1.5, true);

        assert!(result);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 1.5);
    }

    #[test]
    fn test_set_speed_with_mode_negative_with_pitch_correction_rejected() {
        // Attempting negative speed WITH pitch_correction=true should fail
        // (the method enables pitch correction, then rejects negative speed)
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        sample.set_speed(1.0);
        let result = sample.set_speed_with_mode(-1.5, true);

        // Should fail - you can't have negative speed with pitch correction
        assert!(!result);
        // Pitch correction is enabled but speed was rejected
        assert!(sample.has_pitch_correction());
        // Speed remains at previous value
        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_set_speed_with_mode_multiple_transitions() {
        // Test multiple mode transitions
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Forward normal -> Forward pitch-corrected
        sample.set_speed_with_mode(2.0, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 2.0);

        // Forward pitch-corrected -> Reverse normal
        sample.set_speed_with_mode(-1.0, false);
        assert!(!sample.has_pitch_correction());
        assert_eq!(sample.speed, -1.0);

        // Reverse normal -> Forward pitch-corrected
        sample.set_speed_with_mode(0.5, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 0.5);

        // Forward pitch-corrected -> Forward normal
        sample.set_speed_with_mode(3.0, false);
        assert!(!sample.has_pitch_correction());
        assert_eq!(sample.speed, 3.0);
    }

    // === Volume Ramping Tests ===

    #[test]
    fn test_voice_volume_ramping_smooth_transition() {
        // Verify that volume changes happen smoothly frame-by-frame
        let buffer = create_test_buffer(1000, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Change target volume from 1.0 to 0.5
        sample.set_target_voice_volume(0.5);

        // Track volume changes frame-by-frame
        let mut last_volume = sample.voice_volume;
        let mut frame_count = 0;

        // Ramp until we reach target
        while (sample.voice_volume - sample.target_voice_volume).abs() > 0.0001 {
            sample.advance_voice_volume();
            let current = sample.voice_volume;

            // Verify volume changed by at most RAMP_RATE (0.001)
            let delta = (current - last_volume).abs();
            assert!(delta <= 0.0011, "Volume changed too quickly: {} -> {} (delta {})", last_volume, current, delta);

            // Verify monotonic decrease
            assert!(current < last_volume, "Volume should decrease: {} -> {}", last_volume, current);

            last_volume = current;
            frame_count += 1;
        }

        // Should take roughly 500 frames to go from 1.0 to 0.5 (0.5 / 0.001 = 500)
        assert!(frame_count > 400 && frame_count < 600, "Unexpected frame count: {}", frame_count);
        assert!((sample.voice_volume - 0.5).abs() < 0.01, "Final volume should be ~0.5");
    }

    #[test]
    fn test_voice_volume_ramping_rapid_changes() {
        // Verify that rapid volume changes don't cause issues -
        // the system should smoothly transition to the newest target
        let buffer = create_test_buffer(2000, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Change target to 0.5 and start ramping
        sample.set_target_voice_volume(0.5);
        for _ in 0..100 {
            sample.advance_voice_volume();
        }
        // After 100 frames at 0.001/frame = 0.1 change
        // Volume should be 1.0 - 0.1 = 0.9 (still above 0.5)
        let vol_after_first = sample.voice_volume;
        assert!(vol_after_first < 1.0 && vol_after_first > 0.5,
            "Volume {} should be between 0.5 and 1.0", vol_after_first);

        // Interrupt: change target to 0.2 before reaching 0.5
        sample.set_target_voice_volume(0.2);
        assert_eq!(sample.target_voice_volume, 0.2);

        // Continue ramping toward 0.2
        for _ in 0..200 {
            sample.advance_voice_volume();
        }
        // After 200 more frames at 0.001/frame = 0.2 change
        // Volume should be ~0.9 - 0.2 = 0.7
        let vol_after_second = sample.voice_volume;
        assert!(vol_after_second < vol_after_first,
            "Volume should decrease: {} -> {}", vol_after_first, vol_after_second);

        // Interrupt again: change target to 0.9 (going back up)
        sample.set_target_voice_volume(0.9);

        // Volume should start increasing
        let vol_before_third = sample.voice_volume;
        for _ in 0..100 {
            sample.advance_voice_volume();
        }
        assert!(sample.voice_volume > vol_before_third,
            "Volume should increase toward 0.9: {} -> {}", vol_before_third, sample.voice_volume);

        // Key behavior: rapid target changes don't cause discontinuities -
        // the volume always smoothly ramps toward whatever the current target is
    }

    #[test]
    fn test_voice_volume_ramping_in_mix() {
        // Verify that mixing with volume ramping produces smooth output
        let buffer = create_test_buffer(100, 1, 1.0); // Mono, all 1.0 samples
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 1.0, TEST_FILE.to_string());

        // Set target to 0.5 - will ramp during mixing
        sample.set_target_voice_volume(0.5);

        let mut state = MixerState::new(1);
        state.active_samples.push(sample);

        // Mix 50 frames
        let mut output = vec![0.0f32; 50];
        mix_audio(&mut output, &mut state);

        // Verify output ramps smoothly - each sample should be slightly less than previous
        for i in 1..50 {
            let delta = output[i-1] - output[i];
            // Delta should be roughly 0.001 (the ramp rate)
            assert!(delta > 0.0, "Output should decrease frame {} to {}: {} -> {}", i-1, i, output[i-1], output[i]);
            assert!(delta < 0.002, "Output change too large at frame {}: delta = {}", i, delta);
        }

        // First sample should be close to 1.0 (just started ramping)
        assert!(output[0] > 0.99, "First sample should be near 1.0");
        // Last sample should be lower
        assert!(output[49] < output[0], "Last sample should be lower than first");
    }

    #[test]
    fn test_live_input_volume_ramping() {
        // Verify LiveInput also supports smooth volume ramping
        let data = vec![1.0f32; 100];
        let consumer = create_test_ring_buffer_with_data(&data);

        let mut input = LiveInput::new("mic".to_string(), consumer, 1, 1.0, vec![(0, 0)]);

        // Set target volume
        input.set_target_voice_volume(0.3);

        // Verify ramping works
        let initial = input.voice_volume;
        input.advance_voice_volume();
        assert!(input.voice_volume < initial, "Voice volume should decrease toward target");

        // Continue ramping
        for _ in 0..1000 {
            input.advance_voice_volume();
        }
        assert!((input.voice_volume - 0.3).abs() < 0.01, "Should reach target of 0.3");
    }

    #[test]
    fn test_volume_ramping_no_change_when_at_target() {
        // Verify no ramping occurs when current == target
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(1, "test".to_string(), buffer, 1.0, 0.7, TEST_FILE.to_string());

        // Current and target are both 0.7
        assert_eq!(sample.voice_volume, 0.7);
        assert_eq!(sample.target_voice_volume, 0.7);

        // Advancing should not change anything
        let result = sample.advance_voice_volume();
        assert!(!result, "Should return false when at target");
        assert_eq!(sample.voice_volume, 0.7);
    }
}

