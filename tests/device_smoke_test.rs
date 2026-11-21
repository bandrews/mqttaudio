// ABOUTME: Lane B (real device) smoke test — opens the default output device.
// ABOUTME: Gated behind #[ignore] + MQTTAUDIO_DEVICE_TESTS so Lane A (no device) skips it.

use cpal::traits::StreamTrait;
use mqttaudio::audio::engine::{build_output_stream, find_output_config, find_output_device};
use mqttaudio::audio::mixer::MixerState;
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, graveyard_channel, streamed_graveyard_channel,
    AudioCallbackState,
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
    let output_sample_rate = output_config.stream_config.sample_rate;

    let mixer = MixerState::new(channels);

    // The callback owns the bundled mixer + command consumer + command-return
    // producer + graveyard producer behind one uncontended mutex; an empty mixer
    // produces silence.
    let (_cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Arc::new(Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
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

/// Device-detection round-trip on the real CoreAudio host (Sprint 10, cpal 0.17).
/// Enumerate the default OUTPUT device's reported name, then re-select the device
/// *by that name* through `find_output_device`. This guards the name-based
/// detection path the cpal-0.17 migration moved onto `Device::description()`: the
/// name enumeration reports must round-trip back to a device, or selecting the
/// configured `audio.device` by name would silently fail to detect a present
/// device. A bare `--list-devices` only proves enumeration; this proves the
/// enumerate → select-by-name path that real configs exercise.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn output_device_detection_round_trips_by_name() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let default = host
        .default_output_device()
        .expect("a default output device");
    let name = default
        .description()
        .map(|d| {
            mqttaudio::audio::device::output_device_identifier(&d, cfg!(target_os = "linux"))
                .to_string()
        })
        .expect("the default output device reports a name");
    assert!(
        !name.is_empty(),
        "an enumerated output device name must be non-empty"
    );

    let reselected =
        find_output_device(Some(&name)).expect("the enumerated name must round-trip to a device");
    let reselected_name = reselected
        .description()
        .map(|d| {
            mqttaudio::audio::device::output_device_identifier(&d, cfg!(target_os = "linux"))
                .to_string()
        })
        .expect("the re-selected output device reports a name");
    assert_eq!(
        reselected_name, name,
        "selecting by the enumerated name must resolve to that same device"
    );
}

/// Device-detection round-trip on the real CoreAudio host for INPUT devices
/// (Sprint 10, cpal 0.17). Same guard as the output round-trip, over the
/// `get_input_device` name-matching path used by the live-input feature.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn input_device_detection_round_trips_by_name() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }
    use cpal::traits::{DeviceTrait, HostTrait};
    use mqttaudio::audio::input::get_input_device;

    let host = cpal::default_host();
    let default = match host.default_input_device() {
        Some(d) => d,
        None => {
            eprintln!("skipping: no default input device on this host");
            return;
        }
    };
    let name = default
        .description()
        .map(|d| d.name().to_string())
        .expect("the default input device reports a name");
    assert!(
        !name.is_empty(),
        "an enumerated input device name must be non-empty"
    );

    let reselected =
        get_input_device(Some(&name)).expect("the enumerated name must round-trip to a device");
    let reselected_name = reselected
        .description()
        .map(|d| d.name().to_string())
        .expect("the re-selected input device reports a name");
    assert_eq!(
        reselected_name, name,
        "selecting by the enumerated name must resolve to that same input device"
    );
}
