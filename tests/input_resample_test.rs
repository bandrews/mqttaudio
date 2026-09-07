// ABOUTME: Drift-control and format-dispatch tests for the live-input capture path.
// ABOUTME: Simulates two independently-clocked devices and proves the ring stays bounded.

use mqttaudio::audio::input::{
    calculate_ring_buffer_size, convert_input_block, create_ring_buffer, input_stream_builder,
    minimum_latency_ms, resample_block, ResampleState,
};
use mqttaudio::audio::mixer::{mix_audio, LiveInput, MixerState};
use mqttaudio::audio::test_support::{band_energy, rms};

/// Outcome of a drift simulation run.
struct DriftResult {
    min_fill: usize,
    max_fill: usize,
    capacity: usize,
    /// Interleaved output collected after warmup, for spectral checks.
    drained: Vec<f32>,
    /// The ratio the controller had converged to at the end of the run.
    final_ratio: f64,
    nominal_ratio: f64,
}

/// One run of the drift simulation. A producer feeds input frames at `in_rate`
/// (the input device's *actual* clock) through `resample_block`; a consumer
/// drains the resulting output-rate samples at `out_rate` (the output device's
/// *actual* clock). Both nominal rates are `nominal_rate`, so when the two clocks
/// differ the resampler must steer its ratio to keep the ring balanced (D33).
fn run_drift_sim(
    nominal_rate: u32,
    in_rate: f64,
    out_rate: f64,
    channels: usize,
    seconds: f64,
    tone_hz: f32,
) -> DriftResult {
    let ring_size = calculate_ring_buffer_size(nominal_rate, channels, 20);
    let (mut producer, mut consumer) = create_ring_buffer(ring_size);
    let capacity = producer.capacity();
    let mut state =
        ResampleState::new(nominal_rate, nominal_rate, channels, capacity).expect("resampler");

    // Pre-fill toward half so the loop starts near its operating point (a real
    // stream warms up the same way before the mixer starts draining).
    let half = capacity / 2;

    // Simulate in 5 ms slices; accumulate fractional frames so the average rates
    // are exact over the run.
    let dt = 0.005f64;
    let steps = (seconds / dt) as usize;
    let mut in_acc = 0.0f64;
    let mut out_acc = 0.0f64;
    // A continuous-phase tone so the de-interleaved input is a real signal.
    let mut phase = 0.0f64;
    let phase_inc = std::f64::consts::TAU * tone_hz as f64 / in_rate;

    let mut min_fill = usize::MAX;
    let mut max_fill = 0usize;
    let mut drained: Vec<f32> = Vec::new();
    let mut sink = vec![0.0f32; ring_size];

    // Warmup fraction: ignore the first 20% for fill bounds and spectral capture
    // so the controller has reached steady state.
    let warmup_steps = steps / 5;

    for step in 0..steps {
        in_acc += in_rate * dt;
        let frames_in = in_acc.floor() as usize;
        in_acc -= frames_in as f64;

        // Build this slice's interleaved input block (continuous phase).
        let mut block = Vec::with_capacity(frames_in * channels);
        for _ in 0..frames_in {
            let s = phase.sin() as f32 * 0.25;
            phase += phase_inc;
            if phase >= std::f64::consts::TAU {
                phase -= std::f64::consts::TAU;
            }
            for _ in 0..channels {
                block.push(s);
            }
        }
        resample_block(&mut state, &block, &mut producer);

        // Consumer side: on the very first steps, hold off draining until the ring
        // has filled to ~half (warmup), mirroring the mixer starting after the
        // input has buffered.
        if producer.len() >= half || step > warmup_steps / 2 {
            out_acc += out_rate * dt;
            let frames_out = out_acc.floor() as usize;
            out_acc -= frames_out as f64;
            let want = frames_out * channels;
            let sink_len = sink.len();
            let mut got = 0;
            while got < want {
                let take = (want - got).min(sink_len);
                let n = consumer.pop_slice(&mut sink[..take]);
                if n == 0 {
                    break; // underrun: consumer outran the producer this slice
                }
                if step >= warmup_steps {
                    drained.extend_from_slice(&sink[..n]);
                }
                got += n;
            }
        }

        if step >= warmup_steps {
            let fill = producer.len();
            min_fill = min_fill.min(fill);
            max_fill = max_fill.max(fill);
        }
    }

    DriftResult {
        min_fill,
        max_fill,
        capacity,
        drained,
        final_ratio: state.current_ratio(),
        nominal_ratio: state.nominal_ratio(),
    }
}

