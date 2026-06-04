# Known bugs & out-of-scope discoveries

This file tracks bugs and oversights noticed while working that are **out of scope** for the change in
progress, so they aren't lost. Each entry names the owning sprint where known.

## Deferred to a later sprint

- **HTTP windowed streaming is still deferred (redesign S1/S2 — intentional).** Both explicit `mode=stream`
  and auto-windowing apply to **local files** only. For `http://`/`https://`, `mode=stream` logs a warning
  and loads the URL fully, and `mode=auto` always full-loads (no windowing decision is made). A later sprint
  adds HTTP windowed streaming, where the load decision can probe the source and reuse the same connection
  (avoiding a double GET). The hard memory cap still governs the *cache* of HTTP full-loads.
- **Local auto-windowing for header-less files (redesign S2 — RESOLVED).** `mode=auto` probes the local file's
  header (`cache::strategy::probe_local_file`) for a decoded-size estimate. When the container carries a frame
  count the estimate is exact (`frames * channels * 4`); when it does not (e.g. a frame-count-less VBR MP3/OGG),
  it now falls back to a conservative size-based estimate (`decoded_size_estimate_from_file_size`, file bytes ×
  a codec-class factor biased **up**), so a large count-less file still trips the budget/size checks and windows
  rather than full-loading into an OOM. The estimate is approximate and biased toward windowing, so the only
  residual is that an unusually large *compressed-but-would-fit* file may window (lose seek/loop/pitch) when a
  full load would just barely have fit — the safe direction. Covered by `cache::strategy::tests::size_fallback_*`.
- **Incremental HTTP-to-disk persistence of streamed plays is deferred (redesign S3 — LOW).** An ad-hoc HTTP
  `play` of an uncached URL streams to the *memory* cache only (`cache/mod.rs` `start_streaming_load`), so a
  restart re-downloads it. The blocking precache path (`precache_blocking`, `download_and_cache`) already
  persists HTTP assets to disk, and `cache_reload` re-warms an entry, so this only affects URLs that are
  played ad-hoc and never precached. The S3 freshness work added stale-while-revalidate for HTTP entries that
  *are* disk-cached (the freshness tick + `revalidate_stale_http`); teeing raw bytes from the streaming
  download to disk (so an ad-hoc streamed play also persists) is the remaining piece, deferred as low value
  for the disk-focused first client.
- **The seek/speed gate for streamed voices is voice-keyed and best-effort (redesign S1 — LOW).**
  `selector_targets_streamed_voice` (`src/main.rs`) warns when a Seek/Speed selector names a voice that has a
  streamed source. A selector that targets a streamed source by `file`/`id` only, or a voice that mixes
  streamed and full-load sources, may not warn — but the command is a structural no-op on streamed sources
  regardless (they live in a separate voice list the sample-targeted commands never touch), so this is a UX
  nicety, not a correctness gap.
- **Per-streamed-source gauge telemetry is deferred (redesign S4 — LOW).** `/metrics` exposes the cache as
  whole-system gauges (`cache.memory_bytes`/`memory_entries`/`memory_headroom_bytes`/`disk_bytes`, all real),
  and the per-play windowing decision and its reason (estimated decoded MB vs. cache headroom MB) are emitted
  as `tracing::info!` events in `should_window_local` (`src/main.rs`). What is **not** surfaced is
  *per-streamed-source* runtime state: each windowed voice's ring fill level, its cumulative underrun-frame
  count, and a live full-vs-windowed-per-voice list on `/status`. The control plane cannot read `MixerState`
  (D22a), so these would need the audio thread to publish per-source atomics (ring fill, underrun counter) over
  a side channel into the control-side snapshot — the same plumbing pattern as the xruns `Arc<AtomicU64>`, but
  per voice. Deferred as low value for the disk-focused first client: the never-OOM guarantee is observable
  today via `memory_cap_bytes` and `memory_headroom_bytes` (→ 0 is the pressure signal) and the windowing logs,
  and glitch-freeness is gated by the alloc harness + render tests, not a runtime metric. (The absolute resolved
  cap is now surfaced as `cache.memory_cap_bytes`; the remaining gap is purely the per-source atomics.)
