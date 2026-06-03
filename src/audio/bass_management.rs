// ABOUTME: Bass management with LFE extraction and crossover filtering.
// ABOUTME: Uses biquad filters to route low frequencies to subwoofer channel.

use std::f32::consts::PI;

/// Biquad filter coefficients for 2nd-order IIR filter
#[derive(Debug, Clone)]
pub struct BiquadCoefficients {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoefficients {
    /// Create low-pass Butterworth filter coefficients
    pub fn lowpass(cutoff_hz: f32, sample_rate: u32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate as f32;
        let cos_omega = omega.cos();
        let sin_omega = omega.sin();
        let alpha = sin_omega / (2.0_f32).sqrt(); // Q = 1/sqrt(2) for Butterworth

        let b0 = (1.0 - cos_omega) / 2.0;
        let b1 = 1.0 - cos_omega;
        let b2 = (1.0 - cos_omega) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_omega;
        let a2 = 1.0 - alpha;

        // Normalize by a0
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Create high-pass Butterworth filter coefficients
    pub fn highpass(cutoff_hz: f32, sample_rate: u32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate as f32;
        let cos_omega = omega.cos();
        let sin_omega = omega.sin();
        let alpha = sin_omega / (2.0_f32).sqrt(); // Q = 1/sqrt(2) for Butterworth

        let b0 = (1.0 + cos_omega) / 2.0;
        let b1 = -(1.0 + cos_omega);
        let b2 = (1.0 + cos_omega) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_omega;
        let a2 = 1.0 - alpha;

        // Normalize by a0
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

/// Biquad filter state (transposed direct form II)
#[derive(Debug, Clone, Default)]
pub struct BiquadFilter {
    coeffs: Option<BiquadCoefficients>,
    z1: f32,
    z2: f32,
}

impl BiquadFilter {
    /// Create a new filter with the given coefficients
    pub fn new(coeffs: BiquadCoefficients) -> Self {
        Self {
            coeffs: Some(coeffs),
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Create an uninitialized filter (passthrough)
    pub fn passthrough() -> Self {
        Self::default()
    }

    /// Process a single sample through the filter
    pub fn process(&mut self, input: f32) -> f32 {
        let coeffs = match &self.coeffs {
            Some(c) => c,
            None => return input, // Passthrough if no coefficients
        };

        // Transposed direct form II
        let output = coeffs.b0 * input + self.z1;
        self.z1 = coeffs.b1 * input - coeffs.a1 * output + self.z2;
        self.z2 = coeffs.b2 * input - coeffs.a2 * output;

        output
    }

    /// Reset filter state (clear delay line)
    #[cfg(test)]
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Configuration for bass management
#[derive(Debug, Clone)]
pub struct BassManagementConfig {
    pub enabled: bool,
    pub lfe_channel: usize,
    pub crossover_frequency_hz: f32,
    pub source_channels: Vec<usize>,
    pub remove_bass_from_sources: bool,
}

impl Default for BassManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lfe_channel: 3, // Standard 5.1 LFE position
            crossover_frequency_hz: 80.0,
            source_channels: Vec::new(),
            remove_bass_from_sources: false,
        }
    }
}

/// Bass management processor
pub struct BassManagement {
    config: BassManagementConfig,
    /// Low-pass filters for extracting bass from each source channel
    lowpass_filters: Vec<BiquadFilter>,
    /// High-pass filters for removing bass from source channels (if enabled)
    highpass_filters: Vec<BiquadFilter>,
    #[allow(dead_code)] // Stored for potential future reconfiguration
    sample_rate: u32,
}

impl BassManagement {
    /// Create a new bass management processor
    pub fn new(config: BassManagementConfig, sample_rate: u32, output_channels: usize) -> Self {
        let lp_coeffs = BiquadCoefficients::lowpass(config.crossover_frequency_hz, sample_rate);
        let hp_coeffs = BiquadCoefficients::highpass(config.crossover_frequency_hz, sample_rate);

        // Create filters for each source channel
        let mut lowpass_filters = Vec::with_capacity(output_channels);
        let mut highpass_filters = Vec::with_capacity(output_channels);

        for ch in 0..output_channels {
            if config.source_channels.contains(&ch) {
                lowpass_filters.push(BiquadFilter::new(lp_coeffs.clone()));
                if config.remove_bass_from_sources {
                    highpass_filters.push(BiquadFilter::new(hp_coeffs.clone()));
                } else {
                    highpass_filters.push(BiquadFilter::passthrough());
                }
            } else {
                lowpass_filters.push(BiquadFilter::passthrough());
                highpass_filters.push(BiquadFilter::passthrough());
            }
        }

        Self {
            config,
            lowpass_filters,
            highpass_filters,
            sample_rate,
        }
    }

    /// Process an interleaved output buffer
    /// Extracts bass from source channels and adds to LFE channel
    pub fn process(&mut self, output: &mut [f32], output_channels: usize) {
        if !self.config.enabled {
            return;
        }

        let frames = output.len() / output_channels;
        let lfe_ch = self.config.lfe_channel;

        // Validate LFE channel is in range
        if lfe_ch >= output_channels {
            return;
        }

        for frame_idx in 0..frames {
            let mut lfe_sum = 0.0_f32;

            // Process each source channel
            for &src_ch in &self.config.source_channels {
                if src_ch >= output_channels || src_ch == lfe_ch {
                    continue;
                }

                let idx = frame_idx * output_channels + src_ch;
                let sample = output[idx];

                // Extract bass for LFE
                let bass = self.lowpass_filters[src_ch].process(sample);
                lfe_sum += bass;

                // Optionally high-pass the source channel
                if self.config.remove_bass_from_sources {
                    output[idx] = self.highpass_filters[src_ch].process(sample);
                }
            }

            // Add extracted bass to LFE channel
            let lfe_idx = frame_idx * output_channels + lfe_ch;
            output[lfe_idx] += lfe_sum;
        }
    }

    /// Reset all filter states
    #[cfg(test)]
    pub fn reset(&mut self) {
        for filter in &mut self.lowpass_filters {
            filter.reset();
        }
        for filter in &mut self.highpass_filters {
            filter.reset();
        }
    }

    /// Get the current configuration
    #[allow(dead_code)] // Available for future configuration queries
    pub fn config(&self) -> &BassManagementConfig {
        &self.config
    }

    /// Get the sample rate
    #[cfg(test)]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Biquad Coefficient Tests ===

    #[test]
    fn test_lowpass_coefficients_valid() {
        let coeffs = BiquadCoefficients::lowpass(100.0, 48000);

        // Coefficients should be finite and reasonable
        assert!(coeffs.b0.is_finite());
        assert!(coeffs.b1.is_finite());
        assert!(coeffs.b2.is_finite());
        assert!(coeffs.a1.is_finite());
        assert!(coeffs.a2.is_finite());

        // For lowpass, b1 should be positive (sum of b0 and b2)
        assert!(coeffs.b1 > 0.0);
    }

    #[test]
    fn test_highpass_coefficients_valid() {
        let coeffs = BiquadCoefficients::highpass(100.0, 48000);

        // Coefficients should be finite and reasonable
        assert!(coeffs.b0.is_finite());
        assert!(coeffs.b1.is_finite());
        assert!(coeffs.b2.is_finite());
        assert!(coeffs.a1.is_finite());
        assert!(coeffs.a2.is_finite());

        // For highpass, b1 should be negative
        assert!(coeffs.b1 < 0.0);
    }

    // === Biquad Filter Tests ===

    #[test]
    fn test_filter_passthrough() {
        let mut filter = BiquadFilter::passthrough();

        // Passthrough should return input unchanged
        assert_eq!(filter.process(0.5), 0.5);
        assert_eq!(filter.process(-0.3), -0.3);
        assert_eq!(filter.process(1.0), 1.0);
    }

    #[test]
    fn test_lowpass_attenuates_high_frequency() {
        let coeffs = BiquadCoefficients::lowpass(100.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        // Generate a high frequency sine wave (1000 Hz, well above 100 Hz cutoff)
        let sample_rate = 48000.0;
        let frequency = 1000.0;
        let num_samples = 1000;

        let mut input_power = 0.0_f32;
        let mut output_power = 0.0_f32;

        // Skip first 100 samples for filter settling
        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * frequency * t).sin();
            let output = filter.process(input);

            if i >= 100 {
                input_power += input * input;
                output_power += output * output;
            }
        }

        // High frequency should be significantly attenuated
        let attenuation_db = 10.0 * (output_power / input_power).log10();
        assert!(attenuation_db < -20.0, "Expected >20dB attenuation, got {:.1}dB", attenuation_db);
    }

    #[test]
    fn test_lowpass_passes_low_frequency() {
        let coeffs = BiquadCoefficients::lowpass(100.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        // Generate a low frequency sine wave (10 Hz, well below 100 Hz cutoff)
        let sample_rate = 48000.0;
        let frequency = 10.0;
        let num_samples = 5000; // Need more samples for low frequency

        let mut input_power = 0.0_f32;
        let mut output_power = 0.0_f32;

        // Skip first 500 samples for filter settling
        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * frequency * t).sin();
            let output = filter.process(input);

            if i >= 500 {
                input_power += input * input;
                output_power += output * output;
            }
        }

        // Low frequency should pass through with minimal attenuation
        let attenuation_db = 10.0 * (output_power / input_power).log10();
        assert!(attenuation_db > -3.0, "Expected <3dB attenuation, got {:.1}dB", attenuation_db);
    }

    #[test]
    fn test_highpass_attenuates_low_frequency() {
        let coeffs = BiquadCoefficients::highpass(100.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        // Generate a low frequency sine wave (10 Hz, well below 100 Hz cutoff)
        let sample_rate = 48000.0;
        let frequency = 10.0;
        let num_samples = 5000;

        let mut input_power = 0.0_f32;
        let mut output_power = 0.0_f32;

        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * frequency * t).sin();
            let output = filter.process(input);

            if i >= 500 {
                input_power += input * input;
                output_power += output * output;
            }
        }

        // Low frequency should be significantly attenuated
        let attenuation_db = 10.0 * (output_power / input_power).log10();
        assert!(attenuation_db < -20.0, "Expected >20dB attenuation, got {:.1}dB", attenuation_db);
    }

