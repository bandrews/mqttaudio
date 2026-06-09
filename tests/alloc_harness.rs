// ABOUTME: Allocation-counting harness proving the mix path does no heap work per block.
// ABOUTME: A dedicated test binary with a counting global allocator; arms around mix_audio.

use mqttaudio::audio::ducking::{DuckTargetChange, DuckingApplier};
use mqttaudio::audio::input::{
    convert_input_block, create_ring_buffer, resample_block, ResampleState,
};
use mqttaudio::audio::mixer::{mix_audio, ActiveSample, LiveInput, StreamedSource};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use mqttaudio::mqtt::commands::SampleSelector;
use mqttaudio::rt_engine::{
    command_channel, command_return_channel, drain_commands, graveyard_channel, reap_finished,
    reap_finished_streamed, streamed_graveyard_channel, AudioCallbackState, AudioCommand,
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
fn telemetry_position_publish_is_allocation_free() {
    // Sprint W6 (DW3/DW12): with telemetry ON and a position publisher attached,
    // the callback stores each sample's live position into a pre-allocated atomic
    // once per block. The atomic is created off-RT (here, before the armed region);
    // the store itself must be 0 alloc / 0 free.
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let plain = ActiveSample::new(
        1,
        "a".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let mut state = SceneBuilder::new(2).sample(plain).build();
    state.telemetry_enabled.store(true, Ordering::Relaxed);
    let pos = Arc::new(AtomicUsize::new(0));
    state.active_samples[0].position_publisher = Some(pos.clone());

    let mut block = vec![0.0f32; BLOCK * 2];
    mix_audio(&mut block, &mut state); // warm up off the armed region

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "telemetry position publish allocated {allocs} times"
    );
    assert_eq!(
        deallocs, 0,
        "telemetry position publish freed {deallocs} times"
    );
    assert!(
        pos.load(Ordering::Relaxed) > 0,
        "the published position should have advanced"
    );
}

#[test]
fn live_input_underrun_mix_is_allocation_free_after_warmup() {
    // F3: the graceful-underrun fade in mix_live_input_into_output runs on the RT
    // thread (inside mix_audio). It holds the last frame and fades it to silence
    // using only fixed-size stack arrays, never the heap. Mix a live input whose
    // ring is starved (so every block both reads a few frames and then fades the
    // rest) and assert zero alloc/free in steady state.
    const CHANNELS: usize = 2;
    let (mut producer, consumer) = create_ring_buffer(4096);
    let input = LiveInput::new(
        "mic".to_string(),
        consumer,
        CHANNELS,
        1.0,
        vec![(0, 0), (1, 1)],
    );
    let mut state = SceneBuilder::new(2).live_input(input).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Feed fewer frames than a block consumes, so each mix underruns partway and
    // exercises the hold-and-fade path every block.
    let feed = sine(440.0, SR, BLOCK / 2, CHANNELS, 0.3);

    // Warm up off the armed region.
    for _ in 0..4 {
        producer.push_slice(&feed);
        mix_audio(&mut block, &mut state);
    }

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            producer.push_slice(&feed);
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "live-input underrun mix allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "live-input underrun mix freed {deallocs} times across 8 blocks"
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
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
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
            streamed_graveyard,
            output_sample_rate,
        } = acs;
        drain_commands(commands, mixer, command_returns, *output_sample_rate, 64);
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
        reap_finished_streamed(mixer, streamed_graveyard);
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
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
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
            streamed_graveyard,
            output_sample_rate,
        } = acs;
        drain_commands(commands, mixer, command_returns, *output_sample_rate, 64);
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
        reap_finished_streamed(mixer, streamed_graveyard);
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
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
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
            streamed_graveyard,
            output_sample_rate,
        } = acs;
        drain_commands(commands, mixer, command_returns, *output_sample_rate, 64);
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
        reap_finished_streamed(mixer, streamed_graveyard);
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

/// Drive `resample_block` for many capture blocks at the given rates while armed
/// and return the (allocs, deallocs) counted during the steady-state region.
fn count_resample_block_allocs(in_rate: u32, out_rate: u32) -> (usize, usize) {
    const CHANNELS: usize = 2;
    // A capture block large enough that several blocks cross the 1024-frame chunk
    // boundary, so process_into_buffer + the interleave-into-ring run while armed.
    const BLOCK_FRAMES: usize = 480;

    // Ring sized like production (output rate, 20 ms, 4x headroom).
    let ring_size = mqttaudio::audio::input::calculate_ring_buffer_size(out_rate, CHANNELS, 20);
    let (mut producer, mut consumer) = create_ring_buffer(ring_size);
    let mut state =
        ResampleState::new(in_rate, out_rate, CHANNELS, producer.capacity()).expect("resampler");

    // Interleaved input block (a quiet sine on both channels).
    let block = sine(440.0, in_rate, BLOCK_FRAMES, CHANNELS, 0.25);

    // Warm up off the armed region: the first chunks size internal buffers and
    // prime the resampler. Drain the consumer so the ring never wedges full.
    let mut sink = vec![0.0f32; ring_size];
    for _ in 0..16 {
        resample_block(&mut state, &block, &mut producer);
        consumer.pop_slice(&mut sink);
    }

    count_allocs(|| {
        for _ in 0..64 {
            resample_block(&mut state, &block, &mut producer);
            // Drain on the armed thread too; pop_slice is itself allocation-free.
            consumer.pop_slice(&mut sink);
        }
    })
}

