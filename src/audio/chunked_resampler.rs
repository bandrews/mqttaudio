// ABOUTME: Incremental sample rate conversion for streaming audio.
// ABOUTME: Processes audio in chunks, enabling playback before full file loads.

// Allow dead_code until Phase 10 connects streaming to main.rs.
// This code is tested via integration tests and will be integrated soon.
#![allow(dead_code)]

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::config::ResamplerQuality;

/// Error type for chunked resampling operations
#[derive(Debug)]
pub enum ChunkedResampleError {
    /// Rubato resampling error
    Rubato(rubato::ResampleError),
    /// Error constructing resampler
    Construction(rubato::ResamplerConstructionError),
    /// Invalid configuration
    InvalidConfig(String),
}

impl std::fmt::Display for ChunkedResampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChunkedResampleError::Rubato(e) => write!(f, "Resample error: {}", e),
            ChunkedResampleError::Construction(e) => write!(f, "Construction error: {}", e),
            ChunkedResampleError::InvalidConfig(msg) => write!(f, "Invalid config: {}", msg),
        }
    }
}

impl std::error::Error for ChunkedResampleError {}

impl From<rubato::ResampleError> for ChunkedResampleError {
    fn from(err: rubato::ResampleError) -> Self {
        ChunkedResampleError::Rubato(err)
    }
}

impl From<rubato::ResamplerConstructionError> for ChunkedResampleError {
    fn from(err: rubato::ResamplerConstructionError) -> Self {
        ChunkedResampleError::Construction(err)
    }
}

/// Resampler that processes audio incrementally in fixed-size chunks.
/// Designed for streaming audio where data arrives progressively.
pub struct ChunkedResampler {
    /// The underlying Rubato resampler
    resampler: SincFixedIn<f32>,
    /// Per-channel input accumulator
    input_buffer: Vec<Vec<f32>>,
    /// Number of audio channels
    channels: usize,
    /// Fixed chunk size for processing (in frames)
    chunk_size: usize,
    /// Ratio of output/input sample rates
    ratio: f64,
    /// Whether flush() has been called
    flushed: bool,
    /// Filter startup delay still to be trimmed from the stream's head
    leading_delay_to_skip: usize,
    /// Frames pushed in, for sizing the flush drain
    input_frames_received: u64,
    /// Frames handed out so far
    output_frames_emitted: u64,
}

impl ChunkedResampler {
    /// Default chunk size in frames (1024 = ~21ms at 48kHz)
    pub const DEFAULT_CHUNK_SIZE: usize = 1024;

    /// Create a new chunked resampler.
    ///
    /// # Arguments
    /// * `input_rate` - Source sample rate in Hz
    /// * `output_rate` - Target sample rate in Hz
    /// * `channels` - Number of audio channels
    /// * `quality` - Resampler quality preset
    pub fn new(
        input_rate: u32,
        output_rate: u32,
        channels: usize,
        quality: ResamplerQuality,
    ) -> Result<Self, ChunkedResampleError> {
        Self::with_chunk_size(input_rate, output_rate, channels, quality, Self::DEFAULT_CHUNK_SIZE)
    }

    /// Create a new chunked resampler with custom chunk size.
    ///
    /// # Arguments
    /// * `input_rate` - Source sample rate in Hz
    /// * `output_rate` - Target sample rate in Hz
    /// * `channels` - Number of audio channels
    /// * `quality` - Resampler quality preset
    /// * `chunk_size` - Processing chunk size in frames
    pub fn with_chunk_size(
        input_rate: u32,
        output_rate: u32,
        channels: usize,
        quality: ResamplerQuality,
        chunk_size: usize,
    ) -> Result<Self, ChunkedResampleError> {
        if channels == 0 {
            return Err(ChunkedResampleError::InvalidConfig(
                "Channel count must be > 0".to_string(),
            ));
        }
        if chunk_size == 0 {
            return Err(ChunkedResampleError::InvalidConfig(
                "Chunk size must be > 0".to_string(),
            ));
        }

        let ratio = output_rate as f64 / input_rate as f64;

        let params = SincInterpolationParameters {
            sinc_len: quality.sinc_len(),
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: quality.oversampling_factor(),
            window: WindowFunction::BlackmanHarris2,
        };

        let resampler = SincFixedIn::<f32>::new(
            ratio,
            2.0, // Max relative ratio (allows pitch adjustment)
            params,
            chunk_size,
            channels,
        )?;

        let input_buffer = vec![Vec::new(); channels];
        let leading_delay_to_skip = resampler.output_delay();

        Ok(Self {
            resampler,
            input_buffer,
            channels,
            chunk_size,
            ratio,
            flushed: false,
            leading_delay_to_skip,
            input_frames_received: 0,
            output_frames_emitted: 0,
        })
    }