/// A gross ±1% clock mismatch must stay bounded: the steered ratio tracks the
/// drift so the ring never empties (underrun) or overflows. This is the
/// mechanism proof — with a *fixed* ratio the ring drifts monotonically to a
/// boundary within seconds (the loop-disabled run hits min_fill=0).
///
/// (Tiny-ppm mismatches drift to a boundary only over many minutes — far longer
/// than is sensible to simulate sample-accurately through a 256-tap sinc; that
/// long-horizon boundedness is what Lane C's ≥30-min soak confirms. The control
/// *law* that makes it bounded is exercised here and by the convergence test.)
#[test]
fn drift_one_percent_keeps_ring_bounded() {
    let nominal = 48_000u32;
    for sign in [1.0f64, -1.0] {
        let out_rate = nominal as f64 * (1.0 + sign * 0.01);
        let r = run_drift_sim(nominal, nominal as f64, out_rate, 2, 25.0, 1000.0);
        assert!(
            r.min_fill > 0,
            "ring underran at ±1% (sign {sign}): min_fill={}, cap={}",
            r.min_fill,
            r.capacity
        );
        assert!(
            r.max_fill < r.capacity,
            "ring overflowed at ±1% (sign {sign}): max_fill={}, cap={}",
            r.max_fill,
            r.capacity
        );
    }
}

/// The control loop locks onto the true clock ratio: under a steady 0.5%
/// mismatch the commanded ratio converges toward `r_out / r_in` (the equilibrium
/// at which production matches consumption), and the ring stays bounded. This is
/// the direct test of F1's steering law and runs fast.
#[test]
fn steering_converges_to_true_clock_ratio() {
    let nominal = 48_000u32;
    for sign in [1.0f64, -1.0] {
        let mismatch = sign * 0.005;
        let out_rate = nominal as f64 * (1.0 + mismatch);
        let r = run_drift_sim(nominal, nominal as f64, out_rate, 2, 40.0, 1000.0);
        let demanded = out_rate / nominal as f64; // == 1 + mismatch

        // The converged ratio should sit between nominal and the demanded ratio,
        // and most of the way there — a proportional loop reaches its equilibrium
        // deviation, which by construction equals the demanded deviation.
        let progress = (r.final_ratio - r.nominal_ratio) / (demanded - r.nominal_ratio);
        assert!(
            (0.5..=1.5).contains(&progress),
            "ratio did not converge to the true clock ratio (sign {sign}): \
             final={}, nominal={}, demanded={}, progress={progress}",
            r.final_ratio,
            r.nominal_ratio,
            demanded
        );
        assert!(
            r.min_fill > 0 && r.max_fill < r.capacity,
            "ring left bounds while converging (sign {sign}): min={}, max={}, cap={}",
            r.min_fill,
            r.max_fill,
            r.capacity
        );
    }
}

/// D33: even with *identical* nominal and actual rates, the equal-rate case is
/// routed through async SRC and must stay bounded and pass the tone cleanly (the
/// steering should idle near zero deviation, no pitch wobble).
#[test]
fn equal_rate_through_async_src_is_bounded_and_clean() {
    let nominal = 48_000u32;
    let tone = 1000.0f32;
    let r = run_drift_sim(nominal, nominal as f64, nominal as f64, 2, 15.0, tone);

    assert!(
        r.min_fill > 0,
        "ring underran at equal rate: min_fill={}",
        r.min_fill
    );
    assert!(
        r.max_fill < r.capacity,
        "ring overflowed at equal rate: max_fill={}, cap={}",
        r.max_fill,
        r.capacity
    );

    // The output is a live signal whose energy concentrates at the input tone,
    // not at an off-tone bin: a steady sine, not a wobbling/aliased one. (Energy
    // is compared as a ratio between bins so the check does not depend on the
    // tone landing on an exact single-bin Goertzel frequency.)
    assert!(
        r.drained.len() > nominal as usize,
        "captured too little output"
    );
    assert!(
        rms(&r.drained, 0, 2) > 0.05,
        "output nearly silent after async SRC"
    );
    let at_tone = band_energy(&r.drained, nominal, 0, 2, tone);
    let off_tone = band_energy(&r.drained, nominal, 0, 2, tone * 1.5);
    assert!(
        off_tone < at_tone * 0.1,
        "off-tone energy too high (wobble/aliasing): at={at_tone}, off={off_tone}"
    );
}

