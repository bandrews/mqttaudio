// ABOUTME: Lane B (real device) smoke test — opens the default output device.
// ABOUTME: Gated behind #[ignore] + MQTTAUDIO_DEVICE_TESTS so Lane A (no device) skips it.

use cpal::traits::StreamTrait;
use mqttaudio::audio::engine::{build_output_stream, find_output_config, find_output_device};
use mqttaudio::audio::mixer::MixerState;
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, graveyard_channel, AudioCallbackState,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

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
    let output_sample_rate = output_config.stream_config.sample_rate.0;

    let mixer = MixerState::new(channels);

    // The callback owns the bundled mixer + command consumer + command-return
    // producer + graveyard producer behind one uncontended mutex; an empty mixer
    // produces silence.
    let (_cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let callback_state = Arc::new(Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        output_sample_rate,
    }));
    let xruns = Arc::new(AtomicU64::new(0));

    // Build through the real format dispatch; an empty mixer produces silence.
    let stream = build_output_stream(
        &device,
        &output_config.stream_config,
        output_config.sample_format,
        callback_state,
        xruns,
        Arc::new(AtomicBool::new(false)),
    )
    .expect("build an output stream via the format dispatch");

    stream.play().expect("start the output stream");
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(stream);
}

/// Open the real default INPUT device and run the capture path briefly. Exercises
/// the device-open + typed-format dispatch + async-SRC capture callback on a real
/// platform (Sprint 8). The open + channel count is the hard assertion; the
/// captured sample count is informational only, because a host may deny microphone
/// access or be silent, which would legitimately yield zero captured frames.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn default_input_device_opens_and_captures() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }

    use mqttaudio::audio::input::{create_input_stream, InputStreamConfig};

    let mut active = match create_input_stream(InputStreamConfig::default(), 48000) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping: no usable default input device ({e:?})");
            return;
        }
    };
    assert!(
        active.channels >= 1,
        "a default input device should expose at least one channel"
    );
    let consumer = active
        .take_consumer()
        .expect("the input stream should hand over its ring consumer");

    // Let the real capture callback run and push through the async SRC into the ring.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let captured = consumer.len();
    eprintln!("input smoke: captured {captured} samples in 300ms (informational)");
    drop(active);
}