    /// Push interleaved samples and get resampled output if a chunk is ready.
    ///
    /// Returns None if not enough samples have accumulated for a full chunk.
    /// Returns Some(samples) with interleaved resampled audio when ready.
    pub fn push(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, ChunkedResampleError> {
        if self.flushed {
            return Err(ChunkedResampleError::InvalidConfig(
                "Cannot push after flush".to_string(),
            ));
        }

        if samples.is_empty() {
            return Ok(None);
        }

        self.input_frames_received += (samples.len() / self.channels) as u64;

        // De-interleave input into per-channel buffers
        for frame in samples.chunks(self.channels) {
            for (ch_idx, &sample) in frame.iter().enumerate() {
                if ch_idx < self.input_buffer.len() {
                    self.input_buffer[ch_idx].push(sample);
                }
            }
        }

        // Check if we have enough for a full chunk
        if self.input_buffer[0].len() < self.chunk_size {
            return Ok(None);
        }

        // Process all complete chunks
        let mut output = Vec::new();
        while self.input_buffer[0].len() >= self.chunk_size {
            let chunk_output = self.process_chunk()?;
            output.extend(chunk_output);
        }

        if output.is_empty() {
            Ok(None)
        } else {
            Ok(Some(output))
        }
    }

    /// Process one chunk from the input buffer
    fn process_chunk(&mut self) -> Result<Vec<f32>, ChunkedResampleError> {
        // Extract exactly chunk_size frames from each channel
        let mut chunk_input: Vec<Vec<f32>> = Vec::with_capacity(self.channels);
        for ch_buffer in &mut self.input_buffer {
            let chunk: Vec<f32> = ch_buffer.drain(..self.chunk_size).collect();
            chunk_input.push(chunk);
        }

        // Process through Rubato
        let output_channels = self.resampler.process(&chunk_input, None)?;

        // Re-interleave, trimming the filter's startup delay from the head of
        // the stream so output is time-aligned with the input
        let mut interleaved = self.interleave_output(&output_channels)?;
        if self.leading_delay_to_skip > 0 {
            let frames = interleaved.len() / self.channels;
            let skip = self.leading_delay_to_skip.min(frames);
            interleaved.drain(..skip * self.channels);
            self.leading_delay_to_skip -= skip;
        }
        self.output_frames_emitted += (interleaved.len() / self.channels) as u64;
        Ok(interleaved)
    }

    /// Flush remaining samples at end of stream.
    /// Call this when all input has been provided to get final output.
    pub fn flush(&mut self) -> Result<Vec<f32>, ChunkedResampleError> {
        if self.flushed {
            return Ok(Vec::new());
        }
        self.flushed = true;

        let mut output = Vec::new();

        // Process any remaining complete chunks
        while self.input_buffer[0].len() >= self.chunk_size {
            let chunk_output = self.process_chunk()?;
            output.extend(chunk_output);
        }

        // Feed zero-padded chunks until the filter has surrendered every
        // frame the input covered - the delay trim at the head means the last
        // frames only come out with extra zeroes pushed behind them
        let expected_total = (self.input_frames_received as f64 * self.ratio).round() as u64;
        let mut drain_guard = 0;
        while self.output_frames_emitted < expected_total {
            for ch_buffer in &mut self.input_buffer {
                ch_buffer.resize(self.chunk_size, 0.0);
            }
            output.extend(self.process_chunk()?);

            drain_guard += 1;
            if drain_guard > 64 {
                // The delay is at most a fraction of one chunk; this cannot
                // legitimately take dozens of zero chunks
                tracing::warn!("Resampler flush did not converge; emitting what it produced");
                break;
            }
        }

        // Drop any frames past the input's duration
        if self.output_frames_emitted > expected_total {
            let excess = (self.output_frames_emitted - expected_total) as usize;
            let keep = output.len().saturating_sub(excess * self.channels);
            output.truncate(keep);
            self.output_frames_emitted = expected_total;
        }

        Ok(output)
    }

    /// Re-interleave per-channel output to interleaved format
    fn interleave_output(&self, channels: &[Vec<f32>]) -> Result<Vec<f32>, ChunkedResampleError> {
        if channels.is_empty() {
            return Ok(Vec::new());
        }

        let output_frames = channels[0].len();
        let mut output = Vec::with_capacity(output_frames * self.channels);

        for frame_idx in 0..output_frames {
            for ch in channels {
                output.push(ch.get(frame_idx).copied().unwrap_or(0.0));
            }
        }

        Ok(output)
    }

    /// Get the number of frames currently buffered
    pub fn buffered_frames(&self) -> usize {
        self.input_buffer.get(0).map(|b| b.len()).unwrap_or(0)
    }

    /// Get the chunk size in frames
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Get the resampling ratio (output/input)
    pub fn ratio(&self) -> f64 {
        self.ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunked_output_is_time_aligned_and_full_length() {
        // Same contract as the one-shot resampler: the sinc filter's delay
        // must not shift the stream, and its tail must be drained on flush
        let mut resampler =
            ChunkedResampler::new(44100, 48000, 1, ResamplerQuality::Fast).unwrap();

        let input = vec![1.0f32; 4410];
        let mut out = Vec::new();
        for chunk in input.chunks(512) {
            if let Some(processed) = resampler.push(chunk).unwrap() {
                out.extend(processed);
            }
        }
        out.extend(resampler.flush().unwrap());

        let expected = (4410.0f64 * 48000.0 / 44100.0).round() as usize;
        assert!(
            (out.len() as i32 - expected as i32).abs() <= 2,
            "expected ~{} frames, got {}",
            expected, out.len()
        );
        assert!(out[0] > 0.3, "stream must start at the signal, got {}", out[0]);
        assert!(out[64] > 0.99, "got {}", out[64]);
        assert!(out[out.len() - 64] > 0.99, "the tail must not be dropped, got {}", out[out.len() - 64]);
    }

    #[test]
    fn test_construction_basic() {
        let resampler = ChunkedResampler::new(44100, 48000, 2, ResamplerQuality::Fast);
        assert!(resampler.is_ok());
        let r = resampler.unwrap();
        assert_eq!(r.chunk_size(), ChunkedResampler::DEFAULT_CHUNK_SIZE);
        assert!((r.ratio() - 48000.0 / 44100.0).abs() < 0.0001);
    }

    #[test]
    fn test_construction_invalid_channels() {
        let result = ChunkedResampler::new(44100, 48000, 0, ResamplerQuality::Fast);
        assert!(result.is_err());
    }

    #[test]
    fn test_construction_invalid_chunk_size() {
        let result =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_push_returns_none_until_chunk_ready() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 1024)
                .unwrap();

        // Push less than chunk_size frames
        let samples: Vec<f32> = vec![0.5; 512 * 2]; // 512 stereo frames
        let result = resampler.push(&samples).unwrap();
        assert!(result.is_none());
        assert_eq!(resampler.buffered_frames(), 512);

        // Push more - still not enough
        let samples: Vec<f32> = vec![0.5; 256 * 2]; // 256 more frames
        let result = resampler.push(&samples).unwrap();
        assert!(result.is_none());
        assert_eq!(resampler.buffered_frames(), 768);
    }

    #[test]
    fn test_push_returns_output_when_chunk_ready() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 1024)
                .unwrap();