/// No-wobble check: under a 0.5% mismatch the output amplitude is steady across
/// the run (first half vs second half RMS), i.e. the gentle steering introduces
/// no audible amplitude modulation.
#[test]
fn steering_introduces_no_amplitude_wobble() {
    let nominal = 48_000u32;
    let out_rate = nominal as f64 * (1.0 + 0.005);
    let r = run_drift_sim(nominal, nominal as f64, out_rate, 2, 25.0, 1000.0);
    assert!(
        r.drained.len() > 4 * nominal as usize,
        "captured too little"
    );

    let mid = (r.drained.len() / 4) * 2; // frame-aligned (stereo)
    let first = rms(&r.drained[..mid], 0, 2);
    let second = rms(&r.drained[mid..], 0, 2);
    assert!(
        first > 0.01 && second > 0.01,
        "output silent: {first}/{second}"
    );
    let ratio = first / second;
    assert!(
        (0.9..=1.1).contains(&ratio),
        "amplitude wobble across run: first_half_rms={first}, second_half_rms={second}"
    );
}

/// F2 de-interleave correctness: distinct per-channel signals must stay on their
/// own channels through the rewritten (index-based, no Vec::push) de-interleave
/// and the resampler. A left/right swap or channel bleed would put L's tone on R.
#[test]
fn deinterleave_preserves_distinct_channels() {
    let nominal = 48_000u32;
    let channels = 2usize;
    let left_hz = 1000.0f32;
    let right_hz = 3000.0f32;

    let ring_size = calculate_ring_buffer_size(nominal, channels, 20);
    let (mut producer, mut consumer) = create_ring_buffer(ring_size);
    let mut state =
        ResampleState::new(nominal, nominal, channels, producer.capacity()).expect("resampler");

    // Feed many chunks of L=1kHz, R=3kHz, draining as we go so the ring never wedges.
    let mut drained: Vec<f32> = Vec::new();
    let mut sink = vec![0.0f32; ring_size];
    let block_frames = 1024usize;
    let mut phase_l = 0.0f64;
    let mut phase_r = 0.0f64;
    let inc_l = std::f64::consts::TAU * left_hz as f64 / nominal as f64;
    let inc_r = std::f64::consts::TAU * right_hz as f64 / nominal as f64;
    for _ in 0..40 {
        let mut block = Vec::with_capacity(block_frames * channels);
        for _ in 0..block_frames {
            block.push((phase_l.sin() * 0.25) as f32);
            block.push((phase_r.sin() * 0.25) as f32);
            phase_l += inc_l;
            phase_r += inc_r;
        }
        resample_block(&mut state, &block, &mut producer);
        let n = consumer.pop_slice(&mut sink);
        drained.extend_from_slice(&sink[..n]);
    }

    assert!(drained.len() > 8 * channels, "captured too little");
    // Each channel must be a live signal and carry far more energy at its OWN
    // tone than at the other channel's tone. (Compared as ratios so the check is
    // robust to single-bin Goertzel leakage when a tone is off an exact bin.)
    assert!(
        rms(&drained, 0, channels) > 0.05 && rms(&drained, 1, channels) > 0.05,
        "a channel went silent through de-interleave"
    );
    let l_at_l = band_energy(&drained, nominal, 0, channels, left_hz);
    let l_at_r = band_energy(&drained, nominal, 0, channels, right_hz);
    let r_at_r = band_energy(&drained, nominal, 1, channels, right_hz);
    let r_at_l = band_energy(&drained, nominal, 1, channels, left_hz);
    assert!(
        l_at_l > 10.0 * l_at_r,
        "left channel bled the right tone (swap/bleed): l_at_l={l_at_l}, l_at_r={l_at_r}"
    );
    assert!(
        r_at_r > 10.0 * r_at_l,
        "right channel bled the left tone (swap/bleed): r_at_r={r_at_r}, r_at_l={r_at_l}"
    );
}

