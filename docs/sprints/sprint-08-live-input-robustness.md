# Sprint 8 — Live Input Robustness

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 5 |
| Effort | L |
| Lanes | A (Docker), B (native macOS real device), C (manual Windows) |
| Subagents | Optional (alloc-free capture callback / async-SRC drift loop / mute+voice-volume control-plane fixes can split cleanly) |

## Goal

Make the live-input (microphone / line-in) path **glitch-free over long sessions** and **honest about its
controls**. Today the capture path allocates on the real-time thread, has no clock-drift handling between two
independent devices, drops to hard silence on underrun, assumes f32 input, and silently no-ops three documented
controls (`voice_volume`, `input_mute` restore, input-as-ducking-trigger). This sprint lands on top of the
lock-free engine from **Sprint 5** — the capture callback must obey the same never-allocate/never-block rule as
`mix_audio`, and the control-plane fixes ride the off-RT command path Sprint 5 establishes.

## Why

Live input is a first-class feature (`docs/features/microphone-input.md`; configured via `config.inputs`,
opened at `main.rs:435-491`). For the canonical install — a USB mic captured on one device, mixed into output on
a *different* device — the two clock domains drift, the RT capture thread allocates, and the underrun handler
cuts to digital silence. The result is recurring audible dropouts on exactly the long-running daemon mode this
app is built for. Separately, three operator controls are wired but dead for input-only voices, so calibration
and mic-ducking setups behave in surprising ways. These are confirmed findings from the INPUT-CAPTURE and
FEATURE-CONFLICTS audit sections, verified against current source below.

## Scope

**In scope**
- Zero-allocation resampling capture callback (`input.rs` resampling path).
- Adaptive/async SRC steered by ring-buffer fill, applied **even when nominal rates match** (drift handling).
- Graceful underrun (short fade / hold) that keeps the voice-volume ramp time-accurate.
- Non-f32 input device support (typed stream → f32 convert), mirroring Sprint 1's output dispatch.
- Route-source-channel validation warning once the device channel count is known.
- Debug guard on the de-interleave frame-alignment assumption.
- `voice_volume` reaching input-only voices; `input_mute` restoring the prior (calibrated) volume; live-input
  voices usable as **ducking triggers** (the trigger side; pairs with Sprint 6's off-RT ducking notify path).

**Out of scope**
- The ducked-*voice* (receiver) side of ducking and the per-frame ducking smoothing — those are Sprint 6. This
  sprint only adds the **trigger** side for inputs and hands activity transitions to the same notify path
  Sprint 6 owns. If Sprint 6 is not yet `Done`, coordinate: do not duplicate the notify mechanism — extend it.
- Output-side format/rate work (Sprint 1) and the streaming-loop-length bug (Sprint 4). The symmetric f32
  assumption on output is Sprint 1's; here we fix only the **input** side.
- Any rewrite of the resampler crate usage beyond switching to its documented realtime API.

## Findings addressed

Severity tags are from the audit. Every `file:line` below was re-opened and corrected against current source.

1. **No adaptive/async SRC or buffer-level steering between independent input/output clocks** — *HIGH (confirmed)*.
   - **Evidence:** `create_input_stream` picks the path on a *static* compare `let needs_resampling =
     input_sample_rate != target_sample_rate;` (`src/audio/input.rs:169`). Equal rates → `create_passthrough_input_stream`
     (`src/audio/input.rs:217-239`) does zero rate conversion. Unequal → one fixed `let resample_ratio =
     output_rate as f64 / input_rate as f64;` (`src/audio/input.rs:261`) built into a `SincFixedIn`
     (`src/audio/input.rs:266-272`); `set_resample_ratio` is **never called** (`rg set_resample_ratio src/`
     returns nothing). The consumer (`mix_live_input_into_output`, `src/audio/mixer.rs:838-885`) pops exactly
     `frames*input_channels` per callback with no feedback to the producer rate. Two devices are clocked by
     independent oscillators (input opened as a separate cpal stream at `main.rs:444`), so even "48000 == 48000"
     drifts; the ring buffer slowly fills (producer drops samples → click, `input.rs:227-230`/`302-309`) or
     drains (mixer emits silence, `mixer.rs:858-862`).
   - **Fix:** Drive the resampler ratio from the *measured* ring-buffer fill: target ~half-full, adjust
     `resample_ratio` via `set_resample_ratio()` with a slow control loop. Apply async SRC **even in the
     same-rate case** — treat `input == output` as still drifting (passthrough is not safe for two clocks).

2. **Resampling capture callback heap-allocates on the RT thread** — *HIGH (confirmed)*.
   - **Evidence:** The closure at `src/audio/input.rs:279` is passed straight into `build_input_stream`, so it
     runs on cpal's RT capture thread. Inside it: `input_buffer[channel].push(*sample);` grows per-channel Vecs
     (`input.rs:283`); `let input_chunk: Vec<Vec<f32>> = input_buffer.iter_mut().map(|ch|
     ch.drain(..chunk_size).collect()).collect();` allocates channels+1 Vecs per chunk (`input.rs:289-291`);
     `resampler.process(&input_chunk, None)` (`input.rs:294`) is rubato's allocating convenience method (its own
     doc says use `process_into_buffer` for realtime). The `mixer.rs` header already forbids allocation in the
     callback — the capture callback violates the same rule.
   - **Fix:** Pre-allocate fixed-capacity de-interleave accumulators and reuse them (no push-growth /
     drain-collect), and call `resampler.process_into_buffer()` into a reusable buffer from
     `output_buffer_allocate()`. **Zero allocation in the callback.**

