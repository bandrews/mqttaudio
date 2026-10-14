// ABOUTME: Bass management with LFE extraction and crossover filtering.
// ABOUTME: Uses biquad filters to route low frequencies to subwoofer channel.

use std::f32::consts::PI;

/// Magnitude below which biquad state registers are flushed to zero to keep the
/// decay tail out of the (CPU-expensive) f32 subnormal range. Far below audible
/// levels and well above the subnormal floor (~1.2e-38).
const DENORMAL_THRESHOLD: f32 = 1e-30;

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

/// A 4th-order Linkwitz-Riley section: two identical Butterworth biquads in
/// series (D31). Cascading two Q=1/sqrt(2) Butterworth stages yields the LR4
/// response — 24 dB/oct, -6 dB at the crossover, with the low- and high-pass
/// outputs in phase so their acoustic sum is flat at fc (a single Butterworth
/// biquad is -3 dB and 180 deg out of phase at fc, which notches the recombined
/// response).
#[derive(Debug, Clone, Default)]
pub struct Lr4Filter {
    stages: [BiquadFilter; 2],
}

impl Lr4Filter {
    /// Two identical biquad stages from the same coefficients.
    fn new(coeffs: BiquadCoefficients) -> Self {
        Self {
            stages: [BiquadFilter::new(coeffs.clone()), BiquadFilter::new(coeffs)],
        }
    }

    /// A passthrough section (both stages passthrough).
    fn passthrough() -> Self {
        Self {
            stages: [BiquadFilter::passthrough(), BiquadFilter::passthrough()],
        }
    }

    /// Process one sample through both stages in series.
    fn process(&mut self, input: f32) -> f32 {
        let stage0 = self.stages[0].process(input);
        self.stages[1].process(stage0)
    }
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

        // Flush the decay tail to exact zero before it slips into the denormal
        // range. A long IIR tail after the signal goes quiet otherwise leaves the
        // state as ever-smaller subnormal floats, which are dramatically slower to
        // process on common CPUs — a periodic xrun risk on the audio thread. The
        // threshold sits well above the f32 subnormal floor (~1.2e-38), so the
        // state can never stay denormal; it is far below any audible level, so
        // killing the tail here is inaudible. Pure arithmetic, no allocation.
        if self.z1.abs() < DENORMAL_THRESHOLD {
            self.z1 = 0.0;
        }
        if self.z2.abs() < DENORMAL_THRESHOLD {
            self.z2 = 0.0;
        }

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
    /// Linear trim applied to the (count-normalized) summed LFE (D32). Default 1.0.
    pub lfe_gain: f32,
}

impl Default for BassManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lfe_channel: 3, // Standard 5.1 LFE position
            crossover_frequency_hz: 80.0,
            source_channels: Vec::new(),
            remove_bass_from_sources: true,
            lfe_gain: 1.0,
        }
    }
}

/// Bass management processor
pub struct BassManagement {
    config: BassManagementConfig,
    /// 4th-order Linkwitz-Riley low-pass per channel, extracting bass for the LFE
    lowpass_filters: Vec<Lr4Filter>,
    /// 4th-order Linkwitz-Riley high-pass per channel, removing bass from the
    /// source channels (if enabled)
    highpass_filters: Vec<Lr4Filter>,
    #[allow(dead_code)] // Stored for potential future reconfiguration
    sample_rate: u32,
}