/// F4 (D37): a forced-I16 input block converts to f32 and lands in the ring
/// buffer. `convert_input_block` is the pure conversion seam the typed cpal
/// callback uses, so this runs without hardware.
#[test]
fn i16_input_converts_to_f32_in_ring() {
    let (mut producer, mut consumer) = create_ring_buffer(64);
    let mut scratch: Vec<f32> = Vec::new();

    // i16 full-scale +/- maps to ~ +1.0 / -1.0 in f32 (cpal's sample conversion).
    let input: [i16; 4] = [i16::MAX, 0, i16::MIN, i16::MAX / 2];
    convert_input_block::<i16>(&input, &mut scratch, |f32s| {
        producer.push_slice(f32s);
    });

    let mut out = [0.0f32; 4];
    let n = consumer.pop_slice(&mut out);
    assert_eq!(n, 4, "expected 4 converted samples");
    assert!(
        (out[0] - 1.0).abs() < 1e-3,
        "i16::MAX -> ~1.0, got {}",
        out[0]
    );
    assert!(out[1].abs() < 1e-3, "0 -> ~0.0, got {}", out[1]);
    assert!(
        (out[2] + 1.0).abs() < 1e-3,
        "i16::MIN -> ~-1.0, got {}",
        out[2]
    );
    assert!(
        (out[3] - 0.5).abs() < 1e-2,
        "half-scale -> ~0.5, got {}",
        out[3]
    );
}

/// F4: a forced-F32 block passes through `convert_input_block` unchanged.
#[test]
fn f32_input_passes_through_convert() {
    let (mut producer, mut consumer) = create_ring_buffer(64);
    let mut scratch: Vec<f32> = Vec::new();
    let input: [f32; 3] = [0.5, -0.25, 1.0];
    convert_input_block::<f32>(&input, &mut scratch, |f32s| {
        producer.push_slice(f32s);
    });
    let mut out = [0.0f32; 3];
    assert_eq!(consumer.pop_slice(&mut out), 3);
    assert_eq!(out, input);
}

/// F4: the dispatcher accepts every cpal sample format this app supports and
/// rejects the rest with a clear error. (Stream construction itself needs a
/// device — Lane B — so the dispatcher is exercised here at the format level.)
#[test]
fn format_dispatch_accepts_supported_and_rejects_unknown() {
    use cpal::SampleFormat;
    for fmt in [
        SampleFormat::F32,
        SampleFormat::I16,
        SampleFormat::U16,
        SampleFormat::I32,
    ] {
        assert!(
            input_stream_builder(fmt).is_some(),
            "format {fmt:?} should be supported"
        );
    }
    assert!(
        input_stream_builder(SampleFormat::I8).is_none(),
        "I8 is not a supported input format and must be rejected"
    );
}

#[test]
fn ring_overflow_bumps_the_telemetry_counter_instead_of_logging() {
    // D57: the capture path reports drops through relaxed atomic counters (the
    // control thread logs deltas off-RT); a starved ring must grow
    // overflow_dropped_samples, never panic or log on the capture thread.
    use std::sync::atomic::Ordering;

    let channels = 2usize;
    // A tiny ring: one resampler chunk overflows it many times over.
    let (mut producer, _consumer) = create_ring_buffer(64);
    let mut state = ResampleState::new(48000, 48000, channels, 64).unwrap();
    let telemetry = state.telemetry();

    // Feed several chunks so the resampler emits well past the ring's capacity.
    let block = vec![0.25f32; 1024 * channels];
    for _ in 0..4 {
        resample_block(&mut state, &block, &mut producer);
    }

    assert!(
        telemetry.overflow_dropped_samples.load(Ordering::Relaxed) > 0,
        "a starved ring must be counted as overflow drops"
    );
    assert_eq!(
        telemetry.resample_errors.load(Ordering::Relaxed),
        0,
        "well-formed chunks must not count as resample errors"
    );
}

