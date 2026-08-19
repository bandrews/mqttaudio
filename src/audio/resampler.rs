// ABOUTME: Sample rate conversion using rubato.
// ABOUTME: Converts audio to match output device sample rate.

use rubato::{Resampler, ResamplerConstructionError, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use crate::config::ResamplerQuality;

#[derive(Debug)]
pub enum ResampleError {
    RubatoError(rubato::ResampleError),
    ConstructionError(ResamplerConstructionError),
    InvalidInput(String),
}

impl std::fmt::Display for ResampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResampleError::RubatoError(e) => write!(f, "Resample error: {}", e),
            ResampleError::ConstructionError(e) => write!(f, "Resampler construction error: {}", e),
            ResampleError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
        }
    }
}

impl std::error::Error for ResampleError {}

impl From<rubato::ResampleError> for ResampleError {
    fn from(err: rubato::ResampleError) -> Self {
        ResampleError::RubatoError(err)
    }
}

impl From<ResamplerConstructionError> for ResampleError {
    fn from(err: ResamplerConstructionError) -> Self {
        ResampleError::ConstructionError(err)
    }
}

/// Resample audio from one sample rate to another
///
/// # Arguments
/// * `input` - Interleaved PCM samples
/// * `input_rate` - Source sample rate
/// * `output_rate` - Target sample rate
/// * `channels` - Number of audio channels
/// * `quality` - Resampler quality preset (affects speed and audio quality)
///
/// # Returns
/// Resampled interleaved PCM samples
pub fn resample(
    input: Vec<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    quality: ResamplerQuality,
) -> Result<Vec<f32>, ResampleError> {
    // Fast path: no resampling needed
    if input_rate == output_rate {
        tracing::debug!("Sample rates match ({}), no resampling needed", input_rate);
        return Ok(input);
    }

    if input.is_empty() {
        return Ok(input);
    }

    if channels == 0 {
        return Err(ResampleError::InvalidInput("Channel count must be > 0".to_string()));
    }

    tracing::info!(
        "Resampling: {} Hz → {} Hz ({} channels, quality={:?})",
        input_rate,
        output_rate,
        channels,
        quality
    );

    let ratio = output_rate as f64 / input_rate as f64;
    let frames = input.len() / channels;

    // Create resampler with quality-based settings
    let params = SincInterpolationParameters {
        sinc_len: quality.sinc_len(),
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: quality.oversampling_factor(),
        window: WindowFunction::BlackmanHarris2,
    };

    let mut resampler = SincFixedIn::<f32>::new(
        ratio,
        2.0, // Max relative ratio difference (allows 2x speedup/slowdown)
        params,
        frames,
        channels,
    )?;

    // De-interleave input samples
    // rubato expects: Vec<Vec<f32>> where outer vec is channels, inner vec is frames
    let mut input_channels: Vec<Vec<f32>> = vec![vec![0.0; frames]; channels];
    for (frame_idx, frame) in input.chunks(channels).enumerate() {
        for (ch_idx, &sample) in frame.iter().enumerate() {
            input_channels[ch_idx][frame_idx] = sample;
        }
    }

    // Perform resampling, then drain the sinc filter: without the drain the
    // file's tail stays inside the filter history, and without trimming the
    // filter delay the output starts with latency silence and everything is
    // time-shifted by it
    let mut output_channels = resampler.process(&input_channels, None)?;
    let delay = resampler.output_delay();

    let tail: Vec<Vec<f32>> = resampler.process_partial::<Vec<f32>>(None, None)?;
    for (channel, tail_channel) in output_channels.iter_mut().zip(tail) {
        channel.extend(tail_channel);
    }

    // Re-interleave, skipping the delay and keeping exactly the duration the
    // input covered
    let expected_frames = ((frames as f64) * ratio).round() as usize;
    let available = output_channels[0].len();
    let end = (delay + expected_frames).min(available);
    let mut output = Vec::with_capacity(expected_frames * channels);

    for frame_idx in delay..end {
        for ch_idx in 0..channels {
            output.push(output_channels[ch_idx][frame_idx]);
        }
    }

    tracing::debug!(
        "Resampled {} frames → {} frames (ratio: {:.4}, filter delay {} frames)",
        frames,
        output.len() / channels,
        ratio,
        delay
    );

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_resampling_needed() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let result = resample(input.clone(), 44100, 44100, 2, ResamplerQuality::Fast).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_empty_input() {
        let input = vec![];
        let result = resample(input, 44100, 48000, 2, ResamplerQuality::Fast).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_resample_output_is_time_aligned_and_full_length() {
        // The sinc filter has latency: without draining it and trimming the
        // delay, every resampled file starts with near-silence and loses its
        // tail. A DC signal makes both visible.
        let frames = 4410;
        let input = vec![1.0f32; frames];

        let out = resample(input, 44100, 48000, 1, ResamplerQuality::Fast).unwrap();

        let expected = (frames as f64 * 48000.0 / 44100.0).round() as usize;
        assert_eq!(out.len(), expected, "output must cover the whole input duration");

        // Edge frames taper (the filter sees silence beyond the file), but
        // the interior must be at level right away and stay there to the end
        assert!(out[0] > 0.3, "output must start at the signal, not at filter-latency silence, got {}", out[0]);
        assert!(out[64] > 0.99, "the signal should reach level within the filter half-window, got {}", out[64]);
        assert!(out[expected - 64] > 0.99, "the file tail must not be dropped, got {}", out[expected - 64]);
    }

    #[test]
    fn test_upsample_44khz_to_48khz() {
        // Create a simple sine wave at 44.1kHz
        let sample_rate = 44100;
        let duration = 0.1; // 100ms
        let frames = (sample_rate as f32 * duration) as usize;
        let mut input = Vec::with_capacity(frames * 2);

        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            input.push(sample); // Left
            input.push(sample); // Right
        }

        let result = resample(input, 44100, 48000, 2, ResamplerQuality::Fast).unwrap();

        // Output should have ~8.8% more samples
        let expected_frames = (frames as f32 * 48000.0 / 44100.0) as usize;
        let actual_frames = result.len() / 2;

        // The drain-and-trim keeps output within a frame of the exact duration
        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() <= 2,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );
    }

    #[test]
    fn test_downsample_48khz_to_44khz() {
        let sample_rate = 48000;
        let duration = 0.1;
        let frames = (sample_rate as f32 * duration) as usize;
        let mut input = Vec::with_capacity(frames * 2);

        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            input.push(sample);
            input.push(sample);
        }

        let result = resample(input, 48000, 44100, 2, ResamplerQuality::Fast).unwrap();

        // Output should have ~8.2% fewer samples
        let expected_frames = (frames as f32 * 44100.0 / 48000.0) as usize;
        let actual_frames = result.len() / 2;

        // The drain-and-trim keeps output within a frame of the exact duration
        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() <= 2,
            "Expected ~{} frames, got {}",
            expected_frames,
            actual_frames
        );
    }

    #[test]
    fn test_mono_resampling() {
        let frames = 1000;
        let input: Vec<f32> = (0..frames).map(|i| (i as f32) / frames as f32).collect();

        let result = resample(input, 44100, 48000, 1, ResamplerQuality::Fast).unwrap();

        let expected = (1000.0f64 * 48000.0 / 44100.0).round() as usize;
        assert!((result.len() as i32 - expected as i32).abs() <= 2,
            "Got {} frames, expected ~{}", result.len(), expected);
    }

    #[test]
    fn test_invalid_channel_count() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let result = resample(input, 44100, 48000, 0, ResamplerQuality::Fast);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_quality_presets() {
        // Verify all quality presets work correctly
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();

        for quality in [
            ResamplerQuality::Fast,
            ResamplerQuality::Medium,
            ResamplerQuality::High,
            ResamplerQuality::Maximum,
        ] {
            let result = resample(input.clone(), 44100, 48000, 1, quality);
            assert!(result.is_ok(), "Quality {:?} failed", quality);
            let output = result.unwrap();
            assert!(output.len() > 800, "Quality {:?} produced too few samples", quality);
        }
    }
}
