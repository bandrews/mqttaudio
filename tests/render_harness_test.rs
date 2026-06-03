// ABOUTME: Seed tests for the offline render harness (src/audio/test_support.rs).
// ABOUTME: Exercise additive mixing and faded decay, and the analysis helpers.

use mqttaudio::audio::mixer::{ActiveSample, FadeState};
use mqttaudio::audio::test_support::{
    band_energy, decoded, max_inter_sample_delta, peak, render, rms, sine, SceneBuilder,
};

const SR: u32 = 48000;

#[test]
fn two_in_phase_tones_sum_additively() {
    let frames = 4096;
    let amp = 0.25;
    let freq = 480.0;
    let buffer = decoded(sine(freq, SR, frames, 2, amp), 2, SR);

    let s1 = ActiveSample::new(
        1,
        "a".to_string(),
        buffer.clone(),
        1.0,
        1.0,
        "t.wav".to_string(),
    );
    let s2 = ActiveSample::new(
        2,
        "b".to_string(),
        buffer.clone(),
        1.0,
        1.0,
        "t.wav".to_string(),
    );
    let mut state = SceneBuilder::new(2).sample(s1).sample(s2).build();

    let out = render(&mut state, frames, 1);

    // Two identical in-phase tones sum to 2x amplitude (0.5), well under the clamp.
    let measured = rms(&out, 0, 2);
    let expected = (2.0 * amp) / 2.0_f32.sqrt(); // RMS of a 0.5-amplitude sine
    assert!(
        (measured - expected).abs() < 0.02,
        "additive RMS: measured {measured}, expected {expected}"
    );

    let pk = peak(&out);
    assert!(pk > 0.45 && pk <= 0.5 + 1e-3, "peak {pk} should be ~0.5");

    // Energy concentrated at the tone and effectively absent two octaves up.
    assert!(
        band_energy(&out, SR, 0, 2, freq) > 0.4,
        "tone band should be strong"
    );
    assert!(
        band_energy(&out, SR, 0, 2, freq * 4.0) < 0.05,
        "off-tone band should be quiet"
    );
}

#[test]
fn fade_out_decays_monotonically_without_clicks() {
    let block = 512;
    let blocks = 12;
    let total = block * blocks;
    let amp = 0.5;
    let freq = 440.0;

    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "t.wav".to_string());
    // Fade-out spanning the entire render.
    let fade_ms = (total as f32 / SR as f32 * 1000.0) as u32;
    sample.set_fade(FadeState::fade_out(fade_ms, SR));
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let out = render(&mut state, block, blocks);

    // Per-block RMS must be non-increasing (a smooth decay, no resurgence).
    let channels = 2;
    let mut prev = f32::INFINITY;
    for b in 0..blocks {
        let start = b * block * channels;
        let end = start + block * channels;
        let r = rms(&out[start..end], 0, channels);
        assert!(r <= prev + 1e-3, "block {b} RMS rose: {r} > {prev}");
        prev = r;
    }
    assert!(
        prev < 0.1,
        "tail should have decayed, last-block RMS {prev}"
    );

    // No click: the largest sample-to-sample step stays within the tone's own
    // slope (a fade is far slower than the carrier), so no discontinuity.
    let natural_slope = amp * std::f32::consts::TAU * freq / SR as f32;
    let delta = max_inter_sample_delta(&out, 0, channels);
    assert!(
        delta < natural_slope * 2.0 + 0.01,
        "inter-sample delta {delta} exceeds smooth-fade bound"
    );
}
