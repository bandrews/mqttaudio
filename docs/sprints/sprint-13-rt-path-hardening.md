# Sprint 13 — RT-Path Hardening

| Field | Value |
|-------|-------|
| Status | Done (Lane A via documented host approximation — see tracker note; Lane B pending partner) |
| Depends on | 11 (uses its `/metrics` surface); soft-ordered after 12 (owner priority) |
| Effort | L |
| Lanes | A (Docker) + B (native macOS) |
| Subagents | Optional (F1/F2, F3, F4/F5 are independent groups) |

## Goal

Close every known remaining allocation, free, and logging path on the real-time threads — the
documented Sprint 5/6/8 residuals: the first-duck HashMap insert, the >256 voice-pool realloc
(implement the locked D18 steal/reject policy), the pitch-corrector construct/drop on toggle,
the `tracing` calls reachable from the audio and capture callbacks, and the mid-run scratch
regrowth. Every fix lands alloc-harness-test-first, and the harness's deliberate blind spots
(warmed duck voices, untoggled pitch) are removed along the way.

## Why

The RT engine's contract (D22a) is no allocation, no free, no blocking on the callback. The
residuals here are each individually small and bounded — which is why they were deferred — but
they are the difference between "alloc-free in the tested cases" and "alloc-free, period". On
Pi-class hardware a single unlucky page-faulting malloc inside the callback is an audible
dropout. All five are pre-documented in `docs/bugs.md` with agreed fix shapes; this sprint
executes them.

## Scope

**In scope**
- F1 duck-state pre-population; F2 D18 over-cap policy; F3 control-side pitch-corrector
  lifecycle; F4 RT logging → control-side validation + atomic counters; F5 scratch pre-sizing;
  F6 the control-side pitch-on-streaming warning.

**Out of scope** (owned by other sprints / accepted)
- The C++ (signalsmith) side of pitch allocation — the Rust harness cannot see it
  (`docs/bugs.md`, accepted; verified by soak, not the harness).
- The latency metric publication (Sprint 11, already landed by now).
- Per-streamed-source ring gauges (deferred LOW).
- `input.rs:565` — the cpal *error* callback's `tracing::error!` stays (error callbacks are not
  the steady-state RT path; this mirrors the xruns counter's accepted pattern).

## Findings addressed

All citations re-verified against the current tree during Sprint 10.

### F1 — First duck of a voice allocates on the audio thread
- **Statement:** `DuckingApplier::apply_target` inserts into `duck_states` with an owned key
  clone on first duck of each voice; the alloc-harness ducking tests warm the voice first, so
  the gap is deliberately untested.
