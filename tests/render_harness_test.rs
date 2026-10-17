// ABOUTME: Seed tests for the offline render harness (src/audio/test_support.rs).
// ABOUTME: Exercise additive mixing and faded decay, and the analysis helpers.

use mqttaudio::audio::ducking::{DuckTargetChange, DuckingApplier};
use mqttaudio::audio::mixer::{db_to_linear, ActiveSample, FadeState};
use mqttaudio::audio::test_support::{
    band_energy, decoded, max_inter_sample_delta, nan_poisoned, peak, ramp, render, rms, sine,
    transient_loop, SceneBuilder,
};
use std::sync::atomic::Ordering;

const SR: u32 = 48000;

/// A constant-level mono-into-stereo sample on `voice`, so the rendered output
/// level is directly `value * duck_multiplier` with no discrete-waveform fuzz —
/// the cleanest probe for the ducking gain applied to a voice.
fn constant_sample(id: u64, voice: &str, value: f32, frames: usize) -> ActiveSample {
    ActiveSample::new(
        id,
        voice.to_string(),
        decoded(vec![value; frames * 2], 2, SR),
        1.0,
        1.0,
        "c.wav".to_string(),
    )
}

/// A `DuckingApplier` with one voice already ducking toward `target` over
/// `fade_frames`, as the control thread would have armed it via a SetDuckTarget.
fn applier_ducking(voice: &str, target: f32, fade_frames: usize) -> DuckingApplier {
    let mut applier = DuckingApplier::with_ducked_voices([voice]);
    applier.apply_target(&DuckTargetChange {
        voice: voice.to_string(),
        target_volume: target,
        fade_frames,
    });
    applier
}

#[test]
fn two_in_phase_tones_sum_additively() {
    let frames = 4096;
    let amp = 0.25;
    let freq = 480.0;
    let buffer = decoded(sine(freq, SR, frames, 2, amp), 2, SR);

    let s1 = ActiveSample::new(
        1,
        "a".to_string(),
        buffer.clone(),
        1.0,
        1.0,
        "t.wav".to_string(),
    );
    let s2 = ActiveSample::new(
        2,
        "b".to_string(),
        buffer.clone(),
        1.0,
        1.0,
        "t.wav".to_string(),
    );
    let mut state = SceneBuilder::new(2).sample(s1).sample(s2).build();

    let out = render(&mut state, frames, 1);

    // Two identical in-phase tones sum to 2x amplitude (0.5), well under the clamp.
    let measured = rms(&out, 0, 2);
    let expected = (2.0 * amp) / 2.0_f32.sqrt(); // RMS of a 0.5-amplitude sine
    assert!(
        (measured - expected).abs() < 0.02,
        "additive RMS: measured {measured}, expected {expected}"
    );

    let pk = peak(&out);
    assert!(pk > 0.45 && pk <= 0.5 + 1e-3, "peak {pk} should be ~0.5");

    // Energy concentrated at the tone and effectively absent two octaves up.
    assert!(
        band_energy(&out, SR, 0, 2, freq) > 0.4,
        "tone band should be strong"
    );
    assert!(
        band_energy(&out, SR, 0, 2, freq * 4.0) < 0.05,
        "off-tone band should be quiet"
    );
}

#[test]
fn fade_out_decays_monotonically_without_clicks() {
    let block = 512;
    let blocks = 12;
    let total = block * blocks;
    let amp = 0.5;
    let freq = 440.0;

    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "t.wav".to_string());
    // Fade-out spanning the entire render.
    let fade_ms = (total as f32 / SR as f32 * 1000.0) as u32;
    sample.set_fade(FadeState::fade_out(fade_ms, SR));
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let out = render(&mut state, block, blocks);

    // Per-block RMS must be non-increasing (a smooth decay, no resurgence).
    let channels = 2;
    let mut prev = f32::INFINITY;
    for b in 0..blocks {
        let start = b * block * channels;
        let end = start + block * channels;
        let r = rms(&out[start..end], 0, channels);
        assert!(r <= prev + 1e-3, "block {b} RMS rose: {r} > {prev}");
        prev = r;
    }
    assert!(
        prev < 0.1,
        "tail should have decayed, last-block RMS {prev}"
    );

    // No click: the largest sample-to-sample step stays within the tone's own
    // slope (a fade is far slower than the carrier), so no discontinuity.
    let natural_slope = amp * std::f32::consts::TAU * freq / SR as f32;
    let delta = max_inter_sample_delta(&out, 0, channels);
    assert!(
        delta < natural_slope * 2.0 + 0.01,
        "inter-sample delta {delta} exceeds smooth-fade bound"
    );
}

