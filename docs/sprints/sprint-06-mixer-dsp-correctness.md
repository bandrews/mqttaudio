# Sprint 6 — Mixer DSP Correctness

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 5 |
| Effort | L |
| Lanes | A |
| Subagents | YES (independent DSP fixes parallelize) |

## Goal

Make the mixer's signal path **correct**: fix the reverse-interpolation error, sanitize non-finite samples,
apply the per-channel calibration that is currently dead config, clamp the Play `volume`, replace the bare
hard clamp with a real limiter, make the loop crossfade equal-power and seamless, repair the pitch-correction
enable/advance/flush behavior, and fix ducking so it advances once per buffer, smooths per-frame, honors the
configured restore fade, and can be triggered by live inputs. Every claim is proven with an assertion on the
Sprint 0 render harness (`rms` / `peak` / `band_energy` / `max_inter_sample_delta`).

This sprint operates on the **lock-free engine delivered by Sprint 5**: the audio thread owns `MixerState`,
the voice pool and pitch scratch are pre-allocated, ducking notify and voice bookkeeping run off the RT
thread, and `/status` is served from a snapshot. We change DSP *math and ordering* inside `mix_audio` and its
helpers; we do **not** re-introduce locks/allocations in the callback. If a fix needs new state, it lives in
the pre-allocated, audio-thread-owned structures Sprint 5 established.

## Why

The DSP audit found nine confirmed/likely-confirmed correctness defects in the mix path plus a cluster of
ducking-behavior bugs. Several are continuous (reverse interpolation, ducking fade-rate) rather than edge
cases. None can be proven today because there was no offline render harness — Sprint 0 built one, and this
sprint is where it earns its keep. The features most likely to glitch (ducking with overlapping samples,
mid-playback pitch correction, looped ambience) had **zero** integration coverage before Sprint 0.

## Scope

**In scope**
- Final per-output-channel calibration gain stage (`channel_volumes`).
- Non-finite (NaN/Inf) sample sanitization in the final output loop.
- Play `volume` clamping at `ActiveSample` construction.
- A true-peak / soft-knee limiter (interim tanh soft-clip acceptable) with configurable ceiling + master
  gain on the f32 bus, replacing the bare hard clamp; plus a clip/over counter exposed to `/status`.
- Reverse-playback interpolation neighbor fix.
- Equal-power loop crossfade with correct overlap-on-wrap; consider equal-power fade in/out.
- Pitch-correction: pre-roll on enable, position-advance aligned to frames consumed, tail flush at EOF.
- Ducking: advance once per buffer, interpolate per-frame, honor `fade_duration_ms` on restore, allow
  live-input voices to trigger ducking, single source of activity truth, gate `begin_duck` on change.
- Down/upmix per-route gain (optional, low) and a documented decision on speed>1 anti-aliasing.

**Out of scope** (later sprints)
- Bass-management crossover / LFE work → **Sprint 7** (the NaN guard here protects the bass IIR, but the
  Linkwitz-Riley/LFE-gain fixes are Sprint 7).
- Live-input *activity detection* (ring-buffer level gate / input-gate) → **Sprint 8**. Here we implement
  only the ducking-side notify path so it works the moment input activity is reported.
- Full `/status` telemetry enrichment (per-voice duck state, dropouts, `/metrics`) → **Sprint 9**. We add a
  single clip/over `AtomicU64` and surface it; broad observability is Sprint 9.
- Resampler `Cubic` upgrade is a documented, optional polish item, not a required behavior change.

## Findings addressed

Severity is the audit's reconciled verdict. `file:line` re-verified against current source on this branch.

### F1 — Per-channel calibration (`channel_volumes`) is dead config — HIGH (confirmed)
- **Statement:** `audio.channel_volumes` parses and validates but is never applied to output.
- **Where:** declared `src/config.rs:50`, defaulted `:150`, validated `:702-706`
  (`"audio.channel_volumes.{} must be between 0.0 and 1.0"`); aliases `src/config.rs:53`. No reference in
  `mixer.rs` / `engine.rs` / `main.rs`. `MixerState` (`src/audio/mixer.rs:509-524`) has no per-channel gain
  field; the final loop is only `*s = s.clamp(-1.0, 1.0)` (`mixer.rs:585-587`).
- **Evidence:** `{"channel_volumes": {"front_left": 0.5}}` is accepted and has zero audible effect.
- **Fix:** apply `channel_volumes` as a **final per-output-channel gain pass** in `mix_audio`, before the
  limiter/clamp. Resolve aliases → output channel indices **once** (at `MixerState` construction, off the RT
  thread) into a `Vec<f32>` of length `output_channels` (default 1.0). Multiply per channel in the final loop.

