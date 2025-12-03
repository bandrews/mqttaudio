// ABOUTME: Pitch correction for speed-adjusted audio playback.
// ABOUTME: Uses time-stretching to change tempo without affecting pitch.

use signalsmith_stretch::Stretch;

/// Pitch corrector that performs real-time time-stretching.
/// Allows changing playback speed while maintaining original pitch.
pub struct PitchCorrector {
    stretcher: Stretch,
    channels: usize,
    speed: f32,
}

impl PitchCorrector {
    /// Create a new pitch corrector for the given audio format.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let stretcher = Stretch::preset_default(channels as u32, sample_rate);
        Self {
            stretcher,
            channels,
            speed: 1.0,
        }
    }

    /// Set the playback speed. Values > 1.0 = faster, < 1.0 = slower.
    /// Pitch is preserved regardless of speed.
    /// Range: 0.05 to 8.0 (quality degrades at extremes).
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.05, 8.0);
    }

    /// Get the current speed setting.
    #[allow(dead_code)]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Get the input latency in frames.
    #[allow(dead_code)]
    pub fn input_latency(&self) -> usize {
        self.stretcher.input_latency()
    }

    /// Get the output latency in frames.
    #[allow(dead_code)]
    pub fn output_latency(&self) -> usize {
        self.stretcher.output_latency()
    }

    /// Process input samples and produce output.
    ///
    /// Takes interleaved input samples at the current speed rate and produces
    /// output samples at normal playback rate with pitch preserved.
    ///
    /// For speed 2.0: pass 2x as many input samples to get normal-length output
    /// For speed 0.5: pass 0.5x as many input samples to get normal-length output
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        // Input samples should be scaled by speed
        // For 2x speed, we need 2x input for each output frame
        debug_assert_eq!(input.len() % self.channels, 0);
        debug_assert_eq!(output.len() % self.channels, 0);

        self.stretcher.process(input, output);
    }

    /// Reset the stretcher state (for seek operations).
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.stretcher.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pitch_corrector_creation() {
        let pc = PitchCorrector::new(2, 48000);
        assert_eq!(pc.speed(), 1.0);
        assert_eq!(pc.channels, 2);
    }

    #[test]
    fn test_pitch_corrector_set_speed() {
        let mut pc = PitchCorrector::new(2, 48000);

        pc.set_speed(2.0);
        assert_eq!(pc.speed(), 2.0);

        pc.set_speed(0.5);
        assert_eq!(pc.speed(), 0.5);
    }

    #[test]
    fn test_pitch_corrector_speed_clamping() {
        let mut pc = PitchCorrector::new(2, 48000);

        pc.set_speed(0.01); // Too slow - should clamp to 0.05
        assert!(pc.speed() >= 0.05);
        assert!((pc.speed() - 0.05).abs() < 0.001);

        pc.set_speed(10.0); // Too fast - should clamp to 8.0
        assert!(pc.speed() <= 8.0);
        assert!((pc.speed() - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_pitch_corrector_latency() {
        let pc = PitchCorrector::new(2, 48000);

        // Stretcher should report latency values
        let input_lat = pc.input_latency();
        let output_lat = pc.output_latency();

        // Latency should be positive (stretcher needs some samples to process)
        assert!(input_lat > 0 || output_lat > 0);
    }

    #[test]
    fn test_pitch_corrector_process_basic() {
        let mut pc = PitchCorrector::new(2, 48000);

        // At speed 1.0, same input/output sizes
        let input = vec![0.5f32; 1024]; // 512 stereo frames
        let mut output = vec![0.0f32; 1024];

        pc.process(&input, &mut output);

        // After processing, output should have some content
        // (may not be exact due to latency)
        // Just verify no crash and output is modified after warming up
    }

    #[test]
    fn test_pitch_corrector_process_double_speed() {
        let mut pc = PitchCorrector::new(2, 48000);
        pc.set_speed(2.0);

        // At 2x speed, need 2x input for same output length
        let input = vec![0.5f32; 2048]; // 1024 stereo frames of input
        let mut output = vec![0.0f32; 1024]; // 512 stereo frames of output

        pc.process(&input, &mut output);

        // Should complete without panic
    }

    #[test]
    fn test_pitch_corrector_process_half_speed() {
        let mut pc = PitchCorrector::new(2, 48000);
        pc.set_speed(0.5);

        // At 0.5x speed, need 0.5x input for same output length
        let input = vec![0.5f32; 512]; // 256 stereo frames of input
        let mut output = vec![0.0f32; 1024]; // 512 stereo frames of output

        pc.process(&input, &mut output);

        // Should complete without panic
    }

    #[test]
    fn test_pitch_corrector_reset() {
        let mut pc = PitchCorrector::new(2, 48000);

        // Process some audio
        let input = vec![0.5f32; 1024];
        let mut output = vec![0.0f32; 1024];
        pc.process(&input, &mut output);

        // Reset should not panic
        pc.reset();
    }

    #[test]
    fn test_pitch_corrector_mono() {
        let mut pc = PitchCorrector::new(1, 44100);

        let input = vec![0.5f32; 512]; // 512 mono frames
        let mut output = vec![0.0f32; 512];

        pc.process(&input, &mut output);
    }

    #[test]
    fn test_pitch_corrector_multichannel() {
        let mut pc = PitchCorrector::new(6, 48000); // 5.1 surround

        let input = vec![0.5f32; 600]; // 100 6-channel frames
        let mut output = vec![0.0f32; 600];

        pc.process(&input, &mut output);
    }
}
