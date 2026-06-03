// ABOUTME: Proves a panic while holding the mixer lock doesn't brick the audio path.
// ABOUTME: parking_lot does not poison, so the callback can still lock and mix after.

use mqttaudio::audio::mixer::{mix_audio, MixerState};
use parking_lot::Mutex;
use std::sync::Arc;

#[test]
fn panic_while_holding_mixer_lock_does_not_brick_the_callback() {
    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels: 2,
        ducking_applier: None,
        bass_management: None,
    }));

    // A thread panics while holding the mixer guard. With std::sync::Mutex this
    // would poison the lock and the next `.lock().unwrap()` (including the audio
    // callback's) would panic; with parking_lot the guard drops on unwind and the
    // lock stays usable.
    let poisoner = Arc::clone(&mixer_state);
    let handle = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("simulated handler panic while holding the mixer lock");
    });
    assert!(
        handle.join().is_err(),
        "the poisoner thread should have panicked"
    );

    // The audio-callback path can still acquire the lock and mix without panicking.
    let mut output = vec![0.0f32; 256 * 2];
    {
        let mut state = mixer_state.lock();
        mix_audio(&mut output, &mut state);
    }
    // Reaching here without a panic is the point; an empty scene mixes to silence.
    assert!(output.iter().all(|&s| s == 0.0));
}
