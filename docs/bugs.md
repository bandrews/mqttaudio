# Known bugs & out-of-scope discoveries

This file tracks bugs and oversights noticed while working that are **out of scope** for the change in
progress, so they aren't lost. Each entry names the owning sprint where known.

## Deferred to a later sprint

- **No final low-pass on the summed LFE bus (Sprint 7 — LOW, YAGNI per D32).** Bass management low-passes each
  source channel before summing into the LFE, but the LFE *output bus* itself is not low-passed after
  summation. Per D32 a final-LFE low-pass is deliberately **not** added now (YAGNI): each contribution is
  already band-limited by its per-channel low-pass, so the sum is band-limited too. The one case it would
  catch is full-range content routed *directly* to the LFE output index by another voice (documented as the
  additive-LFE collision in `docs/features/bass-management.md`) — that directly-routed content bypasses the
  crossover and is not low-passed. If that becomes a real problem, add a single low-pass on the LFE bus after
  summation in `BassManagement::process`.

- **Alloc harness cannot see the pitch stretcher's C++ allocations (Sprint 6 — LOW, harness limitation).**
  `tests/alloc_harness.rs` hooks Rust's `#[global_allocator]`, so it proves the Rust-side mix/command/ducking
  paths are alloc/free-free. The `signalsmith-stretch` pitch path is C++ FFI (`process`/`seek`/`flush`) and
  allocates via the C++ runtime, which does **not** route through the Rust allocator — so the pitch alloc
  tests cannot prove the C++ side is RT-clean. signalsmith is designed to pre-allocate at construction and not
  allocate per `process`, but the F10 pre-roll/seek and F12 flush primitives are unverified by the harness.
  If pitch-correction RT-safety must be guaranteed, verify with a C++-level allocation tool (or a real-device
  xrun soak) — pairs with the Sprint 6/9 pitch follow-ups.

- **Ducking `apply_target` clones the voice string on the RT thread on first duck (pre-existing, Sprint 5 — LOW).**
  `DuckingApplier::apply_target` does `duck_states.entry(change.voice.clone()).or_insert_with(..)`; the first
  time a given voice is ducked, the `HashMap` insert allocates (the owned key + possible map growth) on the
  audio thread. It is one small allocation per *newly-ducked* voice (not per buffer), pre-dates Sprint 6, and
  the alloc gate's ducking tests warm the voice first so they stay green. Eliminate by pre-populating
  `duck_states` for the configured ducked voices at applier construction (off-RT). Fold into the Sprint 9
  cleanup or the D18 voice-pool follow-up.

- **Resampler (rubato) sinc interpolation stays `Linear` (Sprint 6 R1 — LOW, optional, no decision).** The
  sample-rate-conversion resampler (`src/audio/resampler.rs`, used when a file's rate differs from the device
  rate — not the playback-speed path) uses `SincInterpolationType::Linear` with the default `Fast` quality.
  The sprint lists `Cubic` as an optional, no-required-behavior-change "consider" item and there is no
  DECISIONS.md ruling forcing it (Sprint 6 decisions stop at D29). Switching the table to `Cubic` (cost is
  negligible — the table is precomputed) would change decoded PCM for every resampled file with no test
  pinning the result, so it was left as-is. `Fast` is intentionally not transparent; revisit with Sprint 9
  cleanup if transparency matters. The D27 cubic work above is the *playback-speed* interpolator, a separate
  code path.

- **Voice pool is a soft reserve, not a hard cap (Sprint 5, D17/D18).** `active_samples`/`live_inputs` are now
  pre-reserved to `MAX_VOICES` (256) / `MAX_LIVE_INPUTS` (16) at construction (`src/audio/mixer.rs`), so a Play
  never reallocates the Vec on the RT thread in practice — well past the documented "20+ simultaneous" target.
  But it is a *soft* reserve: exceeding the reservation makes `drain_commands`' `AddSample` push reallocate the
  backing store once, on the audio thread. The locked D18 over-cap policy (steal the oldest non-looping voice,
  else reject the new Play — both moving the displaced/rejected sample to the graveyard for off-RT drop) is not
  yet implemented; it needs the graveyard producer threaded into `drain_commands`. Until then the alloc gate
  proves the common case (`adding_a_sample_into_the_reserved_pool_is_free_free`) but not the >256 over-cap path.