3. **Underrun fills hard silence with no smoothing and stalls the voice-volume ramp** — *MEDIUM (confirmed)*.
   - **Evidence:** `mix_live_input_into_output` reads one frame at a time; on shortfall `if samples_available <
     samples_needed { … break; }` (`src/audio/mixer.rs:858-862`) leaves the rest of the block as the pre-zeroed
     silence. No fade / hold-last-frame. The `break` also stops calling `input.advance_voice_volume()`
     (`mixer.rs:852`) for the remaining frames, so the ramp desyncs from real time by up to one block.
   - **Fix:** On underrun apply a brief fade-to-silence (or hold/repeat the last frame for a few samples) instead
     of a hard cut, and keep advancing `advance_voice_volume()` for the silent frames so the ramp stays
     time-accurate. Combined with finding 1, underruns should become rare.

4. **Input stream format assumed f32; non-f32 device fails to open and is silently dropped** — *MEDIUM (confirmed)*.
   - **Evidence:** Both stream builders use an f32 callback (`move |data: &[f32], …|`, `src/audio/input.rs:224`
     and `:279`) on `default_input_config` (`input.rs:159`/`get_input_config` `:101-104`) without checking
     `supported_config.sample_format()`. cpal does not transparently convert formats; an I16/U16-native device
     returns an error surfaced as `InputError::StreamError` (`input.rs:236`/`:322`). At `main.rs:483-489` that
     error is only logged and the loop continues, so the input never appears in the mix.
   - **Fix:** Inspect `supported_config.sample_format()` and build a typed input stream that converts to f32
     before pushing to the ring buffer (mirror Sprint 1's output dispatch over I16/U16/I32/F32). At minimum,
     surface a clear user-visible error.

5. **Route `source_channel` not validated vs device channel count; out-of-range routes silently dropped** —
   *LOW (confirmed)*.
   - **Evidence:** Input route validation only checks alias resolution (`resolve_channel` calls at
     `src/config.rs:747-752`, inside the route loop `:745-753`); it never compares the resolved source index to
     the device's channel count (unknown until the stream opens). The mixer guard silently skips:
     `if src_ch >= input_channels || dest_ch >= output_channels || src_ch >= 16 { continue; }`
     (`src/audio/mixer.rs:877`).
   - **Fix:** After the stream opens and `active_input.channels` is known (`main.rs:444-474`), warn if any
     route's source channel `>= active_input.channels` so the misconfiguration is visible.

6. **De-interleave assumes `data.len() % channels == 0`** — *LOW (confirmed, latent)*.
   - **Evidence:** `let channel = i % channels; input_buffer[channel].push(*sample);` (`src/audio/input.rs:282-283`)
     then `ch.drain(..chunk_size)` guarded only by `while input_buffer[0].len() >= chunk_size` (`input.rs:287`).
     A non-frame-aligned buffer would give channels unequal lengths and `drain` on a shorter channel could panic.
     cpal delivers whole frames, so this cannot trigger in practice — flagged as fragility only.
   - **Fix:** `debug_assert!(data.len() % channels == 0)` (and/or guard the drain on each channel's length),
     documenting the frame-alignment assumption.

7. **`voice_volume` on an input-only voice does nothing** — *MEDIUM feature-conflict (confirmed)*.
   - **Evidence:** Live inputs are pushed directly into `mixer_state.live_inputs` (`main.rs:468-477`) and are
     **never** registered with the `VoiceManager` (only Play registers voices). In the VoiceVolume handler,
     `let success = voice_mgr.set_voice_volume(&voice, new_volume);` (`main.rs:881`) returns `false` when the
     voice id has no entry (`set_voice_volume` returns `false` on miss, `src/voice.rs:132-139`). The entire
     ramp-update block — including the `for input in state.live_inputs` loop that calls
     `input.set_target_voice_volume` (`main.rs:899-905`) — runs only inside `if success` (`main.rs:885`). So
     unless a *sample* has already created that voice id, an input's voice_volume logs "Voice not found"
     (`main.rs:913`) and never changes.
   - **Fix:** Make VoiceVolume update matching `live_inputs` even when the `VoiceManager` has no sample-backed
     voice (treat a matching input as success), **or** register configured input `voice_id`s in the
     `VoiceManager` at startup. Pick one; the second keeps the `if success` shape intact and is the smaller
     change to the handler.

8. **`input_mute` unmute resets volume to hardcoded 1.0** — *MEDIUM feature-conflict (confirmed)*.
   - **Evidence:** `state.live_inputs[idx].volume = if mute { 0.0 } else { 1.0 };` (`main.rs:1011` by index,
     `:1024` by voice_id). The comment at `main.rs:1009-1010` even acknowledges a real version would store the
     previous volume. `InputConfig.volume` is configurable and `input_volume` sets an arbitrary calibrated value
     (`main.rs:973`, `:986`). Unmute discards it: calibrated 0.7 → mute → unmute returns 1.0 (~3 dB hot).
   - **Fix:** Store the pre-mute volume (or a separate `muted` bool on `LiveInput`) and restore the actual prior
     value on unmute. `LiveInput` (`src/audio/mixer.rs:438-459`) currently has no such field — add one.

9. **Live-input voices can't trigger ducking (the trigger side)** — *MEDIUM feature-conflict (confirmed)*.
   - **Evidence:** The mixer asks the ducking engine for a multiplier by `input.voice_id`
     (`src/audio/mixer.rs:838-885`, applied at `:873`), so inputs **can be ducked**. But `notify_voice_active`
     is only ever called for sample voices (sample-finish and Play paths). Inputs continuously produce audio yet
     never notify the engine they are active, so a rule with a mic as `primary_voice`
     ("duck music while the gamemaster mic is live") never fires.
   - **Fix:** Mark each configured input's `voice_id` as active in the ducking engine — either always-active at
     startup, or gated on input signal presence — through the **same off-RT notify path Sprint 6 owns**. Do not
     notify from the capture callback (that is the Sprint 5 rule). This is the trigger side only.

## Caveats (don't chase ghosts)

- **Finding 1 is real but not constant.** Oscillator drift is tens of ppm; with the 4× ring-buffer headroom
  (~80 ms at the 20 ms default, `input.rs:108-112`) dropouts recur on the order of minutes-to-hours, not every
  second. The audit kept it HIGH because eventual audible glitching is effectively guaranteed for a long-running
  daemon — which is the normal mode. Lane A proves the *mechanism* (bounded ring buffer under simulated drift);
  the real-world period is what Lane C's ≥30-min soak confirms.
- **Finding 2 is conditional.** It only fires when `input_rate != output_rate` (the resampling path). The
  passthrough path doesn't allocate today — but once finding 1 routes *every* input through async SRC, the
  alloc-free requirement applies to the (now always-on) resampling path. Build the harness around the resampling
  callback specifically.
- **Finding 6 cannot trigger in practice** (cpal delivers whole frames). It's a one-line `debug_assert!` for
  defense, not a bug hunt — keep the change minimal.
- **The f32 assumption is symmetric, not unique to input** (output makes it too at `main.rs` / `engine.rs`). That
  output side is **Sprint 1's** job; here fix only the input side (finding 4).
- **Finding 9 is the trigger half only.** The receiver/smoothing half of ducking is Sprint 6. Don't reimplement
  the notify channel — extend Sprint 6's.

## Tasks (ordered, TDD)

Use the Sprint 0 render harness (`rms` / `peak` / `band_energy` / `max_inter_sample_delta`) and the Sprint 5
allocation-counting harness wherever audio output is involved. Write the failing test first, confirm it fails,
write the minimum code, confirm green.

1. **Alloc-free resampling capture callback** (finding 2).
   - **Failing test first:** wrap the resampling-stream callback body (extract it into a testable pure fn, e.g.
     `fn resample_block(state: &mut ResampleState, data: &[f32], producer: &mut HeapProducer<f32>)`) and assert
     **zero allocations** across many invocations using the Sprint 5 allocation-counting harness. It must fail
     against the current `Vec::push` / `drain(..).collect()` / `resampler.process()` code.
   - **Then:** give `ResampleState` pre-sized de-interleave accumulators and a reusable output buffer from
     `resampler.output_buffer_allocate()`; switch `process` → `process_into_buffer`. Re-run: zero alloc.
   - Assert via the render harness that resampled output through the consumer is byte-for-byte (or within float
     tolerance) identical to the pre-change `process()` path for a fixed input block (no behavior change to the
     audio, only to allocation).

2. **Adaptive/async SRC steered by ring-buffer fill** (finding 1).
   - **Failing test first:** a **drift simulation** — a producer feeding the resampler at rate `r_in` and a
     consumer draining at a deliberately mismatched `r_out` (e.g. ±50 ppm and a gross ±1% case) over a simulated
     long run. Assert the ring-buffer fill stays **bounded** within a window around half-full and never hits 0
     (underrun) or capacity (overflow/drop). With today's fixed `resample_ratio` this must fail (monotonic
     drift to a boundary).
   - **Then:** add a slow control loop that reads `consumer.len()` (exposed to the producer side via a shared
     atomic fill counter, since producer/consumer are split) and nudges `resampler.set_resample_ratio(ratio *
     (1 + k*err))` toward target ~half-full. Route the previously-passthrough equal-rate case through the same
     async SRC (`needs_resampling` becomes "always resample for drift control"). Re-run: bounded.
   - Keep the steering gentle (small `k`, clamp ratio deviation well inside the `SincFixedIn` max-relative-ratio
     of 2.0 at `input.rs:266-272`) so no audible pitch wobble — assert `band_energy` of a steady sine through
     the SRC stays within tolerance of the input's pitch.

3. **Graceful underrun + time-accurate ramp** (finding 3).
   - **Failing test first:** drive `mix_live_input_into_output` with a consumer that underruns partway through a
     block while a voice-volume ramp is in progress. Assert (a) **no hard discontinuity** —
     `max_inter_sample_delta` at the underrun boundary stays below the click threshold; (b) the voice-volume
     ramp value after the block equals the *full-block* expected value (ramp advanced for silent frames). Both
     fail today (hard `break` cuts to silence and stops the ramp).
   - **Then:** on shortfall, fade-to-silence over a few samples (or hold/repeat the last frame) and continue
     calling `advance_voice_volume()` for the remaining frames instead of `break`.

4. **`voice_volume` reaches input-only voices** (finding 7).
   - **Failing test first:** in the `handle_command` test harness (Sprint 0), with a `live_inputs` entry whose
     `voice_id` has **no** sample playing, send `VoiceVolume{voice, volume}` and assert the input's
     `target_voice_volume` updated. Fails today (gated behind `if success`).
   - **Then:** either register configured input voice_ids in `VoiceManager` at startup, or make the handler
     update matching `live_inputs` regardless of `success`. Add a test that a *sample-backed* voice still works.

5. **`input_mute` restores the prior calibrated volume** (finding 8).
   - **Failing test first:** set input volume to 0.7 (`InputVolume`), `InputMute{mute:true}` → volume 0.0,
     `InputMute{mute:false}` → assert volume is **0.7**, not 1.0. Fails today.
   - **Then:** add a `pre_mute_volume: f32` (or `muted: bool`) field to `LiveInput`
     (`src/audio/mixer.rs:438-459`); on mute, save current volume and zero it; on unmute, restore the saved
     value. Apply to both the index and voice_id branches (`main.rs:1011`, `:1024`).

6. **Live-input voices as ducking triggers** (finding 9).
   - **Failing test first:** a ducking rule with a mic `voice_id` as `primary_voice`; assert the engine marks
     that voice active (and thus would duck a target) when the input is configured/flowing. Fails today (no
     notify for inputs).
   - **Then:** at startup (or on first signal presence), notify the ducking engine that each configured input
     voice is active **via the Sprint 6 off-RT notify path** — never from the capture callback. If Sprint 6's
     path doesn't exist yet, coordinate before building a parallel one.

7. **Non-f32 input device support** (finding 4).
   - **Failing test first (pure helper):** a `build_input_stream_for_format(sample_format, …)` dispatcher unit
     test over `I16/U16/I32/F32` asserting it selects the right typed builder and a forced-I16 path produces
     f32 samples in the ring buffer without error. (Real-device open is Lane B.)
   - **Then:** inspect `supported_config.sample_format()` in `create_input_stream` (`input.rs:153-214`) and build
     a typed stream converting to f32 before `producer.push_slice`, mirroring Sprint 1's output dispatch. Keep
     the resampling vs passthrough/async-SRC choice orthogonal to the format choice.

8. **Route-channel validation warning** (finding 5).
   - **Test:** after a stream opens with a known channel count, a route whose source `>= channels` emits a
     warning (capture the log and assert its content — test output must stay pristine). Add the warn at
     `main.rs:444-474` once `active_input.channels` is known.

9. **De-interleave frame-alignment guard** (finding 6).
   - Add `debug_assert!(data.len() % channels == 0)` at the top of the de-interleave loop (`input.rs:281`).
     No behavior change in release; documents the assumption.

## Files to create / touch

- **Touch `src/audio/input.rs`:** extract the resampling callback into a testable, alloc-free `ResampleState` +
  `resample_block` (task 1); add the fill-steered `set_resample_ratio` control loop and route equal-rate through
  it (task 2); add `sample_format()` dispatch + typed→f32 conversion (task 7); `debug_assert!` frame alignment
  (task 9).
- **Touch `src/audio/mixer.rs`:** graceful underrun in `mix_live_input_into_output` (`838-885`, task 3); add
  `pre_mute_volume`/`muted` to `LiveInput` (`438-459`, task 5).
- **Touch `src/main.rs`:** un-gate / register input voices for `voice_volume` (task 4); mute save/restore at
  `1011`/`1024` (task 5); input-as-ducking-trigger notify at input setup (`444-474`, task 6); route-channel
  validation warning (task 8).
- **Touch `src/voice.rs`** *(only if task 4 chooses the "register input voices" option)*: a way to register an
  input voice id at startup.
- **Touch `src/config.rs`** *(optional, only if route validation can be partly tightened pre-open)*: keep
  `745-753` as alias-only; the device-count check stays runtime (task 8).
- **Tests:** new unit/integration tests under `tests/` or `#[cfg(test)]` per task; reuse the Sprint 0 render
  harness and Sprint 5 allocation harness — do not add new heavy deps.
- **Refine `docs/sprints/MANUAL-VERIFICATION.md`:** the **W-2** entry already exists; refine it (see below).
- **Update `docs/features/microphone-input.md`** for the behavior changes (mute restore, voice_volume on inputs,
  mic-as-ducking-trigger, drift handling).

## Verification

- **Lane A (Docker / Linux):**
  - Allocation harness proves **zero allocation** in the resampling capture callback (task 1).
  - Drift simulation keeps the ring buffer **bounded** (never under/overflows) under mismatched producer/consumer
    rates with the steered SRC (task 2).
  - Underrun render test: no inter-sample click above threshold and the ramp stays time-accurate (task 3).
  - `handle_command` tests: `voice_volume` changes an input-only voice (task 4); `input_mute` round-trips a
    calibrated 0.7 (task 5); input voice marks the ducking engine active (task 6).
  - Format dispatcher unit test incl. forced-I16 → f32 (task 7); route-out-of-range warning captured (task 8).
  - `scripts/validate.sh` green: build `-D warnings`, clippy `-D warnings`, `fmt --check`, full test suite.
- **Lane B (native macOS, real device):** `scripts/validate.sh --native` green, **including** a smoke test that
  a CoreAudio **input** device opens and routes into the mix (the device-test gate from Sprint 0).
- **Lane C (manual Windows / partner):** refine the existing **W-2** entry in `MANUAL-VERIFICATION.md`: ≥30-min
  two-device soak (USB mic + a *separate* output interface) showing **no periodic dropouts** and a **bounded**
  ring-buffer fill (no monotonic drift), **plus** confirm a **non-f32-native** input device opens and routes.

## Acceptance criteria (from the tracker — verbatim)

- [ ] No allocation in the capture callback (allocation harness); `process_into_buffer` + reusable buffers `[A]`
- [ ] Adaptive/async SRC steered by ring-buffer fill (even at equal nominal rates); drift simulation keeps the ring buffer bounded `[A]`
- [ ] Underrun applies a short fade/hold (no hard cut) and keeps the ramp time-accurate `[A]`
- [ ] `voice_volume` reaches input-only voices; `input_mute` restores the prior volume (not hardcoded 1.0); route source channels validated vs device channels `[A]`
- [ ] Non-f32 input format supported (convert to f32) `[A]`
- [ ] CoreAudio input device opens and routes into the mix (smoke) `[B]`
- [ ] ≥30-min two-device (USB mic + separate output) soak shows no periodic dropouts; non-f32 input opens `[C]`

## Behavior-change / changelog notes

User-visible behavior changes — record in `CHANGELOG.md`/`README.md`:

- **`input_mute` now restores the prior volume on unmute** instead of forcing 1.0. Anyone relying on unmute
  bumping a calibrated input to full level will see a difference. **Decided upfront** in DECISIONS.md (it changes a
  documented command's effect).
- **`voice_volume` now affects input-only voices.** Previously a no-op unless a sample shared the voice id;
  scripts that sent `voice_volume` to a mic voice expecting nothing will now change input level.
- **Live-input voices can now trigger ducking** as a `primary_voice`. A config that listed a mic as a ducking
  primary was silently inert and will now actually duck. **Decided upfront** in DECISIONS.md (changes audible mix
  for existing configs).
- **Equal-rate inputs now go through async SRC** rather than raw passthrough (drift control). Inaudible in
  steady state, but it is a path change worth noting; if any pitch wobble is observed, the steering gain is too
  high.
- **Out-of-range input routes now log a warning** (previously silent). New log line, no audio change.

## Definition of Done

Lane A green · Lane B green (incl. real CoreAudio **input** open + route smoke) · allocation harness shows zero
alloc in the resampling capture callback · drift simulation keeps the ring buffer bounded · underrun fade/hold
+ time-accurate ramp landed · `voice_volume`/`input_mute`/route-validation/input-ducking-trigger fixes landed
with tests · non-f32 input dispatch landed · **W-2** refined in `MANUAL-VERIFICATION.md` · behavior changes in
`CHANGELOG.md`/`README.md` per DECISIONS.md on the mute-restore, input-voice_volume, and input-ducking
changes · out-of-scope discoveries logged to `docs/bugs.md` · committed on a
branch · `cargo build --release` warning-free.
