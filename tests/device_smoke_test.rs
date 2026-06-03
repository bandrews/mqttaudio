// ABOUTME: Lane B (real device) smoke test — opens the default output device.
// ABOUTME: Gated behind #[ignore] + MQTTAUDIO_DEVICE_TESTS so Lane A (no device) skips it.

use cpal::traits::StreamTrait;
use mqttaudio::audio::engine::{build_output_stream, find_output_config, find_output_device};
use mqttaudio::audio::mixer::MixerState;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Open the real default output device and run a brief stream through the actual
/// sample-format dispatch with an empty mixer (silence). Exercises the
/// device-open + typed-callback path on a real platform without making noise.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn default_output_device_opens_and_runs() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }

    let device = find_output_device(None).expect("a default output device");
    let output_config =
        find_output_config(&device, None, Some(48000), 512).expect("an output config");
    let channels = output_config.stream_config.channels as usize;

    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels: channels,
        ducking_engine: None,
        bass_management: None,
    }));
    let active_voices = Arc::new(Mutex::new(HashSet::new()));

    // Build through the real format dispatch; an empty mixer produces silence.
    let stream = build_output_stream(
        &device,
        &output_config.stream_config,
        output_config.sample_format,
        mixer_state,
        active_voices,
    )
    .expect("build an output stream via the format dispatch");

    stream.play().expect("start the output stream");
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(stream);
}