### F2 — Output overload protection is a brickwall hard clamp (mislabeled "saturation") — MEDIUM (confirmed)
- **Statement:** summed bus is hard-clamped, injecting harmonic distortion/aliasing past unity.
- **Where:** `src/audio/mixer.rs:584-587` — comment "Apply saturation to prevent clipping" then
  `*s = s.clamp(-1.0, 1.0)`. `test_saturation` (`mixer.rs:989-1008`) asserts two 0.8 sources sum to exactly
  1.0.
- **Evidence:** hard clipping flat-tops the waveform when many sources sum > 0 dBFS.
- **Fix:** replace with a true-peak / soft-knee limiter with a **configurable ceiling** plus an optional
  **master gain** on the f32 bus. An interim **tanh soft-clip** is acceptable as the first implementation. A
  final hard clamp stays only as the last safety net at the ceiling. Expose a clip/over `AtomicU64` counter.

### F3 — No NaN/non-finite guard; `f32::NAN.clamp()` returns NaN — MEDIUM (confirmed)
- **Statement:** a single non-finite sample reaches the DAC and can poison the bass IIR.
- **Where:** samples read via `get_sample_or_silence` and accumulated `output[dest_idx] += blended_val *
  final_volume` (`mixer.rs:672, 749`) with no finite check; final loop only clamps (`mixer.rs:585-587`).
  Rust `f32::NAN.clamp(-1.0, 1.0)` returns NaN (unordered comparisons), so the clamp does **not** sanitize.
