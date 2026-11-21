// ABOUTME: Integration tests driving bass management through the real mix_audio path.
// ABOUTME: Closes the Sprint-0 gap where bass management was never exercised in the mix loop.

use mqttaudio::audio::bass_management::{BassManagement, BassManagementConfig};
use mqttaudio::audio::mixer::ActiveSample;
use mqttaudio::audio::test_support::{band_energy, decoded, render, sine, SceneBuilder};

const SR: u32 = 48000;
const CHANNELS: usize = 6;
const LFE: usize = 3;
const CROSSOVER: f32 = 80.0;

/// A sample whose tone is routed only to the two front channels (0 and 1), so
/// the LFE channel (3) starts silent and any energy that lands there must have
/// been put there by bass management's extraction, not by direct 1:1 routing.
fn front_pair_sample(freq: f32, frames: usize, amplitude: f32) -> ActiveSample {
    let buffer = decoded(sine(freq, SR, frames, CHANNELS, amplitude), CHANNELS, SR);
    ActiveSample::new_with_mapping(
        1,
        "bass".to_string(),
        buffer,
        1.0,
        1.0,
        vec![(0, 0), (1, 1)],
        "bass.wav".to_string(),
        None,
        false,
        0,
    )
}

/// Bass management wired to extract from channels 0 and 1 into the LFE (3).
fn bass_management() -> BassManagement {
    let config = BassManagementConfig {
        enabled: true,
        lfe_channel: LFE,
        crossover_frequency_hz: CROSSOVER,
        source_channels: vec![0, 1],
        ..Default::default()
    };
    BassManagement::new(config, SR, CHANNELS)
}

/// Low-frequency content fed through `mix_audio` with bass management enabled
/// must appear as real low-band energy on the LFE channel. This is the
/// regression anchor: it proves the crossover runs inside the production mix
/// loop, not just in `bass_management.rs` unit tests.
#[test]
fn low_frequency_reaches_lfe_through_mix_audio() {
    let frames = 4096;
    let blocks = 8;
    let mut state = SceneBuilder::new(CHANNELS)
        .sample(front_pair_sample(40.0, frames * blocks, 0.5))
        .bass_management(bass_management())
        .build();

    let out = render(&mut state, frames, blocks);

    // LFE must carry the 40 Hz fundamental (below the 80 Hz crossover).
    let lfe_low = band_energy(&out, SR, LFE, CHANNELS, 40.0);
    assert!(
        lfe_low > 0.1,
        "LFE should carry low-band energy through mix_audio, got {lfe_low}"
    );
}

/// High-frequency-only content must leave the LFE channel near silent: the
/// crossover's low-pass rejects it, so almost nothing reaches the subwoofer.
#[test]
fn high_frequency_leaves_lfe_silent_through_mix_audio() {
    let frames = 4096;
    let blocks = 8;
    let mut state = SceneBuilder::new(CHANNELS)
        .sample(front_pair_sample(2000.0, frames * blocks, 0.5))
        .bass_management(bass_management())
        .build();

    let out = render(&mut state, frames, blocks);

    // The 2 kHz tone is far above the 80 Hz crossover; the LFE should be quiet.
    let lfe_high = band_energy(&out, SR, LFE, CHANNELS, 2000.0);
    assert!(
        lfe_high < 0.01,
        "LFE should be near silent for high-frequency content, got {lfe_high}"
    );
}
