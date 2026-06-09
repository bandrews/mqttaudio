// ABOUTME: Soak test driving sustained Play/Stop traffic through the RT callback path.
// ABOUTME: Asserts the voice pool + graveyard keep memory bounded and nothing panics.

use mqttaudio::audio::input::create_ring_buffer;
use mqttaudio::audio::mixer::{
    mix_audio, ActiveSample, StreamedSource, MAX_STREAMED_SOURCES, MAX_VOICES,
};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, drain_commands, graveyard_channel, reap_finished,
    reap_finished_streamed, streamed_graveyard_channel, AudioCallbackState, AudioCommand,
};
use parking_lot::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const SR: u32 = 48000;
const BLOCK: usize = 512;

/// A finite windowed source: a short ring of audio with EOF set, so it plays out over
/// a couple of callbacks and then finishes and is reaped (no producer thread needed).
fn finite_streamed(id: u64, voice: &str) -> StreamedSource {
    let samples = vec![0.2f32; BLOCK * 2 * 2]; // ~2 blocks of stereo audio
    let (mut producer, consumer) = create_ring_buffer(samples.len());
    producer.push_slice(&samples);
    drop(producer);
    StreamedSource::new(
        id,
        voice.to_string(),
        "s.wav".to_string(),
        None,
        consumer,
        2,
        1.0,
        vec![(0, 0), (1, 1)],
        Arc::new(AtomicBool::new(true)), // EOF: finishes once the ring drains
        Arc::new(AtomicBool::new(false)),
    )
}

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
    let (streamed_grave_tx, mut streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cret_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
        output_sample_rate: SR,
    });
    let mut block = vec![0.0f32; BLOCK * 2];

    // Samples ~20 callbacks long, so a few dozen overlap before they finish and reap.
    let sample_frames = BLOCK * 20;
    let mut max_active = 0usize;
    let mut max_streamed = 0usize;

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
        // Interleave windowed sources alongside the full-load samples; each finite
        // source finishes after a couple of callbacks and is reaped through the
        // streamed graveyard, so the streamed pool stays bounded too.
        if i % 3 == 0 {
            let _ = cmd_tx.push(AudioCommand::AddStreamedSource(finite_streamed(
                1_000_000 + i as u64,
                &format!("sv{}", i % 4),
            )));
        }

        {
            let mut guard = callback_state.lock();
            let acs = &mut *guard;
            let AudioCallbackState {
                mixer,
                commands,
                command_returns,
                graveyard,
                streamed_graveyard,
                output_sample_rate,
            } = acs;
            drain_commands(
                commands,
                mixer,
                command_returns,
                graveyard,
                *output_sample_rate,
                64,
            );
            mix_audio(&mut block, mixer);
            reap_finished(mixer, graveyard);
            reap_finished_streamed(mixer, streamed_graveyard);
            max_active = max_active.max(mixer.active_samples.len());
            max_streamed = max_streamed.max(mixer.streamed_sources.len());
        }

        // Control-side reaper: drain the return rings off the RT thread.
        while grave_rx.pop().is_some() {}
        while streamed_grave_rx.pop().is_some() {}
        while cret_rx.pop().is_some() {}
    }

    assert!(
        max_active <= MAX_VOICES,
        "active-sample count exceeded the voice pool: {max_active} > {MAX_VOICES}"
    );
    assert!(
        max_streamed <= MAX_STREAMED_SOURCES,
        "streamed-source count exceeded the pool: {max_streamed} > {MAX_STREAMED_SOURCES}"
    );

    // After the run, the finished samples have drained — the pool is not leaking.
    let resident = callback_state.lock().mixer.active_samples.len();
    assert!(
        resident <= max_active,
        "resident samples {resident} should not exceed the peak {max_active}"
    );
}

#[test]
fn soak_past_the_voice_cap_steals_and_stays_capped() {
    // Sprint 13 F2 (D18): a burst far past MAX_VOICES with no stops must hold the
    // pool exactly at the cap (steal-oldest), route every displaced sample
    // through the graveyard, and never panic. Long samples so nothing finishes
    // on its own during the burst.
    let mut mixer = SceneBuilder::new(2).build();
    mixer.active_samples.reserve(MAX_VOICES);

    let (mut cmd_tx, cmd_rx) = command_channel(1024);
    let (cret_tx, mut cret_rx) = command_return_channel(1024);
    let (grave_tx, mut grave_rx) = graveyard_channel(1024);
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cret_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
        output_sample_rate: SR,
    });
    let mut block = vec![0.0f32; BLOCK * 2];

    let sample_frames = BLOCK * 10_000; // far longer than the run
    let total_plays = MAX_VOICES * 3; // three times over the cap
    let mut displaced = 0usize;

    // One shared decoded buffer (an Arc clone per play), so the soak measures the
    // pool behavior rather than waveform generation.
    let shared = decoded(sine(440.0, SR, sample_frames, 2, 0.1), 2, SR);

    for i in 0..total_plays {
        let s = ActiveSample::new(
            i as u64,
            format!("v{}", i % 8),
            std::sync::Arc::clone(&shared),
            1.0,
            1.0,
            "f".to_string(),
        );
        let _ = cmd_tx.push(AudioCommand::AddSample(s));

        {
            let mut guard = callback_state.lock();
            let acs = &mut *guard;
            let AudioCallbackState {
                mixer,
                commands,
                command_returns,
                graveyard,
                streamed_graveyard,
                output_sample_rate,
            } = acs;
            drain_commands(
                commands,
                mixer,
                command_returns,
                graveyard,
                *output_sample_rate,
                64,
            );
            mix_audio(&mut block, mixer);
            reap_finished(mixer, graveyard);
            reap_finished_streamed(mixer, streamed_graveyard);
            assert!(
                mixer.active_samples.len() <= MAX_VOICES,
                "the pool must never exceed the hard cap"
            );
        }
        while grave_rx.pop().is_some() {
            displaced += 1;
        }
        while cret_rx.pop().is_some() {}
    }

    let guard = callback_state.lock();
    assert_eq!(
        guard.mixer.active_samples.len(),
        MAX_VOICES,
        "the pool holds exactly the cap after a 3x burst"
    );
    // Every play past the cap displaced one earlier sample.
    assert_eq!(
        displaced,
        total_plays - MAX_VOICES,
        "each over-cap play steals exactly one voice"
    );
    // The newest plays are the survivors (oldest were stolen first).
    let min_id = guard
        .mixer
        .active_samples
        .iter()
        .map(|s| s.id)
        .min()
        .unwrap();
    assert_eq!(
        min_id,
        (total_plays - MAX_VOICES) as u64,
        "the oldest non-looping voices were stolen in order"
    );
}
