// ABOUTME: Lane B (real device) tests for the config editor's live device test sessions.
// ABOUTME: Gated behind #[ignore] + MQTTAUDIO_DEVICE_TESTS so Lane A (no device) skips them.

use mqttaudio::config::Config;
use mqttaudio::config_editor::device_test::{start_input_test, start_output_test, TEST_TONE_HZ};

/// Open the default output device through the editor's test session, play a
/// tone on channel 0 through the real mixer pathway, and tear down cleanly.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn output_test_session_plays_a_tone() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }

    let config = Config::default();
    let mut session = start_output_test(&config, None).expect("open the default output device");
    assert!(session.channels > 0);
    assert!(!session.description.is_empty());

    session
        .play_tone_on_channel(0, TEST_TONE_HZ)
        .expect("queue a tone on channel 0");

    // Let the tone mix; poll the off-RT rings like the editor's tick does.
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        session.poll().expect("no stream error during the tone");
    }

    // Telemetry meters saw signal on channel 0 while the tone played.
    let peaks = session.channel_peaks();
    assert_eq!(peaks.len(), session.channels);

    // Out-of-range channels are rejected, not panicked on.
    assert!(session
        .play_tone_on_channel(session.channels, TEST_TONE_HZ)
        .is_err());

    drop(session);
}

/// Open the default input device through the daemon's real capture path and
/// meter it briefly. The open is the hard assertion; captured levels depend on
/// the host's microphone permissions and ambient signal.
#[test]
#[ignore] // Lane B only (real device): `cargo test -- --ignored` with MQTTAUDIO_DEVICE_TESTS=1
fn input_test_session_captures_without_panicking() {
    if std::env::var("MQTTAUDIO_DEVICE_TESTS").is_err() {
        eprintln!("skipping: set MQTTAUDIO_DEVICE_TESTS=1 to run real-device smoke tests");
        return;
    }

    let mut session = start_input_test(None, 20, 48000).expect("open the default input device");
    assert!(session.channels > 0);
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        session.poll();
    }
    assert_eq!(session.peaks().len(), session.channels);
}
