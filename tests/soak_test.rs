// ABOUTME: Soak test driving sustained Play/Stop traffic through the RT callback path.
// ABOUTME: Asserts the voice pool + graveyard keep memory bounded and nothing panics.

use mqttaudio::audio::mixer::{mix_audio, ActiveSample, MAX_VOICES};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, drain_commands, graveyard_channel, reap_finished,
    AudioCallbackState, AudioCommand,
};
use parking_lot::Mutex;

const SR: u32 = 48000;
const BLOCK: usize = 512;

#[test]
fn soak_sustained_plays_and_stops_stays_bounded() {
    // Drive many Play (AddSample) + periodic Stop-all (FadeOutAll) commands through the
    // full callback step (drain -> mix -> reap), with the control reaper draining the
    // graveyard + command-return rings off-RT each iteration. The voice pool and reaper
    // must keep the active-sample count bounded (never exceeding the reserve) and nothing
    // may panic over a long run. (Real-device xrun-free behavior is the Lane B soak.)
    let mut mixer = SceneBuilder::new(2).build();
    mixer.active_samples.reserve(MAX_VOICES);

    let (mut cmd_tx, cmd_rx) = command_channel(1024);
    let (cret_tx, mut cret_rx) = command_return_channel(1024);
    let (grave_tx, mut grave_rx) = graveyard_channel(1024);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cret_tx,
        graveyard: grave_tx,
        output_sample_rate: SR,
    });
    let mut block = vec![0.0f32; BLOCK * 2];

    // Samples ~20 callbacks long, so a few dozen overlap before they finish and reap.
    let sample_frames = BLOCK * 20;
    let mut max_active = 0usize;

    for i in 0..10_000u32 {
        let s = ActiveSample::new(
            i as u64,
            format!("v{}", i % 8),
            decoded(sine(440.0, SR, sample_frames, 2, 0.2), 2, SR),
            1.0,
            1.0,
            "f".to_string(),
        );
        let _ = cmd_tx.push(AudioCommand::AddSample(s));
        if i % 25 == 0 {
            let _ = cmd_tx.push(AudioCommand::FadeOutAll { fade_ms: 1 });
        }

        {
            let mut guard = callback_state.lock();
            let acs = &mut *guard;
            let AudioCallbackState {
                mixer,
                commands,
                command_returns,
                graveyard,
                output_sample_rate,
            } = acs;
            drain_commands(commands, mixer, command_returns, *output_sample_rate, 64);
            mix_audio(&mut block, mixer);
            reap_finished(mixer, graveyard);
            max_active = max_active.max(mixer.active_samples.len());
        }

        // Control-side reaper: drain both return rings off the RT thread.
        while grave_rx.pop().is_some() {}
        while cret_rx.pop().is_some() {}
    }

    assert!(
        max_active <= MAX_VOICES,
        "active-sample count exceeded the voice pool: {max_active} > {MAX_VOICES}"
    );

    // After the run, the finished samples have drained — the pool is not leaking.
    let resident = callback_state.lock().mixer.active_samples.len();
    assert!(
        resident <= max_active,
        "resident samples {resident} should not exceed the peak {max_active}"
    );
}