- **Verified at:** `src/audio/ducking.rs:493-503`
  (`duck_states.entry(change.voice.clone()).or_insert_with(DuckState::new)`); the residual and
  the fix shape are recorded in `docs/bugs.md` ("Ducking `apply_target` clones the voice
  string…").
- **Severity:** low (one small alloc per newly-ducked voice) but contract-breaking.
- **Fix:** pre-populate `duck_states` at applier construction (off-RT) with every voice that can
  be ducked — the union of configured ducking-rule voices (`config.ducking_rules` targets and
  any voice nameable as a duck target). Where arbitrary runtime voices can be ducked, pre-insert
  at the **control side**: `SetDuckTarget` is built control-side, so ship the owned key with the
  command and make the RT-side insert allocation-free (`HashMap::insert` of a moved key still
  allocates on growth — so also `reserve` the map to a documented cap at construction, mirroring
  `MAX_VOICES`). TDD: **remove the warm-up crutch** — a new alloc-harness case asserts the
  *first* duck of a never-seen voice is 0 alloc / 0 free, failing before the fix.

### F2 — Voice pool over-cap reallocates on the RT thread; D18 policy unimplemented
- **Statement:** `active_samples` is reserved to `MAX_VOICES` (256) but a Play past the cap
  reallocates the Vec inside `drain_commands`; the locked D18 policy (steal the oldest
  non-looping voice, else reject, displaced/rejected sample to the graveyard) was deferred.
- **Verified at:** `src/audio/mixer.rs:916-919` (`MAX_VOICES`), `:1085`
  (`Vec::with_capacity(MAX_VOICES)`); `src/rt_engine.rs:270` (`AddSample` arm pushes
  unconditionally); residual recorded in `docs/bugs.md` ("Voice pool is a soft reserve…").
- **Severity:** low likelihood (>256 simultaneous voices), contract-breaking when hit.
- **Evidence:** D17/D18 in `DECISIONS.md` locked the policy in Sprint 5; the graveyard producer
  is already threaded into the callback state for finished samples, so the plumbing exists.
- **Fix:** in the `AddSample` arm: if `active_samples.len() >= MAX_VOICES`, find the oldest
  (lowest `sample_id` or earliest-added index) sample with `loop_mode == false`, `swap_remove`
  it to the graveyard, push the new one; if every slot is looping, send the **new** sample to
  the graveyard (reject) — both branches alloc-free. Apply the same shape to
  `live_inputs`/`streamed_sources` caps only if their arms share the unconditional push
  (verify; do not invent work). Owner re-approved the tiny behavior change 2026-06-09. TDD:
  alloc-harness case driving the pool past 256 asserting 0 alloc / 0 free and the
  steal-oldest-non-looping outcome; extend `tests/soak_test.rs` past the cap; a unit test for
  the all-looping reject branch.

### F3 — Toggling pitch correction constructs/drops the stretcher on the RT thread
- **Statement:** `SetSpeedMatching { pitch_correction }` applied on the callback calls
  `enable_pitch_correction` (constructs `PitchCorrector`, sizes the tail buffer) or drops it —
  heap traffic on the RT thread whenever the command flips the mode on a live voice. The alloc
  harness deliberately does not toggle pitch.
- **Verified at:** `src/rt_engine.rs:230-237` (apply on RT) → `src/audio/mixer.rs:470-478`
  (`set_speed_with_mode`) → `:406-423` (`enable_pitch_correction`: `PitchCorrector::new`,
  `pitch_tail_buffer.resize`); residual + fix shape in `docs/bugs.md` ("A Speed command that
  TOGGLES pitch correction…").
- **Severity:** medium (real allocation on a documented command path).
- **Fix (D56):** the control thread pre-builds the corrector and ships it in the command:
  `SetSpeedMatching` gains `corrector: Option<Box<PitchCorrector>>` (built with the *target
  voice's* channels/sample-rate — the control side knows them from the play bookkeeping; where
  a selector matches multiple voices with differing channel counts, fall back per-voice:
  document the chosen resolution in the code and DECISIONS.md addendum if it deviates).
  RT side: on enable, `take()` the shipped corrector (move, no construction), run the existing
  pre-roll; on disable or replacement, move the old corrector into the **command-return ring**
  (the existing husk path) for off-RT drop — never `drop` it on the callback. The tail buffer
  must be pre-sized control-side too (it belongs to the shipped corrector bundle or to a
  pre-sized field per F5). TDD: alloc-harness case toggling pitch on a live voice mid-playback
  asserting 0 Rust-side alloc / 0 free (C++ caveat stays documented); render-harness asserting
  the toggle still produces the F10-S6 no-gap crossfade behavior.

### F4 — `tracing` calls reachable from RT threads
- **Statement:** The audio callback can emit `tracing::warn!` (negative speed with pitch
  correction, via `apply_mutation` → `set_speed`), and the input capture callback can emit
  `tracing::error!`/`debug!` (resampler error, overflow drop, ratio reject). Tracing is not
  guaranteed non-blocking/non-allocating under a slow or contended subscriber.
- **Verified at:** `src/audio/mixer.rs:374` (in `set_speed`, RT via `src/rt_engine.rs:123`
  `apply_mutation`); `src/audio/input.rs:424` (`Resampling error`), `:441` (`Resampler output
  overflow`), `:479` (`set_resample_ratio rejected`). (`input.rs:176`/`:185` are setup-time —
  off-RT, leave them. `input.rs:565` is the error callback — accepted, leave it.)
- **Severity:** low-medium (quiet in steady state; unbounded worst case).
- **Fix (D57):**
  - `mixer.rs:374`: validate **control-side instead** — the dispatcher knows
    speed + pitch_correction before sending `SetSpeedMatching`; warn there and do not send an
    invalid combination. The RT-side branch keeps its silent `return false` (no log). This is
    strictly better feedback (the operator sees the warn even if no voice matched).
  - `input.rs` capture-path sites: replace each with a relaxed `AtomicU64` counter
    (`resample_errors`, `overflow_dropped_samples`, `ratio_rejects`) on the input's shared
    state; the control plane (the existing reaper/status tick) reads-and-logs deltas off-RT and
    `/metrics` surfaces them (the Sprint 11 pattern). TDD: capture-path unit tests assert the
    counters increment (drive the error by feeding an undersized/oversized block); a log test
    asserts the off-RT drain logs once per delta, keeping test output pristine; alloc-harness
    case for the error path (it must be alloc-free even when erroring).

### F5 — Scratch buffers can grow on the RT thread mid-run
- **Statement:** The pitch scratch and the input conversion scratch grow to the largest block
  seen; a device that enlarges its block mid-run triggers a reallocation inside the callback.
- **Verified at:** `src/audio/mixer.rs:1430-1432` (`stretched.resize(output_samples, 0.0)` on
  the taken `pitch_scratch`); `src/audio/input.rs:228` (`scratch.resize(data.len(), 0.0)` in
  `convert_input_block`), `:554` (capture-callback scratch).
- **Severity:** low (steady-state safe; pathological devices only).
- **Fix (D58):** pre-size at construction to the maximum block the stream can deliver: when
  `audio.buffer_size` is fixed, that value; otherwise the device-reported
  `SupportedBufferSize::Range.max` clamped to a documented sane cap (e.g. 8192 frames), falling
  back to the cap when the backend reports `Unknown`. Keep the `resize` as a guarded fallback
  **plus a relaxed `scratch_regrow` counter** (F4's mechanism) so a pathological device is
  visible on `/metrics` instead of silent. TDD: unit test constructing with a max-block hint and
  asserting capacity; alloc-harness case feeding an oversized block asserting the counter ticks
  (and, with the pre-size, no realloc for in-range blocks).

### F6 — Pitch correction on a streaming buffer is a silent no-op
- **Statement:** Enabling pitch correction for a voice whose buffer is still `Streaming` does
  nothing, with no operator feedback (Sprint 12 keeps full decode for *plays* that request
  pitch, so the remaining case is a later `speed`/pitch command targeting a still-loading cold
  play or an uncached-HTTP full-load).
- **Verified at:** `src/audio/mixer.rs:1243-1246`, `:1408-1416` (Complete-only), `:445`
  (pre-roll note); no dispatch-side warning exists (contrast
  `selector_targets_streamed_voice` in `src/main.rs` for windowed voices).
- **Severity:** low (UX/feedback).
- **Fix:** at command dispatch, when a Speed command with `pitch_correction: true` targets a
  voice the control plane knows is still streaming (it tracked `is_streaming` at play time;
  completion is observable via the buffer Arc it can re-check cheaply with
  `SampleBuffer::is_complete`), emit a `tracing::warn!` naming the voice and the limitation.
  Mirror the `selector_targets_streamed_voice` best-effort pattern — voice-keyed, no RT
  involvement. TDD: log-capture test (pristine-output style) asserting the warn fires for a
  streaming target and not for a complete one.

## Implementation deviations (recorded honestly)

- **F3 ships per-sample, not per-selector.** One `SetSpeedMatching` can match several samples but one
  corrector serves one sample, so the dispatcher expands the selector control-side (it already mirrors
  every play in `ctx.playing`, including the channel count added to `SampleStatus` for sizing) into one
  `SetSpeedWithCorrector { id, bundle, displaced }` per match. The `PitchBundle` box is never freed on the
  RT thread: the corrector is taken out of it, the sample's old vecs are swapped into it, and the box rides
  the spent husk back. The selector-based `SetSpeedMatching` remains lib/test-only (`#[allow(dead_code)]`
  with justification); the binary never sends it.
- **F5's pitch half merged into F3's bundle** (the shipped scratch is pre-sized to the negotiated output
  block, `max_block_frames` threaded through `CommandCtx`); the input half pre-sizes the conversion scratch
  from the device's advertised maximum block. Both keep counted fallbacks (`scratch_regrows` per input,
  `pitch_scratch_regrows` global) on `/metrics`.
- **F4's RT alloc-on-error caveat:** the converted capture sites count via relaxed atomics; the
  deterministically drivable counter (ring overflow) is unit-tested
  (`input_resample_test::ring_overflow_bumps_the_telemetry_counter_instead_of_logging`); the resample-error
  and ratio-reject counters are exercised by the same code shape but cannot be deterministically triggered
  through the public API — verified by code review.
- **F6's warning keys off the D51 upgrade registry** (`ctx.streaming_upgrades`), which is exactly the set of
  still-loading cold plays, rather than re-deriving streaming-ness from the buffer.

## Caveats (refuted / over-stated — do not chase ghosts)

- **Do not try to prove the C++ stretcher alloc-free with the Rust harness** — it cannot see
  FFI allocations (`docs/bugs.md`). F3's acceptance is Rust-side harness + render parity +
  soak; the C++ caveat remains documented.
- **`input.rs:565` stays.** Error callbacks are the sanctioned place for `tracing` on stream
  failure (the xruns pattern); converting it would lose information for no RT win.
- **Do not redesign ducking.** F1 is a pre-population/reserve fix; `advance_buffer`'s
  iterate-the-map design (D1, documented in `docs/bugs.md` implementation notes) is correct and
  allocation-free — leave it.
- **`Instant::now()` and relaxed atomics are RT-safe** — established in Sprint 11; reuse, don't
  re-derive.

## Tasks (ordered, TDD-first)

1. F4 counters + control-side speed validation (establishes the counter plumbing F5 reuses).
2. F1 duck pre-population/reserve (remove the warm-up crutch in the same change).
3. F2 D18 over-cap policy + soak extension.
4. F5 scratch pre-sizing + regrow counter.
5. F3 control-side pitch-corrector lifecycle (largest; lands last on purpose).
6. F6 dispatch warning.
7. Update `docs/bugs.md`: mark the duck/voice-pool/pitch-toggle residual entries resolved with
   citations; record anything new discovered. Update `docs/http-api.md` for the new `/metrics`
   counters.

## Files to create / touch

- **Touch:** `src/audio/ducking.rs` (F1), `src/rt_engine.rs` (F2 AddSample arm, F3 command
  variant + return-ring path), `src/audio/mixer.rs` (F3 enable-from-shipped-corrector, F5
  pre-size, `:374` silent-return), `src/audio/input.rs` (F4 counters, F5 pre-size),
  `src/main.rs` (F3 control-side build, F4 validation warn + counter drain, F6 warning),
  `src/http/handlers.rs` (`/metrics` counters), `tests/alloc_harness.rs` (new cases F1/F2/F3/F4/
  F5), `tests/soak_test.rs` (F2), `tests/input_resample_test.rs` (F4), render-harness tests
  (F3 parity), `docs/bugs.md`, `docs/http-api.md`, `CHANGELOG.md`.

## Verification

### Lane A (Docker / Linux)
- `scripts/validate.sh` green; the **full** alloc harness including the new first-duck,
  over-cap, pitch-toggle, error-path, and oversized-block cases; render-harness parity
  unchanged; soak past the voice cap clean.

### Lane B (native macOS real device)
- `scripts/validate.sh --native` green; real-device smoke with a pitch toggle mid-play and a
  burst of >MAX_VOICES plays (scripted) with `xruns == 0`.

### Lane C (manual Windows)
- None new (the existing listening-soak item in `MANUAL-VERIFICATION.md` now also covers the
  pitch-toggle path — append a one-line note).

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 13)

- [ ] First duck of a never-seen voice is 0 alloc / 0 free on the callback (warm-up crutch removed from the harness) `[A]`
- [ ] D18 over-cap policy implemented: steal oldest non-looping else reject, displaced sample via graveyard, alloc-free past 256; soak past the cap green `[A]`
- [ ] Pitch-corrector lifecycle is control-side: toggle mid-play is Rust-side alloc/free-free; displaced corrector dropped off-RT; no-gap crossfade parity kept `[A]`
- [ ] No `tracing` call sites remain on the audio or capture steady-state paths (`mixer.rs` set_speed warn moved control-side; `input.rs` capture sites are relaxed counters surfaced on `/metrics` and drained off-RT) `[A]`
- [ ] Scratch buffers pre-sized to the stream's max block with a counted regrow fallback `[A]`
- [ ] Dispatch warning when pitch correction targets a still-streaming buffer `[A]`
- [ ] Real-device smoke: pitch toggle + over-cap burst with zero xruns `[B]`

## Behavior-change / changelog notes

- **D18 over-cap (behavior change, owner-approved 2026-06-09):** beyond 256 simultaneous
  voices, the oldest non-looping voice is stopped to admit the new play (or the new play is
  rejected if all are looping) instead of an unbounded pool. Changelog + README note.
- **Speed validation moves to dispatch:** an invalid negative-speed-with-pitch command now
  warns at the control plane (and is not sent) instead of being silently ignored per-voice on
  the callback. Strictly more feedback; changelog note.
- New `/metrics` counters (`input.resample_errors`, `input.overflow_dropped_samples`,
  `input.ratio_rejects`, `scratch_regrows`) — additive; document.

## Definition of Done

Lane A green (full gate; all new alloc-harness cases; parity; soak) · Lane B green incl. the
device smoke · behavior changes in `CHANGELOG.md`/`README.md` · `docs/bugs.md` residual entries
closed with citations · `docs/http-api.md` updated · out-of-scope discoveries logged ·
committed atomically · `cargo build --release` warning-free.