- **`handle_command`'s own "Invalid JSON" 400 branch is unreachable (Sprint 9 F10 — discovered, LOW).**
  `src/http/handlers.rs` `handle_command` extracts `Json(body): Json<Value>` then maps a
  `serde_json::to_string(&body)` failure to `400 + CommandResponse::error("Invalid JSON: ...")`. But by the
  time the handler runs, axum's `Json` extractor has *already* parsed the body into a `Value`; re-serializing
  a `Value` cannot fail, so that branch is dead. A genuinely non-JSON body is rejected earlier by the
  extractor with axum's own `400` whose body is **plaintext** (`"Failed to parse the request body as JSON:
  ..."`), not a `CommandResponse`. So the Sprint-9 spec's "400 + `CommandResponse` error shape" for
  `/command` cannot both hold against the current handler: the status is 400 but the body shape is axum's.
  `tests/http_api_test.rs::test_command_non_json_body_returns_400` locks the *actual* behavior (400 +
  plaintext). Making the body a `CommandResponse` would mean adding a custom `JsonRejection` handler (or a
  `WithRejection` wrapper) — a production change owned by the HTTP-handlers work, out of scope for the
  additive test-gap group. Left as-is and surfaced here.

- **Deployment `Dockerfile` Rust version vs `usize::is_multiple_of` (stable 1.87) (Sprint 8 — RESOLVED).**
  `src/audio/streaming.rs` and `src/audio/input.rs` call `usize::is_multiple_of`, which clippy `-D warnings`
  *requires* (lint `manual_is_multiple_of`) on the validate image (`docker/validate.Dockerfile`, `rust:1.95`).
  The deployment image (`/Dockerfile`) previously pinned `rust:1.83`, which predates that API and would fail to
  compile. It now pins `rust:1.95-bookworm` (matching the validate image), so both images build the same code;
  this is resolved. Left on record so the version coupling between the two Dockerfiles is documented.

- **Drift control uses a pure-proportional loop, so the ring settles near — not exactly at — half-full (Sprint 8 — LOW, by design).**
  `steer_ratio` (`src/audio/input.rs`) is a P controller on the smoothed ring fill. Holding a steady clock
  mismatch requires a steady non-zero ratio deviation, which a P loop produces only with a steady non-zero fill
  error: the ring therefore parks at an offset from half (e.g. ~±20% of half at a 1% mismatch, less for realistic
  ppm-scale drift) rather than dead-centre. This satisfies D33's goal (bounded fill, never 0/capacity) and keeps
  the loop simple and provably stable. If a future need wants the fill pinned to half-full (e.g. to maximize
  symmetric headroom), add a small integral term (PI) with anti-windup — note it here rather than building it now
  (YAGNI). Covered by `tests/input_resample_test.rs::steering_converges_to_true_clock_ratio`.

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

- **`xruns` counter surfaced on `/status` and `/metrics` (Sprint 5 residual, CLOSED in Sprint 9).** The
  lock-free RT engine creates an `xruns: Arc<AtomicU64>` in `main.rs`, passes it to the output supervisor,
  and increments it in the cpal error callback (`src/audio/engine.rs:437`). Sprint 9 threads that same `Arc`
  into `AppState` and reads it in `handle_status` (`/status` → `xruns`) and `handle_metrics`
  (`/metrics` → `xruns`), closing the Sprint-5 exposure gap. **Semantic note (surface what exists, do not
  overstate):** the counter counts **cpal stream-error callback invocations**, not per-buffer underruns. On
  most platforms a dropout/underrun surfaces as a stream error (which also triggers the Sprint-1 device
  rebuild), so this is the available dropout signal — but a backend that silently glitches without raising a
  stream error would not bump it. There is no lower-level per-block underrun counter in cpal to surface; if a
  finer dropout metric is ever needed it must come from a backend that reports it. The value is real and
  live, never a placeholder.

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