- **A Speed command that TOGGLES pitch correction still allocates/frees on the RT thread (Sprint 5/9).**
  `SetSpeedMatching { pitch_correction }` is applied on the audio thread by `apply_mutation` ->
  `ActiveSample::set_speed_with_mode`, which calls `enable_pitch_correction` (constructs a signalsmith
  `Stretch`/`PitchCorrector`) or `disable_pitch_correction` (drops it). When the command flips the mode on a
  *live* voice, that create/drop happens in the callback — a heap allocation and/or free on the RT thread.
  The command-return ring (Sprint 5) keeps the *command husk's* heap off the RT thread, but not this
  corrector lifecycle. The alloc gate (`tests/alloc_harness.rs::draining_mutation_commands_is_free_free`)
  deliberately does **not** toggle pitch, so it stays green; this residual is real but out of scope here.
  Eliminating it needs the control thread to pre-build the `PitchCorrector` and send it inside the command,
  and to return the displaced corrector via a graveyard for off-RT drop (mirroring the sample graveyard) —
  a separate, larger change. Pairs with the Sprint-6 pitch-correction work.

- **`xruns` counter is incremented but not surfaced on `/status` (Sprint 5).** The lock-free RT engine
  creates an `xruns: AtomicU64` (`src/main.rs:553`), passes it to the output supervisor, and increments it
  in the cpal error callback (`src/audio/engine.rs`), but it is not yet plumbed into the control-side
  `StatusSnapshot` or the `/status` JSON. Sprint 5's acceptance ("expose it on the status snapshot for the
  soak test") and Sprint 9's `/metrics` work still need to wire it through. The counter is live and usable
  by a soak test that holds the `Arc<AtomicU64>` directly; only the HTTP exposure is missing.

- **`/status/samples` live position is gone by design (Sprint 5 / D20 / D22a).** Because the control plane
  never reads `MixerState` (D22a), the status snapshot is control-side and cannot see audio-thread-owned
  playback position. `/status/samples` therefore reports `position`/`position_ms`/`progress_percent` as 0
  (static metadata and `total_ms` are still accurate). If live position is wanted back, it must be fed to
  the control thread over a dedicated channel (the audio thread publishing per-voice positions), not by
  locking the callback state. Recorded as a deliberate behavior change, not a regression; changelog'd.

- **`DiskCache` uses `DefaultHasher` for cache keys (Sprint 3).** `src/cache/disk.rs:~147` derives the
  on-disk filename from `std::collections::hash_map::DefaultHasher`, which is not guaranteed stable across
  releases/platforms. Sprint 3 should switch to a stable content hash (truncated SHA-256 / xxhash).

- **`CacheError` variant names (`IoError`/`JsonError`/`HttpError`) (Sprint 9).** clippy's
  `enum_variant_names` fires on the shared `Error` suffix. Renaming ripples through many match arms, so it
  is suppressed with a targeted `#[allow(clippy::enum_variant_names)]` for now (`src/cache/disk.rs`).

- **Stale `#![allow(dead_code)]` "Phase 10" banner (Sprint 9, D39).** `src/audio/streaming_decoder.rs`
  still carries a crate-style `#![allow(dead_code)]`. Sprint 9 removes dev scaffolding and must make the
  build warning-free without this blanket allow.

- **Hardcoded `/Users/bandrews/...` paths (Sprint 9, D39).** `audio::engine::test_mixer` embeds absolute
  developer paths for its test files. Sprint 9 removes `test_mixer`/`--test-mixer` and the dev scaffolding.