        // Push exactly chunk_size frames
        let samples: Vec<f32> = vec![0.5; 1024 * 2];
        let result = resampler.push(&samples).unwrap();

        assert!(result.is_some());
        let mut output = result.unwrap();

        // The first chunk emits less than chunk_size * ratio (the filter's
        // startup delay is trimmed from the stream head); flush drains the
        // remainder so push + flush covers the input exactly
        output.extend(resampler.flush().unwrap());
        let expected_frames = (1024.0f64 * 48000.0 / 44100.0).round() as usize;
        let actual_frames = output.len() / 2;

        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() <= 2,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );
    }

    #[test]
    fn test_push_processes_multiple_chunks() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 512)
                .unwrap();

        // Push 3 chunks worth of data
        let samples: Vec<f32> = vec![0.5; 512 * 3 * 2];
        let result = resampler.push(&samples).unwrap();

        assert!(result.is_some());
        let output = result.unwrap();

        // Should have processed 3 chunks
        let expected_frames = (512 * 3) as f64 * 48000.0 / 44100.0;
        let actual_frames = output.len() / 2;

        assert!(
            (actual_frames as f64 - expected_frames).abs() < 100.0,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );

        // Buffer should be empty
        assert_eq!(resampler.buffered_frames(), 0);
    }

    #[test]
    fn test_flush_processes_remaining() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 1024)
                .unwrap();

        // Push less than a chunk
        let samples: Vec<f32> = vec![0.5; 500 * 2];
        let result = resampler.push(&samples).unwrap();
        assert!(result.is_none());

        // Flush should process the remaining
        let flushed = resampler.flush().unwrap();
        assert!(!flushed.is_empty());

        // Output should be approximately 500 * ratio frames
        let expected_frames = (500.0 * 48000.0 / 44100.0) as usize;
        let actual_frames = flushed.len() / 2;

        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() < 100,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );
    }

    #[test]
    fn test_flush_idempotent() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 1024)
                .unwrap();

        let samples: Vec<f32> = vec![0.5; 500 * 2];
        resampler.push(&samples).unwrap();

        let first_flush = resampler.flush().unwrap();
        let second_flush = resampler.flush().unwrap();

        assert!(!first_flush.is_empty());
        assert!(second_flush.is_empty());
    }

    #[test]
    fn test_push_after_flush_fails() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 2, ResamplerQuality::Fast, 1024)
                .unwrap();

        resampler.flush().unwrap();

        let samples: Vec<f32> = vec![0.5; 100 * 2];
        let result = resampler.push(&samples);
        assert!(result.is_err());
    }

    #[test]
    fn test_mono_resampling() {
        let mut resampler =
            ChunkedResampler::with_chunk_size(44100, 48000, 1, ResamplerQuality::Fast, 1024)
                .unwrap();

        let samples: Vec<f32> = vec![0.5; 1024];
        let result = resampler.push(&samples).unwrap();

        assert!(result.is_some());
        let mut output = result.unwrap();
        output.extend(resampler.flush().unwrap());

        // Mono output covers the input duration exactly after the flush drain
        let expected_frames = (1024.0f64 * 48000.0 / 44100.0).round() as usize;
        let actual_frames = output.len(); // mono, so len == frames

        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() <= 2,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );
    }

    #[test]
    fn test_all_quality_presets() {
        for quality in [
            ResamplerQuality::Fast,
            ResamplerQuality::Medium,
            ResamplerQuality::High,
            ResamplerQuality::Maximum,
        ] {
            let result = ChunkedResampler::new(44100, 48000, 2, quality);
            assert!(result.is_ok(), "Failed to create resampler with {:?}", quality);
        }
    }

    #[test]
    fn test_output_matches_full_resampler() {
        use crate::audio::resampler::resample;

        // Generate a test signal (sine wave)
        let input_rate = 44100u32;
        let output_rate = 48000u32;
        let channels = 2;
        let duration_frames = 4096; // Multiple chunks

        let mut input: Vec<f32> = Vec::with_capacity(duration_frames * channels);
        for i in 0..duration_frames {
            let t = i as f32 / input_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            input.push(sample); // Left
            input.push(sample); // Right
        }

        // Full-file resampling
        let full_output = resample(
            input.clone(),
            input_rate,
            output_rate,
            channels,
            ResamplerQuality::Fast,
        )
        .unwrap();

        // Chunked resampling
        let mut chunked = ChunkedResampler::with_chunk_size(
            input_rate,
            output_rate,
            channels,
            ResamplerQuality::Fast,
            1024,
        )
        .unwrap();

        let mut chunked_output = Vec::new();

        // Feed in smaller pieces
        for chunk in input.chunks(512 * channels) {
            if let Some(out) = chunked.push(chunk).unwrap() {
                chunked_output.extend(out);
            }
        }
        chunked_output.extend(chunked.flush().unwrap());

        // Output lengths should be close (within a few frames due to different buffering)
        let full_frames = full_output.len() / channels;
        let chunked_frames = chunked_output.len() / channels;

        assert!(
            (full_frames as i32 - chunked_frames as i32).abs() < 50,
            "Frame count mismatch: full={}, chunked={}",
            full_frames,
            chunked_frames
        );

        // Compare actual sample values (should be very similar)
        let compare_frames = full_frames.min(chunked_frames);
        let mut max_diff: f32 = 0.0;
        let mut sum_diff: f64 = 0.0;

        for i in 0..(compare_frames * channels) {
            let diff = (full_output[i] - chunked_output[i]).abs();
            max_diff = max_diff.max(diff);
            sum_diff += diff as f64;
        }

        let avg_diff = sum_diff / (compare_frames * channels) as f64;

        // Allow small differences due to different buffering strategies
        assert!(
            max_diff < 0.1,
            "Max sample difference too large: {}",
            max_diff
        );
        assert!(
            avg_diff < 0.01,
            "Average sample difference too large: {}",
            avg_diff
        );
    }

    #[test]
    fn test_empty_push() {
        let mut resampler = ChunkedResampler::new(44100, 48000, 2, ResamplerQuality::Fast).unwrap();

        let result = resampler.push(&[]).unwrap();
        assert!(result.is_none());
        assert_eq!(resampler.buffered_frames(), 0);
    }

    #[test]
    fn test_same_sample_rate() {
        // No resampling needed (ratio = 1.0)
        let mut resampler =
            ChunkedResampler::with_chunk_size(48000, 48000, 2, ResamplerQuality::Fast, 512)
                .unwrap();

        let samples: Vec<f32> = vec![0.5; 512 * 2];
        let result = resampler.push(&samples).unwrap();

        assert!(result.is_some());
        let mut output = result.unwrap();
        output.extend(resampler.flush().unwrap());

        // With ratio=1.0, push + flush yields exactly the input length
        let input_frames = 512;
        let output_frames = output.len() / 2;

        assert!(
            (output_frames as i32 - input_frames as i32).abs() <= 2,
            "Expected ~{} frames, got {}",
            input_frames,
            output_frames
        );
    }
}
