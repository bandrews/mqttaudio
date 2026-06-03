// ABOUTME: Sample rate conversion using rubato.
// ABOUTME: Converts audio to match output device sample rate.

use crate::config::ResamplerQuality;
use rubato::{
    Resampler, ResamplerConstructionError, SincFixedIn, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

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
        return Err(ResampleError::InvalidInput(
            "Channel count must be > 0".to_string(),
        ));
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
        ratio, 2.0, // Max relative ratio difference (allows 2x speedup/slowdown)
        params, frames, channels,
    )?;

    // De-interleave input samples
    // rubato expects: Vec<Vec<f32>> where outer vec is channels, inner vec is frames
    let mut input_channels: Vec<Vec<f32>> = vec![vec![0.0; frames]; channels];
    for (frame_idx, frame) in input.chunks(channels).enumerate() {
        for (ch_idx, &sample) in frame.iter().enumerate() {
            input_channels[ch_idx][frame_idx] = sample;
        }
    }

    // Perform resampling
    let output_channels = resampler.process(&input_channels, None)?;

    // Re-interleave output samples
    let output_frames = output_channels[0].len();
    let mut output = Vec::with_capacity(output_frames * channels);

    for frame_idx in 0..output_frames {
        for ch_buf in &output_channels[..channels] {
            output.push(ch_buf[frame_idx]);
        }
    }

    tracing::debug!(
        "Resampled {} frames → {} frames (ratio: {:.4})",
        frames,
        output_frames,
        ratio
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

        // Allow tolerance for resampler latency/buffering (~200 frames = 4ms at 48kHz)
        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() < 200,
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

        // Allow tolerance for resampler latency/buffering (~200 frames = 4.5ms at 44.1kHz)
        assert!(
            (actual_frames as i32 - expected_frames as i32).abs() < 200,
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

        // Mono should work correctly - expected ~1088 frames with some latency
        // Allow wider bounds due to resampler buffering
        assert!(
            result.len() > 800,
            "Got {} frames, expected >800",
            result.len()
        );
        assert!(
            result.len() < 1300,
            "Got {} frames, expected <1300",
            result.len()
        );
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
            assert!(
                output.len() > 800,
                "Quality {:?} produced too few samples",
                quality
            );
        }
    }
}