/// Outcome of running the capture path and the mixer's live-input path together.
struct MixerSimResult {
    /// Trim events after warmup, and the frames they discarded.
    trims: usize,
    trimmed_frames: u64,
    /// Highest backlog the mixer saw at the start of a callback after warmup.
    peak_backlog_frames: usize,
    /// Output frames the mixer had no captured audio for, after warmup.
    underrun_frames: u64,
    /// Frames the capture side could not hand over because the ring was full.
    dropped_frames: u64,
    final_ratio: f64,
    max_backlog_frames: usize,
}

/// Feed the drift-steered resampler and drain it through `mix_audio`, the way
/// the daemon does: capture delivers `block` frames per callback (the capture
/// stream is opened with the output buffer size) and the output callback
/// consumes `block` frames. `mismatch` is the capture clock's excess over the
/// output clock (+0.01 = capture runs 1% fast). Runs for `seconds`, ignoring the
/// first five for the loop to settle.
fn run_mixer_sim(block: usize, latency_ms: u32, mismatch: f64, seconds: usize) -> MixerSimResult {
    let rate = 48_000u32;
    let channels = 1usize;
    let ring_size = calculate_ring_buffer_size(rate, channels, latency_ms);
    let (mut producer, consumer) = create_ring_buffer(ring_size);
    let capacity = producer.capacity();
    let mut state = ResampleState::new(rate, rate, channels, capacity).expect("resampler");
    let max_backlog_frames = state.backlog_ceiling_frames();

    let mut mixer = MixerState::new(channels);
    mixer.live_inputs.push(LiveInput::new(
        "mic".to_string(),
        consumer,
        channels,
        1.0,
        vec![(0, 0)],
        max_backlog_frames,
    ));

    let mut output = vec![0.0f32; block * channels];
    let callbacks_per_second = rate as usize / block;
    let warmup_callbacks = callbacks_per_second * 5;
    let mut capture_acc = 0.0f64;
    let mut trims = 0usize;
    let mut trimmed_frames = 0u64;
    let mut peak_backlog_frames = 0usize;
    let mut underruns_at_warmup = 0u64;

    for callback in 0..callbacks_per_second * seconds {
        capture_acc += block as f64 * (1.0 + mismatch);
        let frames_in = capture_acc.floor() as usize;
        capture_acc -= frames_in as f64;
        resample_block(
            &mut state,
            &vec![0.1f32; frames_in * channels],
            &mut producer,
        );

        let before = mixer.live_inputs[0].trimmed_frames;
        let backlog = mixer.live_inputs[0].backlog_frames();
        mix_audio(&mut output, &mut mixer);
        let input = &mixer.live_inputs[0];
        if callback == warmup_callbacks {
            underruns_at_warmup = input.underrun_frames;
        }
        if callback >= warmup_callbacks {
            let trimmed = input.trimmed_frames - before;
            if trimmed > 0 {
                trims += 1;
                trimmed_frames += trimmed;
            }
            peak_backlog_frames = peak_backlog_frames.max(backlog);
        }
    }

    let input = &mixer.live_inputs[0];
    MixerSimResult {
        trims,
        trimmed_frames,
        peak_backlog_frames,
        underrun_frames: input.underrun_frames - underruns_at_warmup,
        dropped_frames: state
            .telemetry()
            .dropped_frames
            .load(std::sync::atomic::Ordering::Relaxed),
        final_ratio: state.current_ratio(),
        max_backlog_frames,
    }
}