#[test]
fn resampling_capture_block_is_allocation_free_after_warmup() {
    // F2: the cpal CAPTURE callback resamples input frames into the ring buffer.
    // Like mix_audio it runs on an RT thread (cpal's capture thread) and must do no
    // heap work: pre-sized de-interleave accumulators (no Vec::push growth), a
    // reusable rubato output buffer via output_buffer_allocate, and
    // process_into_buffer (never the allocating process()). The drift-control loop
    // it runs each chunk only reads the producer fill and sets two floats. This
    // must fail against the old Vec::push / drain(..).collect() / process() code.
    let (allocs, deallocs) = count_resample_block_allocs(44_100, 48_000);
    assert_eq!(
        allocs, 0,
        "resampling capture block allocated {allocs} times across 64 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "resampling capture block freed {deallocs} times across 64 blocks"
    );
}

#[test]
fn equal_rate_capture_block_is_allocation_free_after_warmup() {
    // D33 routes the equal-rate case through async SRC too (for drift control), so
    // that path is now always-on in production and must also be allocation-free on
    // the capture thread. Same harness, identical nominal in/out rate.
    let (allocs, deallocs) = count_resample_block_allocs(48_000, 48_000);
    assert_eq!(
        allocs, 0,
        "equal-rate capture block allocated {allocs} times across 64 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "equal-rate capture block freed {deallocs} times across 64 blocks"
    );
}

#[test]
fn typed_capture_callback_body_is_allocation_free_after_warmup() {
    // Mirrors the EXACT production capture-callback body for a non-f32 device:
    // convert_input_block::<i16> (native -> f32 into a reused scratch) feeding
    // resample_block. After warmup the scratch is sized and nothing on this path
    // touches the heap — the same RT rule as mix_audio (D22a).
    const IN_RATE: u32 = 44_100;
    const OUT_RATE: u32 = 48_000;
    const CHANNELS: usize = 2;
    const BLOCK_FRAMES: usize = 480;

    let ring_size = mqttaudio::audio::input::calculate_ring_buffer_size(OUT_RATE, CHANNELS, 20);
    let (mut producer, mut consumer) = create_ring_buffer(ring_size);
    let mut state =
        ResampleState::new(IN_RATE, OUT_RATE, CHANNELS, producer.capacity()).expect("resampler");
    let mut scratch: Vec<f32> = Vec::new();

    // An i16 capture block (interleaved), exactly what cpal hands an I16 device.
    let mut block = vec![0i16; BLOCK_FRAMES * CHANNELS];
    for (i, s) in block.iter_mut().enumerate() {
        *s = ((i as f32 * 0.01).sin() * 8000.0) as i16;
    }

    let mut sink = vec![0.0f32; ring_size];
    let mut run_one = |producer: &mut _, consumer: &mut ringbuf::HeapConsumer<f32>| {
        convert_input_block::<i16>(&block, &mut scratch, |f32s| {
            resample_block(&mut state, f32s, producer);
        });
        consumer.pop_slice(&mut sink);
    };

    for _ in 0..16 {
        run_one(&mut producer, &mut consumer);
    }

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..64 {
            run_one(&mut producer, &mut consumer);
        }
    });

    assert_eq!(
        allocs, 0,
        "typed capture callback body allocated {allocs} times across 64 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "typed capture callback body freed {deallocs} times across 64 blocks"
    );
}

#[test]
fn windowed_source_mix_is_allocation_free_after_warmup() {
    // A windowed StreamedSource consumes from a ring exactly like a live input. The
    // armed region must do zero alloc/free on the RT thread across BOTH steady-state
    // consume AND the 64-frame underrun hold-and-fade, so the ring is seeded with only
    // a little audio and then left to underrun.
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let mut mixer = SceneBuilder::new(2).build();
    let (mut producer, consumer) = create_ring_buffer(BLOCK * 2 * 4);
    let seed = vec![0.3f32; BLOCK * 2]; // ~one block of stereo audio
    producer.push_slice(&seed);
    // producer_done stays false so the source never finishes (and is never reaped)
    // during the test: it keeps mixing — first consuming the seed, then underrun-fading.
    let source = StreamedSource::new(
        1,
        "bed".to_string(),
        "stream.wav".to_string(),
        None,
        consumer,
        2,
        1.0,
        vec![(0, 0), (1, 1)],
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    mixer.streamed_sources.push(source);

    let (_cmd_tx, cmd_rx) = command_channel(1024);
    let (cmd_return_tx, _cmd_return_rx) = command_return_channel(1024);
    let (grave_tx, _grave_rx) = graveyard_channel(1024);
    let (streamed_grave_tx, _streamed_grave_rx) = streamed_graveyard_channel(256);
    let callback_state = Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
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
            streamed_graveyard,
            output_sample_rate,
        } = acs;
        drain_commands(commands, mixer, command_returns, *output_sample_rate, 64);
        mix_audio(block, mixer);
        reap_finished(mixer, graveyard);
        reap_finished_streamed(mixer, streamed_graveyard);
    };

    // Warm up off the armed region (one block consumes the seed; later blocks underrun).
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
        "streamed-source mix allocated {allocs} times across 8 blocks (incl. underrun)"
    );
    assert_eq!(
        deallocs, 0,
        "streamed-source mix freed {deallocs} times across 8 blocks (incl. underrun)"
    );
}
