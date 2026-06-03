// ABOUTME: Allocation-counting harness proving the mix path does no heap work per block.
// ABOUTME: A dedicated test binary with a counting global allocator; arms around mix_audio.

use mqttaudio::audio::ducking::{DuckTargetChange, DuckingApplier};
use mqttaudio::audio::mixer::{mix_audio, ActiveSample};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use mqttaudio::mqtt::commands::SampleSelector;
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, drain_commands, graveyard_channel, reap_finished,
    AudioCallbackState, AudioCommand,
};
use parking_lot::Mutex;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    // Const-initialized so accessing it never itself allocates (no lazy heap init).
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Passthrough allocator that counts (de)allocations only while the current
/// thread is "armed". Disarmed it is a thin wrapper over the system allocator.
struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.with(|a| a.get()) {
            DEALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Serializes the armed counting regions. The `ARMED` flag is thread-local but the
/// `ALLOCS`/`DEALLOCS` counters are global, so two tests counting concurrently (the
/// default multi-threaded test runner) would contaminate each other's totals. This
/// lock makes the count regions mutually exclusive.
static COUNT_LOCK: Mutex<()> = Mutex::new(());

/// Count allocations and frees made on this thread while `f` runs.
fn count_allocs<F: FnOnce()>(f: F) -> (usize, usize) {
    let _guard = COUNT_LOCK.lock();
    ALLOCS.store(0, Ordering::Relaxed);
    DEALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    (
        ALLOCS.load(Ordering::Relaxed),
        DEALLOCS.load(Ordering::Relaxed),
    )
}

const SR: u32 = 48000;
const BLOCK: usize = 512;

#[test]
fn mix_path_is_allocation_free_after_warmup() {
    // A representative scene: a plain sample plus a looping sample, mixed to a
    // stereo bus. (The pitch path is covered separately below.)
    let plain = ActiveSample::new(
        1,
        "a".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let looping = ActiveSample::new_with_id(
        2,
        "b".to_string(),
        decoded(sine(330.0, SR, 4800, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "b".to_string(),
        None,
        true,
        0,
    );
    let mut state = SceneBuilder::new(2).sample(plain).sample(looping).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up off the armed region (first block may touch lazily-sized buffers).
    mix_audio(&mut block, &mut state);

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "mix path allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "mix path freed {deallocs} times across 8 blocks"
    );
}

#[test]
fn fractional_speed_and_route_gain_mix_is_allocation_free() {
    // Exercises the cubic interpolation path (a non-integer speed makes frac != 0 on
    // every frame, so cubic_taps/cubic_interpolate run, D27) AND the per-route
    // downmix gain path (a custom channel map with route gains, D29). Both must add
    // no heap work on the audio thread: cubic is pure arithmetic over the borrowed
    // buffer, and the route gain is a Vec lookup set off-RT at construction.
    let mut fractional = ActiveSample::new_with_mapping(
        1,
        "a".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        vec![(0, 0), (1, 1), (0, 1)], // a downmix route (src 0 -> dest 1) to gain
        "a".to_string(),
        None,
        false,
        0,
    );
    // Non-integer speed: frac sweeps, so the cubic branch runs each frame.
    assert!(fractional.set_speed(0.618));
    // Per-route gains (the Vec is allocated here, off the armed region below).
    fractional.set_channel_route_gains(vec![0.5, 0.5, 0.5]);

    let mut state = SceneBuilder::new(2).sample(fractional).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up off the armed region (first block may touch lazily-sized buffers).
    mix_audio(&mut block, &mut state);

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "cubic/route-gain mix path allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "cubic/route-gain mix path freed {deallocs} times across 8 blocks"
    );
}

#[test]
fn pitch_mix_path_is_allocation_free_after_warmup() {
    // The pitch-correction path used to heap-allocate a stretcher output buffer
    // every block (F5-4). With the pre-allocated scratch it must not allocate in
    // steady state.
    let mut pitched = ActiveSample::new(
        1,
        "p".to_string(),
        decoded(sine(220.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "p".to_string(),
    );
    pitched.enable_pitch_correction();
    pitched.set_speed(0.7);
    let mut state = SceneBuilder::new(2).sample(pitched).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up: sizes the pitch scratch and primes the stretcher.
    for _ in 0..4 {
        mix_audio(&mut block, &mut state);
    }

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "pitch mix path allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "pitch mix path freed {deallocs} times across 8 blocks"
    );
}

#[test]
fn pitch_eof_flush_and_drain_is_free_free() {
    // F12: at EOF the pitch path drains the stretcher's tail in a single `flush` into
    // a pre-sized tail buffer, then plays it out block by block. Both the flush and
    // the per-block drain copy must do no heap work on the audio thread. The sample
    // is built and pitch-enabled off the armed region (where the stretcher and the
    // tail buffer are allocated), then EOF and the whole tail drain run armed.
    let mut pitched = ActiveSample::new(
        1,
        "p".to_string(),
        // Short source so EOF and the full tail drain fall inside the armed render.
        decoded(sine(220.0, SR, 6000, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "p".to_string(),
    );
    pitched.enable_pitch_correction();
    pitched.set_speed(1.0);
    let mut state = SceneBuilder::new(2).sample(pitched).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up off the armed region (prime the stretcher; sizes the scratch).
    for _ in 0..2 {
        mix_audio(&mut block, &mut state);
    }

    // Drive past EOF and through the entire tail drain while armed. 6000 input frames
    // at unity ≈ 12 blocks to EOF, then ~6 blocks of tail; 30 blocks covers the EOF
    // flush, every drain copy, and the post-drain no-op blocks. The finished sample
    // is NOT retained-out here: in production its drop happens off the RT thread (the
    // Sprint 5 graveyard), so dropping it here would mismeasure the mix path. What is
    // under test is that the flush and the per-block drain copy allocate nothing.
    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..30 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "pitch EOF flush/drain allocated {allocs} times on the audio thread"
    );
    assert_eq!(
        deallocs, 0,
        "pitch EOF flush/drain freed {deallocs} times on the audio thread"
    );
}

#[test]
fn callback_step_is_allocation_free_after_warmup() {
    // The full cpal callback step the engine runs each block — lock the bundled
    // state, drain pending commands, mix, then reap finished samples into the
    // graveyard — must do no heap work in steady state (D22a: the lock is
    // uncontended and allocation-free; the control plane never touches this state).
    //
    // Representative scene: a plain sample, a looping sample, and a pitch-corrected
    // sample, with a ducking applier carrying a target already applied to one voice.
    // TWO samples share the ducked voice "a" so the D1 once-per-buffer advance and
    // the per-sample non-advancing reads are exercised under the allocation counter.
    let plain = ActiveSample::new(
        1,
        "a".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let plain_same_voice = ActiveSample::new(
        4,
        "a".to_string(),
        decoded(sine(550.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let looping = ActiveSample::new_with_id(
        2,
        "b".to_string(),
        decoded(sine(330.0, SR, 4800, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "b".to_string(),
        None,
        true,
        0,
    );
    let mut pitched = ActiveSample::new(
        3,
        "p".to_string(),
        decoded(sine(220.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "p".to_string(),
    );
    pitched.enable_pitch_correction();
    pitched.set_speed(0.7);

    // Duck voice "a" to 0.5 over a long fade so the multiplier keeps advancing
    // (exercising the applier's per-block fade update) without ever finishing.
    let mut applier = DuckingApplier::new();
    applier.apply_target(&DuckTargetChange {
        voice: "a".to_string(),
        target_volume: 0.5,
        fade_frames: SR as usize * 10,
    });

    let mixer = SceneBuilder::new(2)
        .sample(plain)
        .sample(plain_same_voice)
        .sample(looping)
        .sample(pitched)
        .ducking(applier)
        .build();

    // Bundle the mixer with an (empty) command consumer, command-return producer,
    // and graveyard producer behind one uncontended mutex, exactly as the running
    // engine does.
    let (_cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        output_sample_rate: SR,
    });

    let mut block = vec![0.0f32; BLOCK * 2];

    // One full callback step against the bundled state, mirroring run_mix_callback.
    let step = |callback_state: &Mutex<AudioCallbackState>, block: &mut [f32]| {
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
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
    };

    // Warm up off the armed region: sizes the pitch scratch, primes the stretcher,
    // and grows any lazily-sized buffers so steady-state blocks are alloc-free.
    for _ in 0..4 {
        step(&callback_state, &mut block);
    }

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            step(&callback_state, &mut block);
        }
    });

    assert_eq!(
        allocs, 0,
        "callback step allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "callback step freed {deallocs} times across 8 blocks"
    );
}

#[test]
fn draining_mutation_commands_is_free_free() {
    // Draining control->audio mutation commands in the callback must not allocate
    // OR FREE on the RT thread. Each command carries heap (SampleSelector strings,
    // voice strings); if the callback drops the consumed command, that heap is freed
    // on the audio thread. The fix routes spent commands to a return ring for off-RT
    // drop, so a drain frees nothing here.
    let one = ActiveSample::new(
        1,
        "music".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let two = ActiveSample::new(
        2,
        "music".to_string(),
        decoded(sine(330.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "b".to_string(),
    );
    let mixer = SceneBuilder::new(2).sample(one).sample(two).build();

    let (mut cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        output_sample_rate: SR,
    });
    let mut block = vec![0.0f32; BLOCK * 2];

    let step = |callback_state: &Mutex<AudioCallbackState>, block: &mut [f32]| {
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
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
    };

    // Warm up off the armed region.
    for _ in 0..4 {
        step(&callback_state, &mut block);
    }

    // Pre-build heap-owning mutation commands (these allocate now, before arming)
    // and queue them; pushing moves them into the pre-allocated ring (no alloc).
    let sel = |voice: &str| SampleSelector {
        internal_id: None,
        id: None,
        file: None,
        voice: Some(voice.to_string()),
    };
    let commands = vec![
        AudioCommand::FadeOutMatching {
            selector: sel("music"),
            fade_ms: 10,
        },
        AudioCommand::SeekMatching {
            selector: sel("music"),
            position_ms: 100,
        },
        AudioCommand::SetVoiceVolume {
            voice: "music".to_string(),
            volume: 0.5,
        },
        AudioCommand::SetVolumeMatching {
            selector: sel("music"),
            volume: 0.8,
        },
    ];
    for c in commands {
        let _ = cmd_tx.push(c);
    }

    // One armed step drains all four commands.
    let (allocs, deallocs) = count_allocs(|| {
        step(&callback_state, &mut block);
    });

    assert_eq!(
        allocs, 0,
        "draining mutation commands must not allocate on the RT thread, got {allocs}"
    );
    assert_eq!(
        deallocs, 0,
        "draining mutation commands must not free heap on the RT thread, got {deallocs}"
    );
}

#[test]
fn adding_a_sample_into_the_reserved_pool_is_free_free() {
    // A Play (AddSample) into a voice pool with spare capacity must not allocate or
    // free on the RT thread: the un-boxed sample moves into the pre-reserved Vec.
    // Production reserves MAX_VOICES up front; here we reserve a small headroom.
    let mut mixer = SceneBuilder::new(2).build();
    mixer.active_samples.reserve(8);

    let (mut cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        output_sample_rate: SR,
    });
    let mut block = vec![0.0f32; BLOCK * 2];

    let step = |callback_state: &Mutex<AudioCallbackState>, block: &mut [f32]| {
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
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
    };

    for _ in 0..4 {
        step(&callback_state, &mut block);
    }

    // Build the sample (its channel_map / buffer Arc allocate here, before arming)
    // and queue it; pushing moves it into the pre-allocated ring.
    let sample = ActiveSample::new(
        9,
        "v".to_string(),
        decoded(sine(220.0, SR, 1000, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "p".to_string(),
    );
    let _ = cmd_tx.push(AudioCommand::AddSample(sample));

    let (allocs, deallocs) = count_allocs(|| {
        step(&callback_state, &mut block);
    });

    assert_eq!(
        allocs, 0,
        "adding a sample into the reserved pool must not allocate on the RT thread, got {allocs}"
    );
    assert_eq!(
        deallocs, 0,
        "adding a sample into the reserved pool must not free on the RT thread, got {deallocs}"
    );
}