    #[test]
    fn test_highpass_passes_high_frequency() {
        let coeffs = BiquadCoefficients::highpass(100.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        // Generate a high frequency sine wave (1000 Hz, well above 100 Hz cutoff)
        let sample_rate = 48000.0;
        let frequency = 1000.0;
        let num_samples = 1000;

        let mut input_power = 0.0_f32;
        let mut output_power = 0.0_f32;

        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * frequency * t).sin();
            let output = filter.process(input);

            if i >= 100 {
                input_power += input * input;
                output_power += output * output;
            }
        }

        // High frequency should pass through with minimal attenuation
        let attenuation_db = 10.0 * (output_power / input_power).log10();
        assert!(attenuation_db > -3.0, "Expected <3dB attenuation, got {:.1}dB", attenuation_db);
    }

    #[test]
    fn test_filter_reset() {
        let coeffs = BiquadCoefficients::lowpass(100.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        // Process some samples to build up state
        for _ in 0..100 {
            filter.process(1.0);
        }

        // Reset and verify state is cleared
        filter.reset();

        // After reset, processing should behave like a fresh filter
        let fresh_coeffs = BiquadCoefficients::lowpass(100.0, 48000);
        let mut fresh_filter = BiquadFilter::new(fresh_coeffs);

        let output1 = filter.process(0.5);
        let output2 = fresh_filter.process(0.5);

        assert!((output1 - output2).abs() < 0.0001);
    }

    // === BassManagement Tests ===

    #[test]
    fn test_bass_management_disabled() {
        let config = BassManagementConfig {
            enabled: false,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: false,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        // Create test buffer (6 channels, 10 frames)
        let mut output = vec![0.5; 60];
        let original = output.clone();

        bm.process(&mut output, 6);

        // Buffer should be unchanged when disabled
        assert_eq!(output, original);
    }

    #[test]
    fn test_bass_management_extracts_to_lfe() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: false,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        // Generate low frequency content on channels 0 and 1
        let sample_rate = 48000.0;
        let frequency = 30.0; // Below crossover
        let frames = 2000;
        let channels = 6;

        let mut output: Vec<f32> = vec![0.0; frames * channels];

        // Fill source channels with low frequency sine
        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            let sample = (2.0 * PI * frequency * t).sin() * 0.5;
            output[frame * channels + 0] = sample; // Left
            output[frame * channels + 1] = sample; // Right
        }

        bm.process(&mut output, channels);

        // LFE channel (3) should now have content
        let mut lfe_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels + 3];
            lfe_power += sample * sample;
        }

        assert!(lfe_power > 0.1, "LFE should have significant content");
    }

    #[test]
    fn test_bass_management_removes_bass_from_sources() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0],
            remove_bass_from_sources: true,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        // Generate low frequency content on channel 0
        let sample_rate = 48000.0;
        let frequency = 30.0;
        let frames = 2000;
        let channels = 6;

        let mut output: Vec<f32> = vec![0.0; frames * channels];

        // Fill source channel with low frequency sine
        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            let sample = (2.0 * PI * frequency * t).sin() * 0.5;
            output[frame * channels + 0] = sample;
        }

        // Measure original power
        let mut original_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels + 0];
            original_power += sample * sample;
        }

        bm.process(&mut output, channels);

        // Source channel should have reduced bass
        let mut filtered_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels + 0];
            filtered_power += sample * sample;
        }

        let attenuation_db = 10.0 * (filtered_power / original_power).log10();
        assert!(attenuation_db < -10.0, "Expected bass reduction, got {:.1}dB", attenuation_db);
    }

    #[test]
    fn test_bass_management_preserves_high_frequency() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0],
            remove_bass_from_sources: true,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        // Generate high frequency content on channel 0
        let sample_rate = 48000.0;
        let frequency = 1000.0; // Well above crossover
        let frames = 1000;
        let channels = 6;

        let mut output: Vec<f32> = vec![0.0; frames * channels];

        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            let sample = (2.0 * PI * frequency * t).sin() * 0.5;
            output[frame * channels + 0] = sample;
        }

        let mut original_power = 0.0_f32;
        for frame in 100..frames {
            let sample = output[frame * channels + 0];
            original_power += sample * sample;
        }

        bm.process(&mut output, channels);

        let mut filtered_power = 0.0_f32;
        for frame in 100..frames {
            let sample = output[frame * channels + 0];
            filtered_power += sample * sample;
        }

        // High frequency should be mostly preserved
        let attenuation_db = 10.0 * (filtered_power / original_power).log10();
        assert!(attenuation_db > -3.0, "High frequency should be preserved, got {:.1}dB attenuation", attenuation_db);
    }

    #[test]
    fn test_bass_management_invalid_lfe_channel() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 10, // Invalid - only 6 channels
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: false,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        let mut output = vec![0.5; 60];
        let original = output.clone();

        // Should not crash, should be no-op
        bm.process(&mut output, 6);

        // Buffer should be unchanged
        assert_eq!(output, original);
    }

    #[test]
    fn test_bass_management_skips_lfe_as_source() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 3], // Includes LFE itself - should be skipped
            remove_bass_from_sources: false,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        let frames = 100;
        let channels = 6;
        let mut output: Vec<f32> = vec![0.0; frames * channels];

        // Put content only on LFE channel
        for frame in 0..frames {
            output[frame * channels + 3] = 0.5;
        }

        bm.process(&mut output, channels);

        // LFE content should be preserved (not doubled by extracting from itself)
        // Allow for filter transient
        let lfe_sample = output[50 * channels + 3];
        assert!((lfe_sample - 0.5).abs() < 0.1, "LFE should be preserved, got {}", lfe_sample);
    }

    #[test]
    fn test_bass_management_adds_to_existing_lfe() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0],
            remove_bass_from_sources: false,
        };

        let mut bm = BassManagement::new(config, 48000, 6);

        let sample_rate = 48000.0;
        let frequency = 30.0;
        let frames = 2000;
        let channels = 6;

        let mut output: Vec<f32> = vec![0.0; frames * channels];

        // Put content on both channel 0 AND LFE
        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            let sample = (2.0 * PI * frequency * t).sin() * 0.3;
            output[frame * channels + 0] = sample;
            output[frame * channels + 3] = 0.2; // Existing LFE content
        }

        bm.process(&mut output, channels);

        // LFE should have both existing content AND extracted bass
        // Measure power in the LFE channel after filter settling
        let mut lfe_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels + 3];
            lfe_power += sample * sample;
        }
        let lfe_rms = (lfe_power / (frames - 500) as f32).sqrt();

        // RMS should be > 0.2 (the DC component alone) because we added bass
        assert!(lfe_rms > 0.2, "LFE should have added bass content, RMS was {}", lfe_rms);
    }
}