#[test]
fn pitch_corrected_render_is_stable_across_blocks() {
    // A pitch-corrected sample rendered over many callback blocks exercises the
    // reused pre-allocated pitch scratch buffer (allocated once, then reused). The
    // output must stay clean: finite, non-silent, bounded, and click-free — proving
    // the scratch refactor did not change the audible result.
    let frames = 48000;
    let buffer = decoded(sine(220.0, SR, frames, 2, 0.5), 2, SR);
    let mut sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "f".to_string());
    sample.enable_pitch_correction();
    sample.set_speed(0.5); // half speed, pitch preserved

    // Enough blocks to clear the stretcher's startup latency and reach steady
    // state, so the reused scratch buffer is exercised across many callbacks.
    let mut state = SceneBuilder::new(2).sample(sample).build();
    let out = render(&mut state, 512, 64);

    assert!(
        out.iter().all(|s| s.is_finite()),
        "pitch-corrected output must be finite"
    );
    // `peak` proves real signal got through (RMS is diluted by the stretcher's
    // startup latency over a short render) and that it stays bounded.
    let pk = peak(&out);
    assert!(
        pk > 0.1 && pk <= 1.5,
        "pitch-corrected output should be present and bounded, peak = {pk}"
    );
    assert!(
        max_inter_sample_delta(&out, 0, 2) < 0.5,
        "pitch-corrected output clicked, max delta = {}",
        max_inter_sample_delta(&out, 0, 2)
    );
}

#[test]
fn nan_input_is_sanitized_to_silence() {
    // A buffer carrying NaN/Inf at known frames must never reach the output as a
    // non-finite value: f32::NAN.clamp(-1.0, 1.0) returns NaN, so the old clamp did
    // not sanitize. The final-loop finite-guard renders poisoned frames as 0.0 (F3).
    let frames = 512;
    let bad = [10usize, 200, 511];
    let buffer = decoded(nan_poisoned(frames, 2, 0.5, &bad), 2, SR);
    let sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "t.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let out = render(&mut state, frames, 1);

    assert!(
        out.iter().all(|s| s.is_finite()),
        "output must be finite after the NaN guard"
    );
    assert!(peak(&out).is_finite(), "peak must be finite");

    // Every channel of a poisoned source frame is sanitized to exactly 0.0.
    let channels = 2;
    for &frame in &bad {
        for ch in 0..channels {
            let v = out[frame * channels + ch];
            assert_eq!(v, 0.0, "poisoned frame {frame} ch {ch} must render as 0.0");
        }
    }

    // A clean neighbouring frame still carries the real signal (the guard only
    // zeroes the non-finite samples, not the whole buffer).
    assert_eq!(out[11 * channels], 0.5, "clean frame must be untouched");
}

#[test]
fn per_channel_calibration_scales_each_channel() {
    // channel_gains: ch0 = 0.5, ch1 = 1.0. A full-scale-ish signal on both channels
    // must come out attenuated on ch0 and untouched on ch1 (F1), applied as the
    // final per-channel gain stage. 0.6 stays below the limiter knee, so the gain
    // is the only thing acting.
    let frames = 2048;
    let amp = 0.6;
    let buffer = decoded(sine(300.0, SR, frames, 2, amp), 2, SR);
    let s = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "t.wav".to_string());
    let mut state = SceneBuilder::new(2)
        .channel_gains(vec![0.5, 1.0])
        .sample(s)
        .build();

    let out = render(&mut state, frames, 1);

    let rms0 = rms(&out, 0, 2);
    let rms1 = rms(&out, 1, 2);
    // ch0 is halved relative to ch1.
    assert!(
        (rms0 / rms1 - 0.5).abs() < 0.02,
        "ch0/ch1 rms ratio {} should be ~0.5",
        rms0 / rms1
    );

    let expected_rms1 = amp / 2.0_f32.sqrt();
    assert!(
        (rms1 - expected_rms1).abs() < 0.02,
        "ch1 (gain 1.0) rms {rms1} should match the source {expected_rms1}"
    );

    // Peak per channel reflects the gain too.
    let peak0 = out.iter().step_by(2).fold(0.0f32, |m, &s| m.max(s.abs()));
    let peak1 = out
        .iter()
        .skip(1)
        .step_by(2)
        .fold(0.0f32, |m, &s| m.max(s.abs()));
    assert!((peak0 - amp * 0.5).abs() < 0.02, "ch0 peak {peak0}");
    assert!((peak1 - amp).abs() < 0.02, "ch1 peak {peak1}");
}

