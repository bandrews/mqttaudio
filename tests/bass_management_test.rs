// ABOUTME: Integration tests driving bass management through the real mix_audio path.
// ABOUTME: Closes the Sprint-0 gap where bass management was never exercised in the mix loop.

use mqttaudio::audio::bass_management::{BassManagement, BassManagementConfig};
use mqttaudio::audio::input::create_ring_buffer;
use mqttaudio::audio::mixer::{ActiveSample, LiveInput};
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

/// Bass management over a 5.1 layout: the five mains feed the LFE (3).
fn five_one_bass_management() -> BassManagement {
    let config = BassManagementConfig {
        enabled: true,
        lfe_channel: LFE,
        crossover_frequency_hz: CROSSOVER,
        source_channels: vec![0, 1, 2, 4, 5],
        ..Default::default()
    };
    BassManagement::new(config, SR, CHANNELS)
}

const TONE_HZ: f32 = 40.0;
const BLOCK: usize = 4096;
const BLOCKS: usize = 8;

/// A mono 40 Hz tone played on `mains`.
fn tone_on(id: u64, mains: &[usize], amplitude: f32) -> ActiveSample {
    let buffer = decoded(sine(TONE_HZ, SR, BLOCK * BLOCKS, 1, amplitude), 1, SR);
    ActiveSample::new_with_mapping(
        id,
        "bass".to_string(),
        buffer,
        1.0,
        1.0,
        mains.iter().map(|&main| (0, main)).collect(),
        "tone.wav".to_string(),
        None,
        false,
        0,
    )
}

/// A mono live input carrying a 40 Hz tone, routed to `mains`.
fn input_on(mains: &[usize], amplitude: f32) -> LiveInput {
    let frames = BLOCK * BLOCKS;
    let (mut producer, consumer) = create_ring_buffer(frames * 2);
    producer.push_slice(&sine(TONE_HZ, SR, frames, 1, amplitude));
    LiveInput::new(
        0,
        "mic".to_string(),
        consumer,
        1,
        1.0,
        mains.iter().map(|&main| (0, main)).collect(),
        frames * 2,
    )
}

/// The 40 Hz level on the subwoofer once `scene` has played through 5.1 bass
/// management.
fn subwoofer_level(scene: SceneBuilder) -> f32 {
    let mut state = scene.bass_management(five_one_bass_management()).build();
    let out = render(&mut state, BLOCK, BLOCKS);
    band_energy(&out, SR, LFE, CHANNELS, TONE_HZ)
}

/// The subwoofer level of a tone of `amplitude`: its level on a main without bass
/// management, through the 4th-order Linkwitz-Riley low-pass, which passes 40 Hz
/// at about 0.94 with an 80 Hz crossover.
fn expected_level(amplitude: f32) -> f32 {
    let mut state = SceneBuilder::new(CHANNELS)
        .sample(tone_on(1, &[0], amplitude))
        .build();
    let out = render(&mut state, BLOCK, BLOCKS);
    band_energy(&out, SR, 0, CHANNELS, TONE_HZ) * 0.94
}

fn assert_level(what: &str, level: f32, expected: f32) {
    assert!(
        (level - expected).abs() < expected * 0.1,
        "{what}: subwoofer at {level}, expected about {expected}"
    );
}

#[test]
fn a_sound_reaches_the_subwoofer_at_its_own_level_however_many_mains_play_it() {
    let one_main = subwoofer_level(SceneBuilder::new(CHANNELS).sample(tone_on(1, &[0], 0.4)));
    assert_level("one main", one_main, expected_level(0.4));

    let every_main =
        subwoofer_level(SceneBuilder::new(CHANNELS).sample(tone_on(1, &[0, 1, 2, 4, 5], 0.4)));
    assert_level("every main", every_main, expected_level(0.4));
}

#[test]
fn different_sounds_on_different_mains_add_up_on_the_subwoofer() {
    let level = subwoofer_level(
        SceneBuilder::new(CHANNELS)
            .sample(tone_on(1, &[0], 0.2))
            .sample(tone_on(2, &[1], 0.2)),
    );
    assert_level("two sounds", level, expected_level(0.4));
}

#[test]
fn a_live_input_reaches_the_subwoofer_at_its_own_level() {
    let level =
        subwoofer_level(SceneBuilder::new(CHANNELS).live_input(input_on(&[0, 1, 4, 5], 0.4)));
    assert_level("every route", level, expected_level(0.4));

    // A talkback destination plays the input on some of its routes only.
    let mut input = input_on(&[0, 1, 4, 5], 0.4);
    input.set_routing(0b11, 1.0);
    let level = subwoofer_level(SceneBuilder::new(CHANNELS).live_input(input));
    assert_level("two of four routes", level, expected_level(0.4));
}

#[test]
fn a_sound_routed_quieter_reaches_the_subwoofer_quieter() {
    let mut sound = tone_on(1, &[0, 1], 0.4);
    sound.set_channel_route_gains(vec![0.5, 0.5]);
    let level = subwoofer_level(SceneBuilder::new(CHANNELS).sample(sound));
    assert_level("half-gain routes", level, expected_level(0.2));
}