/// The drift-steered resampler and the mixer's backlog ceiling must agree on
/// where the ring sits. With both device clocks equal the loop parks the ring at
/// its target and each resampler burst lands on top of that, so the mixer must
/// never trim: a trim here is a self-inflicted dropout, not drift compensation.
/// Checked at the daemon's default `audio.buffer_size` and at the smaller and
/// larger blocks an interface may run.
#[test]
fn steady_equal_rate_capture_is_never_trimmed_by_the_mixer() {
    for block in [256usize, 512, 1024] {
        let r = run_mixer_sim(block, 20, 0.0, 30);
        assert_eq!(
            r.trimmed_frames, 0,
            "block {block}: mixer trimmed {} frames in {} callbacks after warmup \
             (ceiling {} frames, peak backlog {} frames, steered ratio {:.5})",
            r.trimmed_frames, r.trims, r.max_backlog_frames, r.peak_backlog_frames, r.final_ratio,
        );
        assert_eq!(
            r.underrun_frames, 0,
            "block {block}: mixer starved after warmup"
        );
        assert_eq!(
            r.dropped_frames, 0,
            "block {block}: capture dropped at the ring"
        );
        assert!(
            (r.final_ratio - 1.0).abs() < 0.001,
            "block {block}: equal clocks should idle near the nominal ratio, got {}",
            r.final_ratio
        );
    }
}

/// While the clock mismatch is inside the steering authority the ratio absorbs
/// it and the mixer still has no reason to trim: the ceiling leaves room for the
/// proportional loop's steady-state offset plus one burst.
#[test]
fn drift_within_steering_authority_is_absorbed_without_trims() {
    for mismatch in [0.015f64, -0.015] {
        let r = run_mixer_sim(512, 20, mismatch, 40);
        assert_eq!(
            r.trimmed_frames, 0,
            "mismatch {mismatch}: mixer trimmed {} frames in {} callbacks (ceiling {}, peak \
             backlog {}, ratio {:.5})",
            r.trimmed_frames, r.trims, r.max_backlog_frames, r.peak_backlog_frames, r.final_ratio,
        );
        assert_eq!(r.underrun_frames, 0, "mismatch {mismatch}: mixer starved");
        assert_eq!(
            r.dropped_frames, 0,
            "mismatch {mismatch}: capture dropped at the ring"
        );
    }
}

/// Past the steering authority the backlog would grow without bound; the mixer's
/// ceiling is the safety net that keeps latency bounded by discarding the oldest
/// audio. A callback can still find one burst above the ceiling, landed since
/// the previous trim, so that is the latency bound.
#[test]
fn drift_beyond_steering_authority_is_trimmed_to_the_ceiling() {
    let r = run_mixer_sim(512, 25, 0.04, 30);
    assert!(r.trims > 0, "a 4% fast capture clock must be trimmed");
    assert!(
        r.peak_backlog_frames <= r.max_backlog_frames + 1024,
        "backlog {} exceeded the ceiling {} plus one burst",
        r.peak_backlog_frames,
        r.max_backlog_frames
    );
}

/// A ring must hold the steering target plus one resampler burst, otherwise the
/// capture side drops at the ring on every chunk. The minimum latency follows
/// from the burst size at the nominal ratio and the output rate.
#[test]
fn minimum_latency_holds_the_target_plus_one_burst() {
    // 1024-frame burst at 48 kHz: 2 bursts / (4 * 48 frames per ms) = 10.67 ms.
    assert_eq!(minimum_latency_ms(48_000, 48_000), 11);
    // 44.1 kHz counts 44 frames per ms in the ring sizing.
    assert_eq!(minimum_latency_ms(44_100, 44_100), 12);
    // Capture at 44.1 kHz into a 48 kHz output: the burst grows with the ratio.
    assert_eq!(minimum_latency_ms(44_100, 48_000), 12);
    // Capture at 96 kHz into 48 kHz: half the burst, half the latency.
    assert_eq!(minimum_latency_ms(96_000, 48_000), 6);

    for (input_rate, output_rate) in [(48_000, 48_000), (44_100, 48_000), (96_000, 48_000)] {
        let latency = minimum_latency_ms(input_rate, output_rate);
        let capacity = calculate_ring_buffer_size(output_rate, 1, latency);
        let state = ResampleState::new(input_rate, output_rate, 1, capacity).expect("resampler");
        let burst = (1024.0 * output_rate as f64 / input_rate as f64).ceil() as usize;
        assert!(
            state.backlog_ceiling_frames() <= capacity,
            "{input_rate}->{output_rate}: ceiling {} does not fit the ring {}",
            state.backlog_ceiling_frames(),
            capacity
        );
        assert!(
            capacity >= 2 * burst,
            "{input_rate}->{output_rate}: ring {} at {latency} ms holds fewer than two bursts",
            capacity
        );
    }
}