impl BassManagement {
    /// Create a new bass management processor
    pub fn new(config: BassManagementConfig, sample_rate: u32, output_channels: usize) -> Self {
        // Warn once, here at construction, if the LFE channel does not exist on
        // the output device: `process()` runs on the audio thread (must not log
        // per call) and silently no-ops in that case, so without this the bass
        // would be dropped with no operator-visible signal. Only when enabled —
        // a disabled config never engages bass management.
        if config.enabled && config.lfe_channel >= output_channels {
            tracing::warn!(
                "bass management LFE channel {} is out of range for {} output channels; \
                 bass management will be a no-op (no bass redirected to the sub)",
                config.lfe_channel,
                output_channels
            );
        }

        let lp_coeffs = BiquadCoefficients::lowpass(config.crossover_frequency_hz, sample_rate);
        let hp_coeffs = BiquadCoefficients::highpass(config.crossover_frequency_hz, sample_rate);

        // Create filters for each source channel
        let mut lowpass_filters = Vec::with_capacity(output_channels);
        let mut highpass_filters = Vec::with_capacity(output_channels);

        for ch in 0..output_channels {
            if config.source_channels.contains(&ch) {
                lowpass_filters.push(Lr4Filter::new(lp_coeffs.clone()));
                if config.remove_bass_from_sources {
                    highpass_filters.push(Lr4Filter::new(hp_coeffs.clone()));
                } else {
                    highpass_filters.push(Lr4Filter::passthrough());
                }
            } else {
                lowpass_filters.push(Lr4Filter::passthrough());
                highpass_filters.push(Lr4Filter::passthrough());
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

        // Count the source channels that actually contribute this call (the ones
        // that pass the in-range / not-the-LFE guard below), once per call rather
        // than per frame. The summed LFE is then normalized by this count so the
        // sub level is independent of how many sources feed it: two correlated
        // sources no longer sum to +6 dB (D32). An `lfe_gain` trim rides on top.
        let active_sources = self
            .config
            .source_channels
            .iter()
            .filter(|&&src_ch| src_ch < output_channels && src_ch != lfe_ch)
            .count();
        if active_sources == 0 {
            return;
        }
        let lfe_scale = self.config.lfe_gain / active_sources as f32;

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

            // Add the count-normalized, gain-trimmed extracted bass to the LFE.
            let lfe_idx = frame_idx * output_channels + lfe_ch;
            output[lfe_idx] += lfe_sum * lfe_scale;
        }
    }

    /// Get the current configuration
    #[allow(dead_code)] // Available for future configuration queries
    pub fn config(&self) -> &BassManagementConfig {
        &self.config
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
        assert!(
            attenuation_db < -20.0,
            "Expected >20dB attenuation, got {:.1}dB",
            attenuation_db
        );
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
        assert!(
            attenuation_db > -3.0,
            "Expected <3dB attenuation, got {:.1}dB",
            attenuation_db
        );
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
        assert!(
            attenuation_db < -20.0,
            "Expected >20dB attenuation, got {:.1}dB",
            attenuation_db
        );
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
        assert!(
            attenuation_db > -3.0,
            "Expected <3dB attenuation, got {:.1}dB",
            attenuation_db
        );
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

    #[test]
    fn test_filter_flushes_denormals_to_zero() {
        // After an impulse the IIR tail decays geometrically. Without a flush the
        // state lingers as ever-smaller (eventually denormal) values processed
        // every sample on the audio thread. With the flush the tail must terminate
        // at *exactly* 0.0 within a bounded number of samples and stay there (the
        // decaying tail crosses zero transiently, so settling is asserted over a
        // sustained run, not on the first zero).
        let coeffs = BiquadCoefficients::lowpass(80.0, 48000);
        let mut filter = BiquadFilter::new(coeffs);

        filter.process(1.0); // impulse

        // Feed silence past the point the (flushed) tail must have terminated.
        let settle_budget = 16_000;
        for _ in 0..settle_budget {
            filter.process(0.0);
        }

        // The state has been flushed, so the output is now permanently silent —
        // without the flush the geometric tail would still be a tiny denormal.
        for _ in 0..2000 {
            assert_eq!(
                filter.process(0.0),
                0.0,
                "filter tail did not flush to exact zero within {settle_budget} samples"
            );
        }
    }

    // === BassManagement Tests ===

    /// Recombined-magnitude ratio (output/input) of the crossover at a single
    /// frequency: feed a sine into source channel 0 (high-passed into channel 0)
    /// with the low-passed bass routed to the LFE on channel 1, then sum the two
    /// outputs frame-by-frame. For a Linkwitz-Riley crossover the acoustic sum is
    /// flat (ratio ~1.0) across the crossover; a phase-cancelling 2nd-order
    /// Butterworth split notches hard at fc, dropping the ratio far below 1.
    fn crossover_recombined_ratio(freq: f32, crossover_hz: f32) -> f32 {
        let sample_rate = 48000.0_f32;
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 1,
            crossover_frequency_hz: crossover_hz,
            source_channels: vec![0],
            remove_bass_from_sources: true,
            lfe_gain: 1.0,
        };
        let mut bm = BassManagement::new(config, sample_rate as u32, 2);

        let frames = 16000;
        let channels = 2;
        let amplitude = 0.5_f32;
        let mut output = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            output[frame * channels] = (2.0 * PI * freq * t).sin() * amplitude;
        }

        bm.process(&mut output, channels);

        // Sum the high-passed source (ch 0) and the low-passed LFE (ch 1) — the
        // acoustic recombination the crossover is meant to keep flat. Skip the
        // filter settling transient.
        let skip = 4000;
        let mut recombined_power = 0.0_f64;
        for frame in skip..frames {
            let s = output[frame * channels] + output[frame * channels + 1];
            recombined_power += (s as f64) * (s as f64);
        }
        let recombined_rms = (recombined_power / (frames - skip) as f64).sqrt() as f32;
        let input_rms = amplitude / (2.0_f32).sqrt();
        recombined_rms / input_rms
    }