- **Evidence:** corrupt file / decoder / future pitch-or-resampler overflow → NaN → click/pop and persistent
  garbage once it enters the bass biquad state (Sprint 7's IIR).
- **Fix:** in the final loop, `*s = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };` (combined with the
  F2 limiter so the finite-guard runs first). Cheap, branch-predictable, callback-safe.

### F4 — Play `volume` applied unbounded while runtime `Volume` clamps — MEDIUM. **BEHAVIOR CHANGE**
- **Statement:** Play accepts `volume > 1.0` with no clamp; runtime `Volume` clamps. Inconsistent.
- **Where:** Play destructured `src/main.rs:650`; `volume` passed unbounded into
  `ActiveSample::new_with_mapping` (`main.rs:731`) and `new_with_id` (`main.rs:745`).
  `src/mqtt/commands.rs:432` sets `volume: play_msg.volume.unwrap_or(1.0)` — no upper bound. Contrast runtime
  `Volume` (`main.rs:1136`) which clamps at `main.rs:1150` (`sample.volume = volume.clamp(0.0, 1.0)`).
- **Evidence:** `{"volume": 5.0}` → 5× contribution → straight into the limiter/clamp.
- **Fix:** clamp Play `volume` to `[0,1]` **at `ActiveSample` construction** (clamp `self.volume` inside the
  `new*` constructors so all three paths are covered in one place). Note in changelog.

### F5 — Linear (not equal-power) fades + loop crossfade → ~3 dB midpoint dip — MEDIUM
- **Statement:** linear gain ramps cause a level dip at crossfade midpoint and perceptually uneven fades.
- **Where:** `FadeState::multiplier` is linear (`mixer.rs:42-58`: fade-in `elapsed/duration` line 49,
  fade-out `1.0 - elapsed/duration` line 55). Loop crossfade blends linearly:
  `interpolated_val * (1.0 - progress) + blend_val * progress` (`mixer.rs:739`, reverse mirror `:725`).
- **Evidence:** uncorrelated material → ~ -3 dB amplitude dip at crossfade midpoint.
- **Fix:** **equal-power** crossfade (`gain_a = cos(progress * PI/2)`, `gain_b = sin(progress * PI/2)`).
  Consider equal-power (or documented-linear) fade in/out. Precompute `PI` constant; no per-sample alloc.

### F6 — Loop crossfade overlaps the head, then replays it at full level after wrap — MEDIUM
- **Statement:** the first `cf_samples` are mixed into the tail during crossfade **and** replayed at unity
  after the wrap (heard twice → flam/level bump at the loop point).
- **Where:** forward crossfade blends `blend_frame = frames_into_crossfade` i.e. head frames `[0..cf_samples]`
  (`mixer.rs:735-739`); the wrap in `advance_position` sets `position = new_pos % buffer_frames`
  (`mixer.rs:406-410`), so playback continues from frame 0 and the head plays again at full level.
- **Evidence:** rhythmic/transient loops get an audible double-trigger of the head each iteration.
- **Fix:** true overlap-add: on wrap, **start the next pass past the overlapped head** (begin at frame
  `cf_samples`) so the blended region is not replayed. Verify with a transient test loop.

### F7 — Down/upmix sums source channels with no attenuation — LOW
- **Statement:** mapping N source channels into one destination sums at unity → clipping risk.
- **Where:** `output[dest_idx] += blended_val * final_volume` (`mixer.rs:749`); `test_quad_to_stereo_downmix`
  (`mixer.rs:1234-1263`) asserts L = 0.1+0.3 = 0.4, R = 0.2+0.4 = 0.6 (no -3/-6 dB compensation).
- **Fix:** optional **per-route gain** in the channel map (extend `Vec<(usize,usize)>` to carry a gain, or a
  sidecar gain vector). Low priority; if not implemented, **document** the no-attenuation behavior.

### F8 — Speed > 1.0 has no anti-aliasing; only linear interpolation — LOW
- **Statement:** stride-based speed-up downsamples with no low-pass → aliasing.
- **Where:** `src_pos += speed` with 2-point linear interpolation (`mixer.rs:675-711, 756`); `set_speed`
  allows magnitudes to 100.0 without pitch correction (`mixer.rs:278`).
- **Fix:** either band-limited/cubic interpolation for the fast path, or **cap** the max non-pitch speed well
  below 100× and **document** the lo-fi tradeoff. Decision required (see Caveats); default to documenting +
  a sane cap unless DECISIONS.md changes it the resampler routed in.

### F9 — Reverse playback interpolates with the WRONG neighbor — HIGH (confirmed)
- **Statement:** reverse uses `src_frame-1` instead of `src_frame+1`; correct is
  `frame_n*(1-frac) + frame_{n+1}*frac` **regardless of direction**.
- **Where:** `frac = (src_pos - src_frame as f64).abs()` (`mixer.rs:660`); reverse branch picks
  `prev_frame = src_frame - 1` and returns `sample_val*(1-frac) + prev_val*frac` (`mixer.rs:676-691`). Since
  `src_frame = src_pos as usize` floors, position 8.3 lies between frames 8 and 9; the reverse branch blends
  8 with 7 (wrong side). Forward branch (`mixer.rs:695-704`) is correct.
- **Evidence:** continuous comb/low-pass distortion across **all** reverse playback where `frac > 0.001`
  (any non-integer reverse speed, e.g. -0.5x/-1.5x/-0.75x, and -1.0x once fractional position is nonzero).
- **Fix:** remove the `is_reverse` neighbor special-case. Always interpolate between `src_frame` and
  `src_frame+1` with `frac = src_pos.fract()` (wrap `src_frame+1 → 0` when looping). The loop-crossfade
  reverse branch still keys on direction; only the **interpolation neighbor** is direction-independent.

### F10 — Enabling pitch correction mid-playback builds a fresh ~120 ms-latency stretcher with no pre-roll — HIGH (confirmed)
- **Statement:** `None→Some` transition warms up a new stretcher with no pre-roll → audible silent gap.
- **Where:** `enable_pitch_correction` builds `PitchCorrector::new` only when `is_none()`
  (`mixer.rs:286-295`) → `Stretch::preset_default` (`pitch_correction.rs:17`, ~120 ms block / 30 ms interval).
  The mix path calls `pc.process(input_slice, &mut stretched)` directly with no prior pre-roll
  (`mixer.rs:801-803`). Triggered live by `set_speed_with_mode` on every matching active sample
  (`main.rs:1086`). The wrapper exposes `reset()` (`pitch_correction.rs:67-70`) but **no `seek`/pre-roll**
  method, and `reset` is never called in the production path.
- **Evidence:** first ~`input_latency` frames of corrected output are warm-up state → silence/garble gap.
- **Fix:** pre-roll on enable. The signalsmith-stretch crate provides a pre-roll/seek primitive ("add
  pre-roll to the output"); add a `PitchCorrector::preroll(&pre_samples)` (or `seek`) wrapper and call it in
  `enable_pitch_correction` using the samples just before `sample.position`, **or** crossfade direct↔stretched
  over a few ms. Pre-allocate the stretcher scratch (Sprint 5 owns the pre-alloc; here just use it — do not
  `vec![0.0; …]` in the callback as `mixer.rs:798` currently does).

### F11 — Pitch position advance (`frames*speed`) diverges from input consumed (`ceil`) — MEDIUM
- **Statement:** at non-integer speeds the stretcher is re-fed overlapping/skipped input → phase
  discontinuities.
- **Where:** `input_frames_needed = ((frames as f32 * speed).ceil()).max(1)` (`mixer.rs:781`); slice taken
  from `sample.position` (`mixer.rs:792-794`); position **not** advanced here (`mixer.rs:834`) but by
  `sample.advance_position(frames)` in `mix_audio` (`mixer.rs:564`), which advances by `output_frames*speed`
  (`advance_position` `mixer.rs:394-429`). So consumed = `ceil(frames*speed)` but slice-start advances by the
  fractional `frames*speed`.
- **Evidence:** e.g. speed 0.05, frames 512 → 26 fed but position advances 25.6 → ~1 frame/callback drift.
  Masked by integer products (2.0/1.5/0.5 at buffer 512), which is why current tests pass.
- **Fix:** advance `sample.position` by the **integer input frames actually fed** to the stretcher, carrying
  the fractional remainder in a dedicated accumulator on the (audio-thread-owned) `ActiveSample`, so each
  input slice begins exactly where the previous ended.

### F12 — Pitch tail truncation — stretcher never flushed at EOF — LOW
- **Statement:** the last ~`input_latency` buffered frames are never emitted.
- **Where:** near EOF `input_frames` is clamped (`mixer.rs:784-785`) but output length stays `frames` and
  position overshoots via `advance_position`; next callback `available_frames == 0 → return`
  (`mixer.rs:787-789`). No flush.
- **Fix:** when `available_frames < input_frames_needed`, advance only by frames consumed and **flush** the
  stretcher (zero-pad / drain primitive) to emit the remaining tail instead of overshooting and returning
  silence.

### Ducking

### D1 — Per-voice duck fade advanced once **per sample** (and per live input) sharing a voice — HIGH (confirmed)
- **Statement:** N concurrent samples on one ducked voice advance the shared `DuckState` N× per buffer →
  fades N× too fast and inconsistent intra-buffer gain.
- **Where:** `mix_audio` calls `engine.get_multiplier(&sample.voice_id, frames)` once per sample
  (`mixer.rs:553-556`) then again per live input (`mixer.rs:568-571`). `get_multiplier` always calls
  `advance_and_get_multiplier(frames)` (`ducking.rs:156-163`), which mutates the single `DuckState`
  (`fade_elapsed_frames` `ducking.rs:79-80`). `duck_states` is keyed only by `voice_id` (`ducking.rs:114`),
  and multiple samples per voice is supported (no dedup at `main.rs:791,796`).
- **Evidence:** an ambience voice with two overlapping loops reaches target 2× too fast; sample 2 sees a
  different multiplier than sample 1 in the same buffer.
- **Fix:** **separate read from advance.** Compute each distinct voice's multiplier **exactly once per
  buffer**: iterate distinct `voice_id`s, advance state once, cache results (small fixed-cap map / `SmallVec`
  / pre-allocated scratch — no per-call alloc), then apply the cached value to every sample/input of that
  voice. Add a `DuckingEngine::advance_buffer(distinct_voices, frames)` + a non-advancing
  `current_multiplier(voice_id)` read.

### D2 — Duck gain constant per buffer (stairstep) — MEDIUM
- **Statement:** ducking gain only changes at buffer boundaries → zipper/stair noise.
- **Where:** one scalar per buffer (`mixer.rs:555-561`) applied unchanged at `mixer.rs:657, 813, 873`;
  `advance_and_get_multiplier` advances by full `frames` and returns one value (`ducking.rs:77-93`). Contrast
  per-frame `voice_volume`/`fade_state` (`mixer.rs:617, 655, 753`).
- **Fix:** interpolate the duck multiplier **per frame** inside the mix loop (from buffer-start multiplier
  toward target across `frames`), exactly like `voice_volume` and `fade_state`. Combine with D1: compute the
  per-buffer start and end multiplier for each voice once, then lerp per frame.

### D3 — Restore hardcoded 2000 ms, ignoring the rule's `fade_duration_ms` — MEDIUM. **BEHAVIOR CHANGE**
- **Statement:** restore always uses 2000 ms regardless of the rule that ducked the voice.
- **Where:** restore branch sets `let fade_duration_ms = 2000; // Default restore time` (`ducking.rs:200`),
  despite the comment "Use the longest fade duration from previous rules" (`ducking.rs:199`); restore begins
  over that fixed duration via `begin_restore` (`ducking.rs:201-207`).
- **Evidence:** a rule with `fade_duration_ms: 200` still takes 2 s to recover — asymmetric, unconfigured.
- **Fix:** **track and reuse** the fade that produced the current duck (store it on `DuckState` when
  `begin_duck` runs; if multiple rules touched the voice, keep the max) and use it for restore. Note in
  changelog (restore timing changes for any config relying on the 2 s default).

### D4 — Live-input voices can never **trigger** ducking — MEDIUM
- **Statement:** only sample voices notify activity, so a mic voice as `primary_voice` ducks nothing.
- **Where:** the only `notify_voice_active` callers are the sample-finish RT path (`main.rs:574-575`) and the
  file-playback path (`main.rs:789`). `LiveInput` carries a `voice_id` "for ducking" (`mixer.rs:438-440`),
  but no code marks an input voice active. The ducked **side** already works for inputs (multiplier applied
  at `mixer.rs:873`).
- **Fix:** implement the **ducking-side notify path** for configured input voices so it fires the instant
  input activity is reported. Actual input-activity *detection* (ring-buffer level gate) is **Sprint 8** — do
  not build the detector here; build the wiring (mark configured input voices, route through the same off-RT
  notify path) and a test that drives the notify directly.

### D5 — Three sources of activity truth can wedge a voice ducked; `begin_duck` resets fade on every toggle — LOWER (two items)
- **Statement:** (a) `main.rs active_voices` HashSet, (b) engine `active_voices` map, (c) mixer
  `active_samples` can diverge under interleaving and wedge a voice ducked/restoring; (b) `begin_duck` resets
  `start_multiplier`/`fade_elapsed_frames` on every still-active voice when **any** voice toggles.
- **Where:** three truths at `main.rs:496 / ducking.rs:111 / active_samples`; RT overwrite
  `*active = voices_after` (`main.rs:581`); `update_duck_states` unconditionally calls `begin_duck`
  (`ducking.rs:248`) which sets `start_multiplier = current_multiplier; fade_elapsed_frames = 0`
  (`ducking.rs:59-64`). `params_changed` is already computed (`ducking.rs:228-233`) but only used for logging.
- **Fix:** **single source of truth** — derive activity from the mixer's `active_samples` set plus input
  gates on **one** thread (Sprint 5 already moved bookkeeping off RT; consolidate here). **Gate `begin_duck`
  on `params_changed`** so an unrelated toggle does not re-stretch a settled/in-flight fade.

### Resampler (polish)

### R1 — Resampler default uses Linear sinc interpolation — LOW
- **Where:** `SincInterpolationType::Linear`, `f_cutoff: 0.95` (`resampler.rs:82-88`); default
  `ResamplerQuality::Fast` (`config.rs:314, 374`), `sinc_len()=64` (`config.rs:322`), oversample 64
  (`config.rs:332`).
- **Fix:** **consider** `Cubic` (table is precomputed; cost negligible) and/or **document** that Fast is
  intentionally not transparent. No required behavior change; decision item.

## Caveats (refuted / over-stated — do not chase ghosts)

- **F2 / F3 downgraded high→medium.** Both are real but only manifest past unity (F2) or when an upstream
  source actually produces NaN/Inf (F3). symphonia PCM does not normally emit NaN. Implement the guard and
  limiter, but don't treat these as guaranteed-glitch bugs in normal playback.
- **F10 framing correction.** The gap occurs on the **`None→Some` transition** (first enable, or re-enable
  after disable), **not** on every speed command while pitch correction is already on — `enable` is guarded
  by `is_none()` (`mixer.rs:287`). Test the transition, not steady-state.
- **F10 API nuance.** The audit references `Stretch::seek` as "available but unused." In the **current**
  wrapper only `PitchCorrector::reset()` is exposed (`pitch_correction.rs:67-70`); there is **no** `seek`
  wrapper. You must **add** the pre-roll wrapper over the crate primitive — don't assume one exists.
- **F11 is masked, not absent.** Integer `buffer_size*speed` (e.g. 512×{2.0,1.5,0.5}) hides it, which is why
  `test_pitch_correction_mixing`/`_slow_speed` pass. Write the failing test at a **non-integer** product.
- **D1 effect is "too-fast but smooth," not an xrun.** Correct it, but the audible severity is moderate; the
  intra-buffer inconsistency (sample 2 ≠ sample 1) is the subtler half.
- **F7/F8/R1 are LOW/optional.** Prefer documenting + a minimal guard (per-route gain, speed cap, doc note)
  over a large refactor unless DECISIONS.md changes it. These are decision items, not mandates.
- **Bass-management findings are explicitly Sprint 7**, even though the F3 NaN guard protects the bass IIR.

## Tasks (ordered, TDD — failing test first, minimum code, confirm green)

Subagents: F-group (mix-bus output stage: F1/F2/F3/F4), I-group (interpolation/loop: F9/F5/F6),
P-group (pitch: F10/F11/F12), D-group (ducking: D1/D2/D3/D4/D5) are largely independent and parallelize.
All share the Sprint 0 render harness; coordinate only on the final output-loop ordering (F1→F2→F3 run in
sequence in the same loop).

1. **Harness scene builders.** Add reusable render-harness fixtures: a ramp buffer (frame value = frame
   index, for interpolation/reverse tests), a transient-loop buffer (single impulse near the head, for
   crossfade overlap), a `DuckingEngine`-wired scene with overlapping samples on one voice, and a NaN-poisoned
   buffer. These feed every test below.

2. **F9 reverse interpolation (HIGH).** *Failing test first:* render a ramp buffer at `speed = -0.5`; assert
   each output frame equals `frame_n*(1-frac) + frame_{n+1}*frac` (direction-independent) within tolerance —
   it currently blends the wrong neighbor. Then remove the `is_reverse` neighbor special-case in
   `mix_sample_into_output` (`mixer.rs:676-691`): always use `src_frame`/`src_frame+1`, `frac =
   src_pos.fract()`, wrapping `+1→0` under loop. Confirm green; confirm forward tests unchanged.

3. **F3 NaN guard (MEDIUM).** *Failing test first:* render the NaN-poisoned buffer; assert `peak` is finite
   and the NaN frame renders as 0.0 (`is_finite` over the whole output). Then in the final loop
   (`mixer.rs:585-587`) replace with `*s = if s.is_finite() { … } else { 0.0 }`. Confirm green.

4. **F2 limiter + clip counter (MEDIUM).** *Failing test first:* sum sources to ~1.6 and assert (a) `peak ≤
   ceiling` and (b) no hard flat-top — `max_inter_sample_delta` and the waveform shape differ from a brickwall
   clamp (e.g. a tanh-shaped curve), and (c) the clip/over `AtomicU64` increments. Then add a configurable
   `output_ceiling` + optional `master_gain` (config + validation), implement the soft-knee/tanh limiter as
   the final stage (after F1, incorporating the F3 finite-guard), bump an `AtomicU64` clip counter, and
   surface it in `handle_status` (`src/http/handlers.rs:569`) as a single field. Replace/repurpose
   `test_saturation` (`mixer.rs:989-1008`) to assert limiter behavior, **not** the old `== 1.0` brickwall
   (note this is an intended behavior change in the test, justified by the fix — do not just delete it).

5. **F1 per-channel calibration (HIGH).** *Failing test first:* configure `channel_volumes` (e.g. ch0=0.5,
   ch1=1.0), render a full-scale signal, assert per-channel `rms`/`peak` reflect the gains. Then resolve
   aliases→indices once into a per-channel gain `Vec<f32>` at `MixerState` construction (off RT), store it on
   `MixerState`, and apply it in the final loop **before** the F2 limiter. Order in the final loop:
   per-channel gain → limiter/soft-clip → finite-guard → ceiling clamp.

6. **F4 Play volume clamp (MEDIUM, behavior change).** *Failing test first:* drive a Play with `volume = 5.0`
   through `handle_command` (Sprint 0) and assert the resulting `ActiveSample.volume == 1.0`. Then clamp
   `self.volume = volume.clamp(0.0, 1.0)` inside the `ActiveSample::new*` constructors (`mixer.rs:155-243`).
   Confirm runtime `Volume` (`main.rs:1150`) and Play now agree. Changelog note.

7. **F5 equal-power crossfade/fades (MEDIUM).** *Failing test first:* render a looped uncorrelated buffer
   across the crossfade; assert `rms` through the crossfade stays within ~±0.5 dB of steady-state (linear
   currently dips ~3 dB). Then make the loop crossfade equal-power (`cos`/`sin`) at `mixer.rs:725, 739`;
   evaluate equal-power fade in/out in `FadeState::multiplier` (`mixer.rs:42-58`) — if you keep linear there,
   document why. Confirm no new inter-sample click.

8. **F6 crossfade overlap-on-wrap (MEDIUM).** *Failing test first:* render the transient-loop buffer with a
   crossfade; assert the head impulse appears **once** per loop (not twice) and `max_inter_sample_delta` at
   the loop seam is below the click threshold. Then change the wrap so the next pass starts past the
   overlapped head (`advance_position` `mixer.rs:399-414` + crossfade region `mixer.rs:729-743`): a true
   overlap-add that does not replay `[0..cf_samples]`.

9. **D1 advance-once-per-buffer (HIGH).** *Failing test first:* two overlapping samples on one ducked voice;
   advance one buffer; assert the duck fade advanced **once** (not twice) and both samples saw the **same**
   multiplier this buffer. Then split read/advance in `DuckingEngine`: add `advance_buffer(distinct_voices,
   frames)` and a non-advancing `current_multiplier(voice_id)`; in `mix_audio` (`mixer.rs:553-577`) advance
   each distinct voice once into pre-allocated scratch, then apply the cached value to all samples/inputs.

10. **D2 per-frame duck smoothing (MEDIUM).** *Failing test first:* render a long buffer through a duck fade
    and assert `max_inter_sample_delta` shows no per-buffer stairstep (smooth monotonic gain). Then, using D1's
    per-buffer start/end multipliers, **lerp the duck gain per frame** in the mix loops (`mixer.rs:657, 813,
    873`), like `voice_volume`/`fade_state`.

11. **D3 restore honors `fade_duration_ms` (MEDIUM, behavior change).** *Failing test first:* duck with a
    200 ms rule, release, assert restore completes in ~200 ms (currently 2000 ms). Then store the duck's fade
    on `DuckState` in `begin_duck` (keep max across rules) and use it in the restore branch (`ducking.rs:199-
    208`) instead of the hardcoded 2000. Changelog note.

12. **D5 gate `begin_duck` + single truth (LOWER).** *Failing test first:* assert toggling an unrelated voice
    does not reset a settled voice's fade (its `fade_elapsed_frames`/multiplier are unchanged). Then gate the
    `begin_duck` call on the already-computed `params_changed` (`ducking.rs:228-248`). Consolidate activity to
    a single source of truth derived from `active_samples` + input gates on one thread (Sprint 5 moved this
    off RT; remove the redundant truth here).

13. **D4 live-input trigger wiring (MEDIUM).** *Failing test first:* with a ducking rule whose `primary_voice`
    is a configured input voice, drive the (off-RT) notify path with "input active = true" and assert the
    ducked voice's multiplier moves toward target. Then mark configured input voices and route them through
    the same off-RT `notify_voice_active` path. Detection is Sprint 8 — the test drives the notify directly.

14. **F10 pitch pre-roll on enable (HIGH).** *Failing test first:* play a steady tone, enable pitch
    correction mid-playback via `set_speed_with_mode`, render; assert the first block's `rms` is **not** near-
    silence (no warm-up gap) and there is no inter-sample click at the transition. Then add
    `PitchCorrector::preroll(&pre_samples)` over the crate's pre-roll primitive and call it in
    `enable_pitch_correction` (`mixer.rs:286-295`) with the samples before `sample.position` (or implement a
    short direct↔stretched crossfade). Use Sprint 5's pre-allocated scratch — no `vec!` in the callback.

15. **F11 pitch position-advance alignment (MEDIUM).** *Failing test first:* render pitch-corrected at a
    **non-integer** product (e.g. buffer 512 × speed 0.7) over several buffers; assert phase continuity
    (`max_inter_sample_delta` below threshold; no recurring discontinuity). Then advance `sample.position` by
    the **integer input frames fed**, carrying the fractional remainder in a new accumulator on `ActiveSample`,
    instead of relying on `advance_position(frames)` for the pitch path (`mixer.rs:564, 781-794, 834`).

16. **F12 pitch tail flush (LOW).** *Failing test first:* play a short buffer to EOF with pitch correction and
    assert the corrected tail energy is emitted (final `rms` over the last ~`input_latency` frames is non-zero,
    not truncated). Then flush/drain the stretcher at EOF (`mixer.rs:783-789`) instead of overshooting and
    returning silence.

17. **F7/F8/R1 decisions (LOW/optional).** Implement the locked decision (DECISIONS.md): per-route gain (F7), speed cap +
    doc (F8), `Cubic` resampler (R1). Implement only what's agreed; otherwise add documentation + a minimal
    guard and log the deferral in `docs/bugs.md`.

18. **Green gate.** Run Lane A (and Lane B for the device smoke that Sprint 5 established). Fix all warnings;
    clippy clean; `fmt --check`. Update `README.md`/`CHANGELOG.md` for behavior
    changes (F4, D3, the limiter, any speed cap), and `docs/bugs.md` for deferred optional items.

## Files to create / touch

- **Touch:** `src/audio/mixer.rs` (final output stage: F1 gain pass, F2 limiter, F3 finite-guard; F9 reverse
  neighbor; F5/F6 crossfade; F10/F11/F12 pitch path; D1/D2 duck application; `ActiveSample` F4 clamp + F11
  accumulator; `MixerState` per-channel gain vector).
- **Touch:** `src/audio/ducking.rs` (D1 read/advance split + `advance_buffer`/`current_multiplier`; D2
  per-frame support; D3 stored restore fade; D5 `params_changed` gate).
- **Touch:** `src/audio/pitch_correction.rs` (F10 `preroll`/seek wrapper; F12 flush/drain).
- **Touch:** `src/main.rs` (D4 input-voice notify wiring; D5 single source of truth — coordinate with Sprint
  5's off-RT bookkeeping; F4 already covered in constructors).
- **Touch:** `src/config.rs` (F2 `output_ceiling` + optional `master_gain` with validation; resolve
  `channel_volumes` aliases→indices helper for F1; optional F7 per-route gain, F8 speed cap, R1 quality).
- **Touch:** `src/http/handlers.rs` (expose the F2 clip/over `AtomicU64` in `handle_status` — single field;
  full enrichment is Sprint 9).
- **Touch:** render-harness fixtures (Sprint 0 module) — add ramp/transient/duck/NaN scene builders.
- **Touch:** `README.md` / `CHANGELOG.md` (behavior changes), `docs/bugs.md` (deferred LOW/optional items).

## Verification (Lane A only)

All verification is **Lane A** via the Sprint 0 render harness (`rms`, `peak`, `band_energy`,
`max_inter_sample_delta`); no behavior is Windows-specific. Every finding above has a corresponding
assertion (tasks 2–16). Lane B re-runs the host suite + the device smoke Sprint 5 established (no new
device behavior this sprint). Concretely, Lane A must show:

- **F9:** -0.5x ramp render matches `frame_n*(1-frac)+frame_{n+1}*frac`.
- **F3:** NaN-poisoned buffer → finite output, NaN frame → 0.0.
- **F2:** sum > 1.0 → `peak ≤ ceiling`, soft (non-brickwall) shape, clip counter increments and appears in
  `/status`.
- **F1:** per-channel `channel_volumes` reflected in per-channel `rms`/`peak`.
- **F4:** Play `volume=5.0` → `ActiveSample.volume == 1.0`.
- **F5:** crossfade `rms` within ~±0.5 dB of steady-state.
- **F6:** head transient appears once per loop; seam `max_inter_sample_delta` below click threshold.
- **D1:** shared-voice duck advances once/buffer; both samples see equal multiplier.
- **D2:** ducked-voice gain smooth (no stairstep) per frame.
- **D3:** 200 ms-rule restore completes in ~200 ms.
- **D4:** input-voice notify moves the ducked voice toward target.
- **D5:** unrelated toggle doesn't reset a settled voice's fade.
- **F10:** mid-playback enable has no silent gap / click at the transition.
- **F11:** non-integer pitch product stays phase-continuous over several buffers.
- **F12:** pitch-corrected tail energy emitted at EOF.

## Acceptance criteria (mirror the tracker)

- [ ] NaN/non-finite input → silence, not NaN, at the output; clip/over counter exposed `[A]`
- [ ] Reverse interpolation uses `frame_n*(1-frac)+frame_{n+1}*frac`; -0.5x ramp test passes `[A]`
- [ ] Per-channel calibration (`channel_volumes`) applied as a final output gain stage; test asserts per-channel gain `[A]`
- [ ] Play `volume` clamped to [0,1] at construction `[A]`
- [ ] Ducking advanced once per buffer, applied per-frame (smooth), restore honors `fade_duration_ms`, live-input voices can trigger ducking; ducked-voice RMS follows configured fade consistently across overlapping samples `[A]`
- [ ] Pitch correction pre-rolled on enable (no silent gap); position-advance matches frames consumed; tail flushed `[A]`
- [ ] Equal-power loop crossfade with correct overlap-on-wrap; no inter-sample click at the loop point `[A]`
- [ ] True-peak/soft-knee limiter with configurable ceiling replaces the bare hard clamp; peak ≤ ceiling `[A]`

## Behavior-change / changelog notes

These change observable behavior — document in `CHANGELOG.md`/`README.md`; **implement F4 and D3 per DECISIONS.md** (they alter how existing configs/commands behave):

- **F4 (DECISIONS.md):** Play `volume > 1.0` is now clamped to 1.0 (previously amplified). Anyone relying on
  Play to boost above unity will hear a level change. (Aligns Play with the runtime `Volume` command.)
- **D3 (DECISIONS.md):** Duck **restore** now uses the rule's `fade_duration_ms` instead of a fixed 2000 ms.
  Configs that depended on the slow 2 s release will recover faster.
- **F2:** output overload is now a soft-knee/tanh limiter at a configurable ceiling (+ optional master gain)
  rather than a brickwall clamp — quieter, less harsh near 0 dBFS; new `output_ceiling`/`master_gain` config
  keys and a `/status` clip counter.
- **F5/F6:** loop crossfades are equal-power and seamless (no midpoint dip, no double-triggered head) — looped
  ambience will sound subtly different (better).
- **F8 (if a cap is added):** max non-pitch-corrected speed may be capped below 100× — document the new limit.
- `test_saturation` is intentionally re-purposed to assert limiter behavior (not the old `== 1.0`); this is a
  fix, not a weakened test.

## Definition of Done

Lane A green · Lane B green (host suite + Sprint 5's device smoke; no new device behavior) · every finding
F1–F12, D1–D5 (+ agreed F7/F8/R1) has a harness assertion that passed · F4 and D3 implemented per DECISIONS.md ·
no locks/allocations re-introduced in the callback (Sprint 5 invariant holds — re-audit the touched paths) ·
`CHANGELOG.md`/`README.md` updated for behavior changes · deferred LOW/
optional items logged in `docs/bugs.md` · committed on a branch · `cargo build --release` warning-free, clippy
clean, `fmt --check` clean.