- **ALSA name matcher hardening deferred (Sprint 1 F6 — LOW, Linux-only).** `try_match_alsa_device` /
  `extract_alsa_card_from_name` in `src/audio/engine.rs` compare only the card identifier (ignoring the
  DEV/subdevice), miss some prefixes (`hdmi:`/`iec958:`), and re-read `/proc/asound/cards` per comparison.
  This is the *secondary* device-resolution path — the exact enumerated-name match is tried first and
  handles the common case — so the realistic worst case is right-card/wrong-subdevice or a missed `hdmi:`
  device when selecting by a non-enumerated name. The fix (subdevice comparison + a cached `/proc` map) is
  Linux-only and can only be verified in Lane A; it was deferred to keep Sprint 1 focused on the critical
  non-f32 crash and the gating acceptance criteria. Pick it up when a real device exposes the mismatch
  (also a fit for Sprint 9's device cleanup). Prefix broadening is the cheap first step.

- **Chunked-vs-one-shot resampler divergence (Sprint 4 caveat — LOW, consistency-only).** The streaming
  decode path resamples in chunks (`StreamingDecoder` + `ChunkedResampler`), zero-padding the final
  partial chunk and truncating to the expected frame count without compensating rubato's startup delay,
  while disk-cache hits and local files use the one-shot `decoder::decode_file`. The two produce slightly
  different PCM for the same source (the `matches_full_decode` test tolerates the diff); it is not an
  audible glitch. Sprint 4's promotion (F2) means a streamed URL resolves to the one-shot path on replay,
  so the divergence only affects the first, still-streaming play. Left for Sprint 9 cleanup — do not
  rewrite the resampler for this.

- **Reverse-loop crossfade still replays the overlapped tail (Sprint 6 F6, out of scope).** The F6
  overlap-on-wrap fix is forward-only, matching the sprint's F6 statement and its forward transient-loop
  test. The *reverse* loop crossfade has the symmetric defect: approaching frame 0 it fades in the buffer's
  end region `[buffer_frames-cf .. buffer_frames]`, then the reverse wrap (`new_pos < 0` in
  `advance_position`, mirrored in `mix_sample_into_output`) lands back near `buffer_frames` and replays that
  tail at full level — the reverse analogue of the head double-trigger. A symmetric fix would land the
  reverse wrap at `buffer_frames - cf` (effective loop `[0, buffer_frames-cf)`) and skip the inline reverse
  wrap's replay, gated by the same `crossfade_active()` predicate. It was left out here because the sprint
  scoped F6 to the forward head and no existing test exercises reverse looping with `crossfade_ms > 0`;
  adding it would be an unverified change to the twin path. Pick it up if reverse looped crossfades are
  exercised in earnest.

## Implementation notes

- **Sprint 6 pitch pre-roll (F10) covers the mid-playback enable, not first-enable-at-start.** Enabling pitch
  correction while a sample is already playing pre-rolls the stretcher with the audio just before the
  playback position and equal-power crossfades direct→stretched over ~5 ms, so there is no silent gap and no
  click (`enabling_pitch_correction_mid_playback_has_no_silent_gap`). When pitch correction is enabled at the
  very start of a sample (`position == 0`) there is no prior audio to pre-roll or crossfade from, so the
  stretcher's own short synthesis warm-up still applies for the first block; this is inherent to the stretcher
  and is not a regression (a sample that *starts* pitch-corrected has always begun this way). The pre-roll
  reads a borrowed slice of the already-decoded buffer and the EOF tail buffer is sized when correction is
  enabled, so the per-block mix path allocates nothing (`pitch_eof_flush_and_drain_is_free_free`, plus the
  existing `pitch_mix_path_is_allocation_free_after_warmup`).
- **Sprint 6 pitch tail flush (F12) requires a single full `flush`.** `signalsmith-stretch`'s `flush` drains
  its entire buffered tail in one call sized to `output_latency`; calling it repeatedly with small
  (block-sized) buffers returns silence after the first call. The pitch path therefore drains the whole tail
  once at EOF into a pre-sized `pitch_tail_buffer` and plays it out block by block, rather than calling
  `flush` per block.

- **Sprint 6 output-stage ordering: finite-guard runs BEFORE the limiter.** The sprint file's task list is
  internally inconsistent on the final-loop order (F3 task says "the finite-guard runs first"; F1 task lists
  "limiter → finite-guard → ceiling clamp"). Implementation found the guard must run first on the gained
  value: the soft-knee limiter uses `tanh`, and `tanh(+Inf) == 1.0` is *finite*, so a `+Inf` sample would
  otherwise pass the limiter as a ceiling-level value and never be caught by a guard placed after it (the
  `nan_input_is_sanitized_to_silence` harness test, which injects `+Inf`/`-Inf` as well as `NaN`, proves
  this). The shipped order in `mix_audio` is therefore: per-channel gain (+ master gain) → finite-guard
  (non-finite → 0.0) → soft-knee limiter → hard clamp at the ceiling. This satisfies F3's intent
  (non-finite input → silence) and keeps the limiter (D26). Other output-stage findings (none here) should
  build on this order.

- **Sprint 6 ducking advance-once (D1): `advance_buffer` takes no distinct-voice list.** The sprint text
  proposes `advance_buffer(distinct_voices, frames)`. Collecting the distinct voice set in the callback would
  allocate (a `Vec`/`HashSet`), violating the RT no-alloc invariant (D22a, `alloc_harness`). Instead
  `DuckingApplier::advance_buffer(frames)` advances every existing `DuckState` exactly once by iterating the
  duck-state map in place — that map *is* the distinct-voice set, and iterating it allocates nothing. Each
  state snapshots its buffer-start/-end multiplier; the mix loop then reads a per-frame interpolation
  (`buffer_endpoints` + `mixer::duck_frame_gain`, D2) without advancing. This is the documented deviation
  required by the no-alloc invariant; the behavior (each voice's fade advances once per buffer regardless of
  how many samples/inputs reference it) is exactly what D1 asks for.

- **Sprint 6 input ducking (D4) activates for the input's whole stream lifetime; signal gating is Sprint 8
  (D36).** Configured input voices are marked active once at input setup (via the same off-RT
  `notify_voice_activity` → `compute_changes` path the sample-playback path uses), so a microphone whose
  `voice_id` is a ducking `primary_voice` ducks its background while the input stream is open. There is no
  input-level gate yet, so the duck does not follow the microphone's signal (it holds while the input is
  running). The ring-buffer level gate that makes ducking follow actual speech/level is Sprint 8 (D36); it
  will drive the *same* notify path, so only the activation trigger changes. The Sprint 6 tests drive the
  notify directly, as the sprint specifies.