    /// LFE-channel RMS produced by feeding the same correlated low-frequency tone
    /// into every listed source channel. Without count compensation the LFE level
    /// scales with the number of sources (two correlated sources sum to +6 dB);
    /// with D32 normalization it is independent of source count.
    fn lfe_rms_for_sources(source_channels: Vec<usize>) -> f32 {
        let sample_rate = 48000.0_f32;
        let lfe_channel = 5;
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel,
            crossover_frequency_hz: 80.0,
            source_channels: source_channels.clone(),
            remove_bass_from_sources: false,
            lfe_gain: 1.0,
        };
        let mut bm = BassManagement::new(config, sample_rate as u32, 6);

        let frames = 8000;
        let channels = 6;
        let frequency = 30.0;
        let amplitude = 0.5_f32;
        let mut output = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            let t = frame as f32 / sample_rate;
            let s = (2.0 * PI * frequency * t).sin() * amplitude;
            for &ch in &source_channels {
                output[frame * channels + ch] = s;
            }
        }

        bm.process(&mut output, channels);

        let skip = 2000;
        let mut power = 0.0_f64;
        for frame in skip..frames {
            let s = output[frame * channels + lfe_channel];
            power += (s as f64) * (s as f64);
        }
        (power / (frames - skip) as f64).sqrt() as f32
    }

    #[test]
    fn test_lfe_level_independent_of_source_count() {
        let one_source = lfe_rms_for_sources(vec![0]);
        let two_sources = lfe_rms_for_sources(vec![0, 1]);

        // Correlated bass into one vs two sources must yield the same LFE level
        // once normalized by the active source count (D32).
        let ratio_db = 20.0 * (two_sources / one_source).log10();
        assert!(
            ratio_db.abs() <= 1.0,
            "LFE level should be count-independent (<=1 dB), got {ratio_db:.2} dB \
             (1 src rms {one_source:.4}, 2 src rms {two_sources:.4})"
        );
    }

    #[test]
    fn test_crossover_recombines_flat_across_fc() {
        let crossover = 80.0;
        // Bands straddling the crossover, including fc itself where a 2nd-order
        // Butterworth split cancels.
        let freqs = [40.0, 60.0, 80.0, 110.0, 160.0];
        for &freq in &freqs {
            let ratio = crossover_recombined_ratio(freq, crossover);
            let ripple_db = 20.0 * ratio.log10();
            assert!(
                ripple_db.abs() <= 1.0,
                "recombined magnitude at {freq} Hz should be flat (<=1 dB ripple), got {ripple_db:.2} dB (ratio {ratio:.4})"
            );
        }
    }

    #[test]
    fn test_bass_management_disabled() {
        let config = BassManagementConfig {
            enabled: false,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: false,
            lfe_gain: 1.0,
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
            lfe_gain: 1.0,
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
            output[frame * channels] = sample; // Left
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
            lfe_gain: 1.0,
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
            output[frame * channels] = sample;
        }

        // Measure original power
        let mut original_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels];
            original_power += sample * sample;
        }

        bm.process(&mut output, channels);

        // Source channel should have reduced bass
        let mut filtered_power = 0.0_f32;
        for frame in 500..frames {
            let sample = output[frame * channels];
            filtered_power += sample * sample;
        }

        let attenuation_db = 10.0 * (filtered_power / original_power).log10();
        assert!(
            attenuation_db < -10.0,
            "Expected bass reduction, got {:.1}dB",
            attenuation_db
        );
    }

    #[test]
    fn test_bass_management_preserves_high_frequency() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0],
            remove_bass_from_sources: true,
            lfe_gain: 1.0,
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
            output[frame * channels] = sample;
        }

        let mut original_power = 0.0_f32;
        for frame in 100..frames {
            let sample = output[frame * channels];
            original_power += sample * sample;
        }

        bm.process(&mut output, channels);

        let mut filtered_power = 0.0_f32;
        for frame in 100..frames {
            let sample = output[frame * channels];
            filtered_power += sample * sample;
        }

        // High frequency should be mostly preserved
        let attenuation_db = 10.0 * (filtered_power / original_power).log10();
        assert!(
            attenuation_db > -3.0,
            "High frequency should be preserved, got {:.1}dB attenuation",
            attenuation_db
        );
    }

    #[test]
    fn test_bass_management_invalid_lfe_channel() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 10, // Invalid - only 6 channels
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: false,
            lfe_gain: 1.0,
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
            lfe_gain: 1.0,
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
        assert!(
            (lfe_sample - 0.5).abs() < 0.1,
            "LFE should be preserved, got {}",
            lfe_sample
        );
    }

    #[test]
    fn test_bass_management_adds_to_existing_lfe() {
        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3,
            crossover_frequency_hz: 80.0,
            source_channels: vec![0],
            remove_bass_from_sources: false,
            lfe_gain: 1.0,
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
            output[frame * channels] = sample;
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
        assert!(
            lfe_rms > 0.2,
            "LFE should have added bass content, RMS was {}",
            lfe_rms
        );
    }

    // === Construction-time warning ===

    use std::sync::{Arc, Mutex};
    use tracing::Level;

    /// Captures WARN-and-above event messages into a shared buffer so a test can
    /// assert on emitted log output without it leaking to the console.
    struct CaptureLayer {
        warnings: Arc<Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for CaptureLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() > Level::WARN {
                return;
            }
            let mut message = String::new();
            event.record(&mut CaptureVisitor {
                message: &mut message,
            });
            self.warnings.lock().unwrap().push(message);
        }
    }

    struct CaptureVisitor<'a> {
        message: &'a mut String,
    }

    impl tracing::field::Visit for CaptureVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.message = format!("{value:?}");
            }
        }
    }

    #[test]
    fn test_out_of_range_lfe_warns_once_at_construction() {
        use tracing_subscriber::layer::SubscriberExt;

        let warnings = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            warnings: warnings.clone(),
        });

        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 8, // out of range for a 4-channel device
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: true,
            lfe_gain: 1.0,
        };

        tracing::subscriber::with_default(subscriber, || {
            let _bm = BassManagement::new(config, 48000, 4);
        });

        let warnings = warnings.lock().unwrap();
        assert_eq!(
            warnings.len(),
            1,
            "construction with an out-of-range LFE should warn exactly once, got {warnings:?}"
        );
        let msg = &warnings[0];
        assert!(
            msg.contains("bass management") && msg.contains('8') && msg.contains('4'),
            "warning should name bass management and the out-of-range channels, got {msg:?}"
        );
    }

    #[test]
    fn test_in_range_lfe_does_not_warn() {
        use tracing_subscriber::layer::SubscriberExt;

        let warnings = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            warnings: warnings.clone(),
        });

        let config = BassManagementConfig {
            enabled: true,
            lfe_channel: 3, // in range for a 6-channel device
            crossover_frequency_hz: 80.0,
            source_channels: vec![0, 1],
            remove_bass_from_sources: true,
            lfe_gain: 1.0,
        };

        tracing::subscriber::with_default(subscriber, || {
            let _bm = BassManagement::new(config, 48000, 6);
        });

        assert!(
            warnings.lock().unwrap().is_empty(),
            "an in-range LFE must not warn at construction"
        );
    }
}