#[test]
fn limiter_holds_summed_bus_below_ceiling() {
    // Two constant 0.8 sources sum to 1.6. A brickwall clamp flat-tops at 1.0; the
    // soft-knee limiter holds the peak at or below the ceiling (-1.0 dBFS) and
    // counts the over-ceiling event (F2/D26).
    let frames = 512;
    let ceiling = db_to_linear(-1.0);
    let buf = || decoded(vec![0.8f32; frames * 2], 2, SR);

    let s1 = ActiveSample::new(1, "a".to_string(), buf(), 1.0, 1.0, "a.wav".to_string());
    let s2 = ActiveSample::new(2, "b".to_string(), buf(), 1.0, 1.0, "b.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(s1).sample(s2).build();
    let clip = state.clip_count.clone();

    let out = render(&mut state, frames, 1);
    let pk = peak(&out);

    assert!(
        pk <= ceiling + 1e-4,
        "peak {pk} must be at or below the ceiling {ceiling}"
    );
    // It still drives close to the ceiling (it limits to it, not far below, and is
    // certainly not the brickwall's 1.0).
    assert!(pk > ceiling - 0.05, "peak {pk} should approach the ceiling");
    assert!(pk < 1.0, "soft limiter must not flat-top at full scale");
    assert!(
        clip.load(Ordering::Relaxed) > 0,
        "the clip/over counter must increment when the limiter acts"
    );
}

#[test]
fn reverse_interpolation_uses_the_forward_neighbor() {
    // F9: reverse playback must interpolate between frame_n and frame_n+1 with
    // frac = src_pos.fract(), exactly like forward — the neighbor is
    // direction-independent. On a ramp (frame value = n * step) that two-point
    // interpolation resolves to src_pos * step at any position, so the output is
    // analytically predictable.
    //
    // src_pos starts at the integer start position and steps by `speed` each
    // frame, matching mix_sample_into_output (use, then advance). The old code
    // blended frame_n with frame_n-1, giving src_pos = src_frame - frac instead
    // of src_frame + frac — wrong by 2*frac*step wherever frac != 0.
    let frames = 64;
    let step = 0.01;
    let buffer = decoded(ramp(frames, 2, step), 2, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "ramp.wav".to_string());
    let start = 40usize;
    sample.position = start;
    assert!(sample.set_speed(-0.5), "reverse speed must be accepted");

    let mut state = SceneBuilder::new(2).sample(sample).build();
    let render_frames = 40; // src_pos 40.0 -> 20.5, all in (0, 64)
    let out = render(&mut state, render_frames, 1);

    let speed = -0.5f64;
    for i in 0..render_frames {
        let src_pos = start as f64 + speed * i as f64;
        let expected = (src_pos as f32) * step;
        for ch in 0..2 {
            let got = out[i * 2 + ch];
            assert!(
                (got - expected).abs() < 1e-4,
                "frame {i} ch {ch}: src_pos {src_pos}, expected {expected}, got {got} \
                 (a frame_n-1 blend would read {})",
                ((src_pos - 2.0 * src_pos.fract()) as f32) * step
            );
        }
    }
}

#[test]
fn fractional_speed_interpolation_is_low_distortion() {
    // D27: the non-pitch speed path resamples by reading the source at fractional
    // positions. Two-point LINEAR interpolation is a poor reconstruction filter —
    // resampling a pure sine at a non-integer ratio folds significant energy into
    // spurious bins (audible as a gritty buzz at slow speeds). Cubic/Hermite
    // (Catmull-Rom) interpolation has a far flatter passband and much lower error,
    // so the same render stays close to a clean tone.
    //
    // Speed 0.61803 (≈ 1/golden ratio) is deliberately irrational-looking so the
    // fractional part sweeps the whole [0,1) interval rather than landing on a few
    // exact fractions — every interpolation phase is exercised. A pure low tone is
    // heavily oversampled, so an ideal resampler would emit essentially only the
    // fundamental; the spurious-to-fundamental ratio measures the interpolator.
    let block = 512;
    let blocks = 40;
    let speed = 0.618_034_f32;
    let amp = 0.5;
    // Choose the source frequency so the RESAMPLED tone (freq*speed) lands exactly
    // on a Goertzel analysis bin for the n = block*blocks output samples/channel,
    // so the single-bin fundamental read is not diluted by spectral leakage and the
    // spurious/fundamental ratio reflects interpolation error, not bin misalignment.
    let n = (block * blocks) as f32;
    let out_freq = 105.0 * SR as f32 / n; // exactly bin 105 (~246 Hz)
    let freq = out_freq / speed;
    let total = 48000 * 2; // plenty of source for the whole render at this speed
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    assert!(sample.set_speed(speed), "fractional speed must be accepted");
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let out = render(&mut state, block, blocks);

    // The fundamental is bin-aligned (see above), so its single-bin magnitude is a
    // faithful read. Total RMS captures fundamental + ALL distortion; subtracting
    // the fundamental's power leaves the broadband interpolation error (whatever its
    // spectral shape). Linear interpolation leaves a much larger residual than
    // cubic, so the residual-to-fundamental ratio is a shape-independent
    // discriminator (no reliance on guessing where the distortion bins fall).
    let fundamental = band_energy(&out, SR, 0, 2, out_freq);
    let total = rms(&out, 0, 2) * 2.0_f32.sqrt(); // amplitude-equivalent of total power
                                                  // band_energy normalizes a full-scale bin sine to ~amplitude, matching the
                                                  // amplitude-equivalent total above, so both are in the same units.
    let residual_power = (total * total - fundamental * fundamental).max(0.0);
    let residual = residual_power.sqrt() / fundamental;
    assert!(
        fundamental > 0.4,
        "resampled fundamental {fundamental} should be near the source amplitude {amp}"
    );
    // Linear interpolation leaves ~0.07 here; cubic drops it to ~0.01.
    assert!(
        residual < 0.02,
        "fractional-speed resampling is too dirty (linear interpolation): \
         residual/fundamental {residual} (fundamental {fundamental})"
    );
}

#[test]
fn reverse_interpolation_wraps_neighbor_at_loop_start() {
    // F9 + loop + D27: reverse playback interpolates frame_n..frame_n+1 with the
    // direction-independent forward neighbor, and the loop path resolves the cubic
    // taps periodically — the tap *before* frame 0 wraps to the last frame (a loop's
    // content is periodic), not edge-clamped and not silence. On this ramp fixture
    // (deliberately discontinuous at the seam: frame 31 = 0.62, frame 0 = 0.0) that
    // wrapped p0 is what makes the interpolated value distinctive, so the assertion
    // proves the wrap is in effect.
    let frames = 32;
    let step = 0.02;
    let buffer = decoded(ramp(frames, 1, step), 1, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "ramp.wav".to_string());
    sample.loop_mode = true;
    sample.position = 1; // start at frame 1
    assert!(sample.set_speed(-0.5));

    let mut state = SceneBuilder::new(1).sample(sample).build();
    let out = render(&mut state, 3, 1);

    // Frame 0: src_pos 1.0 -> exactly frame 1 -> 1*step (integer position, no interp).
    assert!((out[0] - step).abs() < 1e-4, "frame 0: got {}", out[0]);

    // Frame 1: src_pos 0.5, between frame 0 and frame 1. Catmull-Rom taps with the
    // periodic wrap are p0=frame[31]=31*step, p1=frame[0]=0, p2=frame[1]=step,
    // p3=frame[2]=2*step; the analytic result at t=0.5 is computed here so the test
    // pins the math, not the implementation's output. (A 2-point linear blend would
    // read 0.5*step; edge-clamping p0 to frame[0]=0 would read a positive value —
    // the wrapped p0 makes this distinctly negative.)
    let (p0, p1, p2, p3) = (31.0 * step, 0.0, step, 2.0 * step);
    let t = 0.5f32;
    let a0 = 2.0 * p1;
    let a1 = p2 - p0;
    let a2 = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let a3 = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let expected = 0.5 * (a0 + t * (a1 + t * (a2 + t * a3)));
    assert!(
        expected < 0.0,
        "wrapped-tap cubic should overshoot negative here"
    );
    assert!(
        (out[1] - expected).abs() < 1e-4,
        "frame 1 (between 0 and 1): got {}, expected wrapped-tap cubic {expected}",
        out[1]
    );
}

#[test]
fn limiter_is_transparent_below_threshold_and_smooth_above() {
    // A signal well under the knee passes through bit-for-bit (a soft-knee limiter,
    // unlike a global tanh soft-clip, does not attenuate clean signal). A constant
    // 0.5 buffer makes transparency exact, with no discrete-waveform fuzz.
    let frames = 4096;
    let quiet = decoded(vec![0.5f32; frames * 2], 2, SR);
    let s = ActiveSample::new(1, "q".to_string(), quiet, 1.0, 1.0, "q.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(s).build();
    let out = render(&mut state, frames, 1);
    assert!(
        out.iter().all(|&v| v == 0.5),
        "below the knee the limiter must be perfectly transparent (a global tanh \
         soft-clip would attenuate 0.5 to ~0.45)"
    );

    // A loud sine is limited to the ceiling with a smooth shape (no click): the
    // largest sample-to-sample step stays near the carrier's own slope, not a
    // clamp discontinuity.
    let ceiling = db_to_linear(-1.0);
    let loud = decoded(sine(220.0, SR, frames, 2, 1.6), 2, SR);
    let s = ActiveSample::new(1, "l".to_string(), loud, 1.0, 1.0, "l.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(s).build();
    let out = render(&mut state, frames, 1);
    assert!(
        peak(&out) <= ceiling + 1e-4,
        "loud peak {} exceeds ceiling {ceiling}",
        peak(&out)
    );
    let natural_slope = ceiling * std::f32::consts::TAU * 220.0 / SR as f32;
    let delta = max_inter_sample_delta(&out, 0, 2);
    assert!(
        delta < natural_slope * 2.0 + 0.01,
        "limiter introduced a discontinuity: delta {delta}, slope bound {natural_slope}"
    );
}

/// Deterministic white-ish noise in [-amplitude, amplitude], one channel.
/// Independent samples mean the head and tail of the buffer are uncorrelated, so
/// the loop crossfade sums two uncorrelated signals — the case where a linear
/// crossfade dips ~3 dB at the midpoint and an equal-power one stays flat.
fn noise(frames: usize, amplitude: f32, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut data = Vec::with_capacity(frames);
    for _ in 0..frames {
        // SplitMix64 step → a uniform bit pattern, mapped to [-1, 1).
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let unit = (z >> 40) as f32 / (1u32 << 24) as f32; // [0, 1)
        data.push((unit * 2.0 - 1.0) * amplitude);
    }
    data
}

#[test]
fn loop_crossfade_holds_level_through_the_midpoint() {
    // F5/D28: across the loop crossfade the tail fades out while the head fades
    // in. For uncorrelated material a linear blend dips ~3 dB at the midpoint; an
    // equal-power (cos/sin) blend keeps the summed power flat. The whole buffer is
    // independent noise, so the head [0..cf] and the crossfaded tail are
    // uncorrelated and the steady region's RMS is the reference level.
    let frames = 16000;
    let cf = 4000; // buffer_frames > cf*2, so the crossfade engages
    let amp = 0.3; // RMS ~0.17; even summed in the crossfade stays under the knee
                   // Long regions keep each segment's RMS estimate tight (segment
                   // RMS std ~1/sqrt(2*N)), so the equal-power result lands well
                   // inside ~±0.5 dB rather than riding statistical noise variance.
    let buffer = decoded(noise(frames, amp, 0xC0FF_EE11), 1, SR);
    let sample = ActiveSample::new_with_id(
        1,
        "v".to_string(),
        buffer,
        1.0,
        1.0,
        "noise.wav".to_string(),
        None,
        true, // loop_mode
        cf,   // crossfade_samples
    );
    let mut state = SceneBuilder::new(1).sample(sample).build();

    // One full pass: src_pos 0..3999. Steady region [cf .. frames-cf] has no
    // crossfade; the crossfade occupies [frames-cf .. frames].
    let out = render(&mut state, frames, 1);

    let steady = rms(&out[cf..frames - cf], 0, 1);

    // Measure the middle half of the crossfade, where progress ~0.25..0.75 — the
    // dip zone. A linear blend averages ~ -2.4 dB here; equal-power stays ~0 dB.
    let cf_start = frames - cf;
    let lo = cf_start + cf / 4;
    let hi = cf_start + 3 * cf / 4;
    let through_cf = rms(&out[lo..hi], 0, 1);

    let db = 20.0 * (through_cf / steady).log10();
    assert!(
        db.abs() < 0.5,
        "crossfade level {db:.2} dB off steady (linear dips ~ -2.4 dB here); \
         steady rms {steady}, crossfade rms {through_cf}"
    );

    // The equal-power blend must not introduce a discontinuity at the seam: the
    // largest sample-to-sample step stays comparable to the noise's own jumps in
    // the steady region (noise is broadband, so its natural step is sizable).
    let steady_delta = max_inter_sample_delta(&out[cf..frames - cf], 0, 1);
    let cf_delta = max_inter_sample_delta(&out[cf_start..frames], 0, 1);
    assert!(
        cf_delta < steady_delta * 1.5 + 1e-4,
        "crossfade seam click: cf delta {cf_delta}, steady delta {steady_delta}"
    );
}

#[test]
fn loop_crossfade_does_not_replay_the_overlapped_head() {
    // F6/D28: the loop crossfade mixes the head [0..cf] into the tail (fading in).
    // The buggy wrap restarts the next pass at frame 0, replaying that head at
    // FULL level — so a transient in the head is double-triggered every loop. A
    // true overlap-add starts the next pass past the overlapped head (at cf), so
    // the head is heard at full level only on the very first pass; thereafter it
    // arrives once per loop via the crossfade fade-in.
    let frames = 1000;
    let cf = 200; // frames > cf*2
    let impulse = 50; // inside the crossfaded head region [0..cf]
    let amp = 0.8; // below the limiter knee, so it passes at full amplitude
    let buffer = decoded(transient_loop(frames, 1, impulse, amp), 1, SR);
    let sample = ActiveSample::new_with_id(
        1,
        "v".to_string(),
        buffer,
        1.0,
        1.0,
        "impulse.wav".to_string(),
        None,
        true, // loop_mode
        cf,   // crossfade_samples
    );
    let mut state = SceneBuilder::new(1).sample(sample).build();

    // ~6 loop periods. Position advances across blocks (advance_position) and
    // within a block (the inline wrap), so both wrap sites are exercised.
    let out = render(&mut state, 500, 10);

    // The head impulse at full amplitude must appear exactly once (the first
    // pass). Its faded crossfade copy is sin(impulse/cf * PI/2) * amp ≈ 0.31, well
    // under this 0.5*amp threshold, so the count isolates full-level hits. The bug
    // replays it at full level once per loop (~6 times).
    let full_threshold = 0.5 * amp;
    let full_hits = out.iter().filter(|&&v| v > full_threshold).count();
    assert_eq!(
        full_hits, 1,
        "head impulse replayed at full level {full_hits} times (overlap-on-wrap \
         must replay it 0 extra times; the bug replays it once per loop)"
    );
}

#[test]
fn loop_crossfade_seam_is_click_free() {
    // F6: with overlap-on-wrap the next pass continues from frame cf, so the last
    // crossfaded output (≈ head[cf-1]) is followed by head[cf] — a contiguous
    // sample. The buggy wrap-to-0 instead jumps to head[0], a discontinuity
    // wherever head[0] != head[cf-1] (a seam click). A 100 Hz sine whose period
    // (480) does not divide cf makes head[0] and head[cf-1] differ.
    let frames = 1000;
    let cf = 200;
    let amp = 0.6;
    let freq = 100.0;
    let buffer = decoded(sine(freq, SR, frames, 1, amp), 1, SR);
    let sample = ActiveSample::new_with_id(
        1,
        "v".to_string(),
        buffer,
        1.0,
        1.0,
        "sine.wav".to_string(),
        None,
        true,
        cf,
    );
    let mut state = SceneBuilder::new(1).sample(sample).build();
    let out = render(&mut state, 500, 10); // several loop periods

    // The only steps present should be the sine's own slope plus the gentle
    // equal-power crossfade curvature — never a wrap discontinuity.
    let natural_slope = amp * std::f32::consts::TAU * freq / SR as f32;
    let delta = max_inter_sample_delta(&out, 0, 1);
    assert!(
        delta < natural_slope * 4.0,
        "loop seam click: max inter-sample delta {delta}, natural slope {natural_slope} \
         (the wrap-to-0 bug jumps head[cf-1]->head[0], ~0.3 here)"
    );
}

#[test]
fn enabling_pitch_correction_mid_playback_has_no_silent_gap() {
    // F10: enabling pitch correction on the None->Some transition builds a fresh
    // stretcher whose input latency (~2880 frames at 48k) spans ~6 callback blocks.
    // Without a pre-roll EVERY one of those first blocks is exactly warm-up silence
    // (rms 0.0) — an audible gap. Pre-rolling the stretcher with the audio just
    // before the playback position primes it so signal is present from the very
    // first post-enable block and reaches full level within a couple of blocks.
    //
    // The stretcher still applies its own short synthesis fade-in on its first
    // output block (block 0 is quiet but non-zero); that sub-block ramp is inherent
    // to the overlap-add and is not the defect. The defect is the multi-block
    // silence, which the pre-roll removes.
    let block = 512;
    let amp = 0.5;
    let freq = 220.0;
    // Two seconds so there is plenty of audio both before and after the enable.
    let total = block * 160;
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    // Play a while with no pitch correction so the playback position is deep into
    // the buffer (well past the stretcher's input-latency worth of pre-roll).
    let warm = render(&mut state, block, 40);
    let warm_rms = rms(&warm, 0, 2);
    assert!(
        warm_rms > 0.2,
        "pre-enable tone should be present: {warm_rms}"
    );

    // Enable pitch correction mid-playback, exactly as a live `speed` command does
    // (set_speed_with_mode -> enable_pitch_correction on the None->Some edge).
    assert!(
        state.active_samples[0].set_speed_with_mode(0.9, true),
        "enabling pitch correction at a forward speed must succeed"
    );

    // Render the blocks spanning the stretcher's warm-up window.
    let post = render(&mut state, block, 6);

    // Block 0 must carry signal, not the bug's exact silence. Without the pre-roll
    // this is 0.0; the pre-roll makes it clearly non-zero (the stretcher's own
    // first-block fade-in keeps it modest, so the floor is small but unambiguous).
    let first_rms = rms(&post[..block * 2], 0, 2);
    assert!(
        first_rms > 0.005,
        "first post-enable block is warm-up silence (no pre-roll): rms {first_rms}"
    );

    // The window that was a dead gap (the first three blocks) must now carry real
    // energy. The bug leaves all three silent (rms 0.0); the pre-roll fills them.
    let early_rms = rms(&post[..3 * block * 2], 0, 2);
    assert!(
        early_rms > 0.08,
        "the post-enable warm-up window is still a silent gap: 3-block rms {early_rms}"
    );

    // And the level must recover to near steady-state quickly: by the 4th block the
    // pitch-corrected tone is essentially at full level (the bug is still silent
    // here). amp/sqrt(2) ≈ 0.354 is the steady RMS; require most of it.
    let recovered_rms = rms(&post[3 * block * 2..4 * block * 2], 0, 2);
    assert!(
        recovered_rms > 0.25,
        "pitch-corrected level did not recover after enable: block-3 rms {recovered_rms}"
    );

    // The transition must not click: the seam between the last pre-enable sample and
    // the first post-enable sample stays within a few carrier slopes (the stretcher
    // is not sample-accurate, so allow headroom, but well below a silence cliff).
    let mut seam = Vec::new();
    seam.extend_from_slice(&warm[warm.len() - 2 * 2..]); // last frame (stereo)
    seam.extend_from_slice(&post[..2 * 2]); // first two frames
    let natural_slope = amp * std::f32::consts::TAU * freq / SR as f32;
    let seam_delta = max_inter_sample_delta(&seam, 0, 2);
    assert!(
        seam_delta < natural_slope * 8.0 + 0.05,
        "pitch-enable transition clicked: seam delta {seam_delta}, slope {natural_slope}"
    );
}

#[test]
fn pitch_correction_stays_phase_continuous_at_non_integer_product() {
    // F11: the pitch path feeds the stretcher ceil(frames*speed) input frames but
    // advances the read position by the *fractional* frames*speed. At a non-integer
    // product (block 512 x speed 0.7 = 358.4) the slice start drifts from where the
    // previous slice ended, so each block re-feeds ~0.6 frame of overlapping source.
    // The stretcher absorbs that as a recurring phase perturbation: a pure input
    // tone comes out with its fundamental partly smeared into sidebands. Advancing
    // the position by the integer frames actually fed (carrying the fractional
    // remainder, so slices abut exactly) keeps the output a clean tone.
    //
    // 512 * 0.7 = 358.4 is the spec's prescribed non-integer product; the masked
    // integer products 512*{2.0,1.5,0.5} hide the bug. The discriminator is spectral
    // purity, not inter-sample delta (the stretcher smooths the sub-frame jitter, so
    // a steady tone's max step barely moves; the energy that leaves the fundamental
    // is what the bug reveals). Measured fixed-vs-buggy at this exact condition:
    // fundamental 0.500 vs 0.407, off-tone/fundamental ratio 0.0003 vs 0.008.
    let block = 512;
    let speed = 0.7f32;
    let amp = 0.5;
    let freq = 200.0;
    let total = 48000 * 4; // long enough for a tight spectral estimate
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    sample.enable_pitch_correction();
    assert!(sample.set_speed(speed));
    let mut state = SceneBuilder::new(2).sample(sample).build();

    // Warm past the stretcher startup latency (~2880 frames ≈ 6 blocks), then keep
    // many steady-state blocks to measure.
    let _ = render(&mut state, block, 16);
    let out = render(&mut state, block, 60);

    // Pitch is preserved, so the fundamental stays at the input frequency. With
    // slices abutting it comes through essentially intact (~full amplitude); the bug
    // bleeds ~19% of it into sidebands.
    let fundamental = band_energy(&out, SR, 0, 2, freq);
    assert!(
        fundamental > 0.46,
        "pitch-corrected fundamental {fundamental} is depleted (input re-fed, energy \
         smeared); expected near the source amplitude {amp}"
    );

    // Off-tone bins (no harmonic relation to the tone) must stay near silence. The
    // recurring re-feed of the bug raises this by ~20x relative to the fundamental.
    let off = band_energy(&out, SR, 0, 2, freq * 1.37)
        .max(band_energy(&out, SR, 0, 2, freq * 2.53))
        .max(band_energy(&out, SR, 0, 2, freq * 3.71));
    let purity = off / fundamental;
    assert!(
        purity < 0.003,
        "non-integer pitch product smeared the tone: off/on ratio {purity} \
         (off {off}, fundamental {fundamental})"
    );

    // Sanity: the output is present and bounded (no runaway from the position math).
    let r = rms(&out, 0, 2);
    assert!(
        r > 0.2 && r < 0.6,
        "pitch-corrected steady tone RMS {r} should sit near the source level"
    );
}

#[test]
fn pitch_correction_flushes_the_tail_at_eof() {
    // F12: near EOF the pitch path feeds the last partial input, but the stretcher
    // still holds ~output_latency frames of buffered tail. The old code overshoots
    // the position via advance_position and the next callback sees zero available
    // input and returns silence, truncating that tail. Flushing/draining the
    // stretcher at EOF emits the buffered tail instead.
    //
    // A short buffer (so EOF arrives quickly) of a steady tone; after the source is
    // exhausted, continued rendering must still produce non-trivial energy (the
    // drained tail) rather than immediate silence.
    let block = 512;
    let amp = 0.5;
    let freq = 220.0;
    // ~0.25 s of source — long enough to clear startup latency, short enough that
    // EOF and the flushed tail both occur within the render window.
    let total = 12000;
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    sample.enable_pitch_correction();
    assert!(sample.set_speed(1.0)); // unity ratio: input frames ~= output frames

    let mut state = SceneBuilder::new(2).sample(sample).build();

    // Render enough blocks to consume the whole source and then drain the tail.
    // At unity ratio the source (12000 frames) is consumed after ~24 blocks; the
    // stretcher then still holds ~output_latency (2880) frames of tail.
    let blocks = 40;
    let out = render(&mut state, block, blocks);

    // The bug truncates at the block where input runs out: the sample finishes and
    // is retained out, so every block AFTER input exhaustion is pure silence and the
    // buffered tail is lost. The window strictly past the source's worth of input
    // (block 24 -> output frame 24*512 = 12288) is therefore silent under the bug;
    // a flush drains the buffered tail into those blocks, so it carries energy.
    let input_blocks = total.div_ceil(block); // 24 blocks fed the source
    let tail_start = input_blocks * block * 2; // stereo
    assert!(
        tail_start < out.len(),
        "render window must extend past input exhaustion so the tail is observable"
    );
    let tail = &out[tail_start..];
    let tail_rms = rms(tail, 0, 2);
    assert!(
        tail_rms > 0.02,
        "pitch tail was truncated at EOF (no flush): post-input tail rms {tail_rms}"
    );
}

#[test]
fn pitch_correction_flushes_the_tail_when_source_is_an_exact_block_multiple() {
    // F12 edge case: when the source length is an exact multiple of the per-block
    // consumption, the last block consumes *exactly* to the end. The position then
    // lands on buffer.frames() via the normal feed path; if EOF is only recognised
    // when the requested input *exceeds* what remains, that block never arms the tail
    // drain and `is_finished` retires the sample before the tail is emitted. The pitch
    // path takes the EOF/flush path as soon as a block reaches the end, so the tail is
    // drained here too.
    let block = 512;
    let amp = 0.5;
    let freq = 220.0;
    let total = 24 * block; // 12288 frames: an exact block multiple at unity speed
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let mut sample =
        ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    sample.enable_pitch_correction();
    assert!(sample.set_speed(1.0));

    let mut state = SceneBuilder::new(2).sample(sample).build();
    let out = render(&mut state, block, 40);

    // Past the last input block the tail must still carry energy (it would be silent
    // if the exact-boundary block had finished the sample without draining).
    let tail_start = (total / block) * block * 2;
    let tail = &out[tail_start..];
    assert!(
        rms(tail, 0, 2) > 0.02,
        "exact-multiple source truncated the pitch tail: tail rms {}",
        rms(tail, 0, 2)
    );
}

#[test]
fn duck_fade_advances_once_per_buffer_regardless_of_sample_count() {
    // D1: a ducked voice's fade must advance once per buffer, not once per sample on
    // that voice. Render the SAME duck (target 0.0 over a fade of N blocks) twice:
    // once with a single sample on the voice, once with TWO. Over several blocks the
    // per-voice duck gain must track the SAME ramp regardless of sample count — with
    // the per-sample bug the two-sample voice advances its fade twice as fast and is
    // already far more ducked than the single-sample voice.
    let block = 512;
    let blocks = 10;
    let fade = block * 20; // fully ducks over 20 blocks; 10 blocks reaches ~halfway
                           // Keep two summed samples (2 * value) below the limiter knee so the comparison
                           // measures the duck rate, not limiter compression.
    let value = 0.3;

    // One sample on the ducked voice.
    let one = applier_ducking("music", 0.0, fade);
    let mut state_one = SceneBuilder::new(2)
        .sample(constant_sample(1, "music", value, block * (blocks + 2)))
        .ducking(one)
        .build();
    let out_one = render(&mut state_one, block, blocks);

    // Two overlapping samples on the SAME ducked voice.
    let two = applier_ducking("music", 0.0, fade);
    let mut state_two = SceneBuilder::new(2)
        .sample(constant_sample(1, "music", value, block * (blocks + 2)))
        .sample(constant_sample(2, "music", value, block * (blocks + 2)))
        .ducking(two)
        .build();
    let out_two = render(&mut state_two, block, blocks);

    // Per-voice duck gain at the end of the render. Single sample: level == value *
    // gain. Two samples sum, so divide by 2 to recover the same per-voice gain.
    let last = (block * blocks - 1) * 2;
    let gain_one = out_one[last] / value;
    let gain_two = (out_two[last] / value) / 2.0;
    assert!(
        (gain_one - gain_two).abs() < 1e-3,
        "duck advanced at different rates for 1 vs 2 samples on a voice: \
         gain_one {gain_one}, gain_two {gain_two} (fade must advance once per buffer)"
    );

    // After 10 of 20 fade-blocks the voice is about halfway ducked. This pins the
    // rate: the per-sample bug would have advanced ~20 blocks and be near 0.
    assert!(
        (gain_one - 0.5).abs() < 0.05,
        "after half the fade the duck gain should be ~0.5, got {gain_one}"
    );
}

#[test]
fn duck_gain_is_smooth_per_frame_without_buffer_stairstep() {
    // D2: the duck gain must interpolate per frame, not jump only at buffer
    // boundaries. With a constant source the output equals value * duck_gain, so any
    // per-buffer stairstep shows up as a large sample-to-sample jump exactly at a
    // block boundary. Render a fast duck across several blocks and assert the largest
    // inter-sample step is tiny (a smooth ramp), not a per-block stair.
    let block = 256;
    let fade = block * 8; // fully ducks over 8 blocks
    let value = 0.5;

    let applier = applier_ducking("music", 0.0, fade);
    let mut state = SceneBuilder::new(2)
        .sample(constant_sample(1, "music", value, block * 16))
        .ducking(applier)
        .build();
    let out = render(&mut state, block, 8);

    // The ideal per-frame step of a linear fade from value*1.0 to 0 over `fade`
    // frames is value/fade. A per-buffer stairstep would jump ~value/8 at each of the
    // 8 boundaries — orders of magnitude larger. Allow a few ideal steps of slack.
    let ideal_step = value / fade as f32;
    let delta = max_inter_sample_delta(&out, 0, 2);
    assert!(
        delta < ideal_step * 4.0,
        "duck gain stairsteps per buffer: max delta {delta}, ideal per-frame step {ideal_step}"
    );

    // Sanity: it did duck substantially over the 8 blocks (so the smoothness above is
    // not the trivial no-change case).
    let last = (block * 8 - 1) * 2;
    assert!(
        out[last] < value * 0.2,
        "the voice should be well ducked after the full fade, got {}",
        out[last]
    );
}

#[test]
fn shipped_corrector_enable_matches_the_no_gap_behavior() {
    // D56 parity: enabling pitch correction through the control-side-built
    // PitchBundle (apply_shipped_speed — the running daemon's path) must show the
    // same no-gap pre-roll behavior as enable_pitch_correction (the F10 contract
    // asserted above).
    use mqttaudio::audio::mixer::PitchBundle;

    let block = 512;
    let amp = 0.5;
    let freq = 220.0;
    let total = block * 160;
    let buffer = decoded(sine(freq, SR, total, 2, amp), 2, SR);
    let sample = ActiveSample::new(1, "v".to_string(), buffer, 1.0, 1.0, "tone.wav".to_string());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let warm = render(&mut state, block, 40);
    assert!(rms(&warm, 0, 2) > 0.2, "pre-enable tone should be present");

    // The dispatcher builds the bundle (channels/rate/speed/max block) and ships
    // it; the audio thread installs it with pure moves.
    let mut bundle = Some(Box::new(PitchBundle::for_voice(2, SR, 0.9, block)));
    let mut displaced = None;
    assert!(
        state.active_samples[0].apply_shipped_speed(0.9, true, &mut bundle, &mut displaced),
        "the shipped enable must accept the speed"
    );
    assert!(
        bundle
            .as_ref()
            .map(|b| b.corrector.is_none())
            .unwrap_or(false),
        "the corrector is taken; the box stays for the husk ride home"
    );

    let post = render(&mut state, block, 6);
    let first_rms = rms(&post[..block * 2], 0, 2);
    assert!(
        first_rms > 0.005,
        "first post-enable block must not be warm-up silence: rms {first_rms}"
    );
    let early_rms = rms(&post[..3 * block * 2], 0, 2);
    assert!(
        early_rms > 0.08,
        "the warm-up window must carry energy: rms {early_rms}"
    );
    let recovered_rms = rms(&post[3 * block * 2..4 * block * 2], 0, 2);
    assert!(
        recovered_rms > 0.25,
        "level must recover after the shipped enable: rms {recovered_rms}"
    );

    // Disabling moves the corrector out instead of dropping it on the caller.
    let mut none_bundle = None;
    let mut displaced = None;
    assert!(state.active_samples[0].apply_shipped_speed(
        1.0,
        false,
        &mut none_bundle,
        &mut displaced
    ));
    assert!(
        displaced.is_some(),
        "the displaced corrector must ride out for off-RT drop"
    );
}