- **Two file-level `#![allow(dead_code)]` "Phase 10" banners remain in the streaming decode path (Sprint 9 F2 — deferred, LOW).**
  Sprint 9 F2 removed the stale banner in `src/audio/chunked_resampler.rs` (the three now-genuinely-dead
  test-only accessors `buffered_frames`/`chunk_size`/`ratio` were narrowed to `#[cfg(test)]`). The same
  "Allow dead_code until Phase 10 …" banner still sits at `src/audio/streaming_decoder.rs:4-6` and
  `src/cache/http_stream.rs:4-6`. Removing the `chunked_resampler` banner did **not** surface warnings there
  (each file's own blanket still masks whatever is dead inside it), so per the Sprint 9 spec they were left in
  place — the gate is green without touching them. De-masking each would require auditing and narrowing every
  dead item in those two files individually (a larger change than F2's scope). Pick them up the same way
  (remove the blanket, then `#[cfg(test)]`/narrow-`#[allow]`/delete per item) when that path is next touched.

  Audited under Sprint 9's final gate (banners temporarily removed, `RUSTFLAGS=-D warnings` build) — the exact
  set a future de-mask must resolve, and the right disposition for each (the binary does not call into the
  streaming decode path; these items are exercised only by the lib's integration tests and the files' own
  `#[cfg(test)]` modules, which is why they are dead in the `bin` target but live in the `lib`/test targets):
  - `src/cache/http_stream.rs`: `HttpStreamReader::bytes_available` is **genuinely dead** (zero callers in
    `src/` or `tests/`; its logic is inlined in `read()`/`wait_for_data()`) — **delete** it. `HttpStreamError::Cancelled`
    is never constructed today (the download task reports cancellation via `fail("Download cancelled")`, not the
    variant) — either wire it or **delete** it. `cancel`/`is_complete` are public API used by the file's own
    `#[cfg(test)]` tests — narrow to `#[cfg(test)]` or keep as documented public API.
  - `src/audio/streaming_decoder.rs`: the `sample_rate`/`target_sample_rate`/`estimated_frames` **fields** and
    the `source_sample_rate`/`sample_rate`/`estimated_frames`/`is_finished` **accessors** are public API consumed
    only by that file's `#[cfg(test)]` tests; keep as documented public API (or `#[cfg(test)]` the accessors) —
    do not delete the fields, they back the public accessors.

- **Unused `ctrlc` dependency after dev-scaffolding removal (Sprint 9 F1 — RESOLVED in Sprint 9 finalization).** The
  `ctrlc = "3.4"` dependency (`Cargo.toml`) was used **only** by the removed `--test-tone`/`--file`/`--test-mixer`
  handlers; the daemon's real shutdown path uses `tokio::signal::ctrl_c()`/SIGTERM in `shutdown_signal()`, not the
  `ctrlc` crate. F1 removed the handlers, leaving `ctrlc` unused (zero references anywhere in `src/`; `cargo tree -i
  ctrlc` showed `mqttaudio` as its only dependent, so nothing transitive needed it). It produced no compiler
  warning, so the gate was unaffected, but shipping a dead dependency in the final release is exactly the
  tree-cleanliness this sprint exists to close. The Sprint-9 finalization pass removed it from `Cargo.toml` and
  `Cargo.lock` (dropping `ctrlc v3.5.0` + its `cfg_aliases v0.2.1`); the full gate stays green.

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

- **Auto voice-id format: `_auto_<millis>_<n>` shipped, reconciling DECISIONS.md D24 vs D41/Sprint-9 F5.**
  DECISIONS.md D24 (Sprint 6) specified `_auto_<n>` (a bare monotonic counter, no millis). Sprint 9's F5
  finding, decision D41, and the Sprint-9 orchestration brief all specify `_auto_<millis>_<n>` (the original
  `_auto_<millis>` with a process-global `AtomicU64` counter appended). The later, more specific Sprint-9
  decision was implemented and is what ships (`next_auto_voice_id()` in `src/main.rs`, tested by
  `auto_voice_ids_are_unique_within_a_millisecond` and `two_no_voice_plays_get_distinct_voice_ids`). Both forms
  satisfy D24's actual requirement — uniqueness, so same-millisecond no-voice Plays no longer collide into one
  voice — and the shipped form is strictly more informative (it carries the start timestamp). The CHANGELOG
  documents `_auto_<millis>_<n>` as the user-visible contract. Noted here only so the D24↔D41 wording mismatch
  is on the record; no further action needed.

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

- **Sprint 8 control-plane fixes (F3/F5/F7/F8) land on the lock-free command path, not the pre-Sprint-5
  in-handler mutation the sprint file references.** The Sprint 8 file's `main.rs:1011`/`:1024` line refs for
  the mute restore (D34) and `main.rs:881`/`:899` for `voice_volume` (D35) predate the Sprint-5 engine, which
  moved all `MixerState` mutation onto the audio thread behind the command ring (D22a). So:
  - **F8 (D34) mute restore** lives on the audio side: `LiveInput` gained `muted`/`pre_mute_volume` and
    `set_muted`/`set_volume` (`src/audio/mixer.rs`); a new `AudioCommand::SetInputMute { input, mute }` is
    applied by `rt_engine::apply_input_mute`. The control handler (`main.rs` `InputMute`) no longer sends a
    hardcoded volume — it sends the mute bool. Setting an explicit `input_volume` clears the muted state
    (`set_volume`) so a later unmute does not revert it.
  - **F7 (D35) `voice_volume` on input-only voices**: `rt_engine`'s `SetVoiceVolume` already iterated
    `live_inputs`, but the control handler gated the *send* behind a `VoiceManager` hit, so an input-only
    voice never reached the audio thread. The control plane cannot read `live_inputs` (D22a), so the handler
    now also treats a match against the configured input voice ids it owns (`ctx.inputs`) as success and
    sends the command.
  - **F3 graceful underrun** is in `mix_live_input_into_output` (`src/audio/mixer.rs`): on a ring shortfall it
    holds the last frame and fades it to silence over `UNDERRUN_FADE_FRAMES` (64) instead of `break`ing, and
    keeps calling `advance_voice_volume()` for the silent frames so the ramp stays time-accurate. It uses only
    fixed stack arrays (RT-safe; `tests/alloc_harness.rs::live_input_underrun_mix_is_allocation_free_after_warmup`).
  - **F5 route-channel warning** is a pure helper `out_of_range_input_routes` in `main.rs`, called at input
    setup once `active_input.channels` is known, returning the warning string the caller logs (so the unit
    test asserts content without a log-capture dependency, keeping test output pristine).

- **Sprint 8 D33 always-on async SRC adds input latency even at equal rates (by design, latency note).**
  Routing every input through `SincFixedIn` (`src/audio/input.rs`, `sinc_len: 256`, `RESAMPLE_CHUNK_SIZE:
  1024`) for drift control means the equal-rate (e.g. 48000→48000) path now carries the resampler's
  fixed group delay plus the 1024-input-frame chunk accumulation before each emit — latency the removed raw
  passthrough did not have (~tens of ms at 48 kHz, on top of the configured `latency_ms` ring). This is the
  accepted cost of D33 (bounded ring vs. a passthrough that drifts to a click); it is not a bug. If a
  latency-critical input ever needs it trimmed, a smaller resampler chunk trades the buffering latency for
  more frequent `process_into_buffer` calls (still alloc-free) — note it here rather than tuning it now
  (YAGNI; no decision requires a specific input latency budget).

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
