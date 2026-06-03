// ABOUTME: Lane B (real device) smoke test — opens the default output device.
// ABOUTME: Gated behind #[ignore] + MQTTAUDIO_DEVICE_TESTS so Lane A (no device) skips it.

use cpal::traits::{DeviceTrait, StreamTrait};
use mqttaudio::audio::engine::{find_output_config, find_output_device};

/// Open the real default output device and run a brief f32 silence stream.
/// This exercises the device-open + callback path on a real platform without
/// emitting any sound.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn default_output_device_opens_and_runs() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }

    let device = find_output_device(None).expect("a default output device");
    let config = find_output_config(&device, None, Some(48000)).expect("an output config");

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // Silence — exercises the f32 device path without making noise.
                data.fill(0.0);
            },
            move |err| eprintln!("output stream error: {err}"),
            None,
        )
        .expect("build an f32 output stream on the default device");

    stream.play().expect("start the output stream");
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(stream);
}