- **Sprint 6 speed interpolation is cubic; speeds > 1 still alias (F8/D27 — documented, no resampler).** The
  non-pitch speed path now uses 4-point Catmull-Rom cubic interpolation (`cubic_taps`/`cubic_interpolate` in
  `src/audio/mixer.rs`), which has a much flatter passband than the old two-point linear read and so produces
  far less spurious energy when resampling at a fractional ratio (`fractional_speed_interpolation_is_low_distortion`
  drops the residual from ~0.07 to ~0.01). Per D27 the `±100` speed range is **kept** and **no speed cap was
  added**: cubic interpolation is a reconstruction filter, not an anti-aliasing (decimation) filter, so
  speeds **> 1.0** on the non-pitch fast path still **alias** — there is no low-pass before the stride-based
  downsample. This is the locked, documented trade-off (D27: "document that speeds > 1 alias on the fast
  path"); routing the fast path through a band-limited decimating resampler is explicitly YAGNI for now.
  Users who need clean large speed-ups should enable pitch correction (the stretcher path), which is
  band-limited. F8's alternative remedy (a hard cap well below 100×) was therefore **not** taken, so the
  documented `set_speed` range is unchanged and no existing config/command behavior is restricted.

- **Sprint 6 per-route downmix gain (D29) covers sample channel maps, not live-input routes.** The optional
  per-route `gain` (default 1.0) is wired through the Play `channel_map` (`ChannelMapping.gain`) into
  `ActiveSample::channel_route_gains` and applied in both the normal and pitch-corrected sample mix loops
  (`src/audio/mixer.rs`); an absent gain reads as unity, so existing 1:1/sum routing is bit-identical
  (`test_channel_route_gains_default_to_unity`, `test_quad_to_stereo_downmix_with_route_gains`). This is the
  exact location of the F7 finding (the sample downmix sum at `output[dest_idx] += …`). Live-input routes
  (`config.inputs[].routes`, `InputRouteConfig`) are a separate config surface and are **not** given a
  per-route gain here — F7/D29 are scoped to the sample downmix where overlapping source channels sum and
  clip. Add an input-route gain alongside the Sprint 8 live-input work if input downmix clipping shows up.

## Architectural notes

- **Binary re-declares modules instead of using the library crate.** `src/main.rs` declares its own
  `mod audio; mod cache; mod config; mod http; mod mqtt; mod voice;` rather than depending on the
  `mqttaudio` lib, so the modules are compiled twice and the two trees have drifted (the lib's `mqtt`
  module omits `logger`, which the binary uses). Because of this, `handle_command`/`CommandCtx` live in the
  binary crate and are covered by binary unit tests. Unifying the binary onto the library crate would halve
  compile time and make the command path testable from integration tests, but it is a larger refactor than
  the validation-harness sprint should take on. Candidate for Sprint 9.

## Fixed in passing (Sprint 0, to make the gate meaningful)

- `benches/mixer_benchmark.rs` no longer compiled: `ActiveSample::new_with_mapping` had gained
  `loop_mode`/`crossfade_samples` parameters that the bench call didn't pass. Updated the call.
- Removed two unused `#[cfg(test)]` methods (`BassManagement::reset`, `BassManagement::sample_rate`) that
  had no callers and tripped `dead_code` under `-D warnings`.
- Test reads in `src/cache/http_stream.rs` used `Read::read` and ignored the returned count
  (`clippy::unused_io_amount`, deny-by-default); switched to `read_exact`, which is what the assertions
  already assumed.
