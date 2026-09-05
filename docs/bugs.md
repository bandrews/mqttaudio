# Known bugs & out-of-scope discoveries

This file tracks bugs and oversights noticed while working that are **out of scope** for the change in
progress, so they aren't lost. Each entry names the owning sprint where known.

## Deferred to a later sprint

- **HTTP windowed streaming (redesign — RESOLVED).** Windowing now applies to `http://`/`https://` as well as
  local files. An uncached HTTP `play` opens the response headers (`open_http_stream`), and the load decision
  (`strategy::decide` + `strategy::probe_http` on `Content-Length`, against the live memory budget) routes a
  big/over-budget/unknown-size URL to a windowed play; a small one falls through to the full (cache) load on the
  same logic. A windowed HTTP play streams through a forward-only, back-pressured `BoundedHttpReader` (resident
  memory O(channel) compressed bytes) feeding the decoded window ring (O(window) samples) — so even a multi-hour
  WAV over HTTP cannot OOM. A live stream (no `Content-Length`) always windows (it has no finite end to
  full-load). The windowed branch downloads a single request; a full-load fallback costs only the headers.
  *Remaining piece:* persisting a cacheable windowed download to disk — see the incremental-persist entry below.
- **Local auto-windowing for header-less files (redesign S2 — RESOLVED).** `mode=auto` probes the local file's
  header (`cache::strategy::probe_local_file`) for a decoded-size estimate. When the container carries a frame
  count the estimate is exact (`frames * channels * 4`); when it does not (e.g. a frame-count-less VBR MP3/OGG),
  it now falls back to a conservative size-based estimate (`decoded_size_estimate_from_file_size`, file bytes ×
  a codec-class factor biased **up**), so a large count-less file still trips the budget/size checks and windows
  rather than full-loading into an OOM. The estimate is approximate and biased toward windowing, so the only
  residual is that an unusually large *compressed-but-would-fit* file may window (lose seek/loop/pitch) when a
  full load would just barely have fit — the safe direction. Covered by `cache::strategy::tests::size_fallback_*`.
- **Incremental HTTP-to-disk persistence of windowed plays (redesign — RESOLVED for cacheable windowed plays).**
  A windowed play of a *cacheable* HTTP URL (one with a `Content-Length`, not overridden `cacheable: false`)
  now tees its download to a temp file and atomically renames it into the disk cache on completion, registering
  the entry (`http_stream::PersistTarget`/`PersistSink` → `CacheManager::record_streamed_download`), so a
  restart/replay hits disk with no extra GET (covered by
  `streaming_test::cacheable_windowed_play_persists_and_replay_hits_disk`). A live source (no `Content-Length`)
  or `cacheable: false` is intentionally not persisted. *Update (Sprint 12, D55):* a small uncached HTTP **play**
  now full-loads over the probe's reused response and tees a cacheable download to disk
  (`start_streaming_load_from_reader`), closing the main re-download case. The remaining unpersisted case is the
  direct cache-API full-load streaming path (`start_streaming_load`, e.g. `precache` of an uncached URL in
  non-blocking mode), which still writes only to the memory cache — narrow and managed, left as is.
- **Cold local and disk-cached-HTTP full-loads block on a complete decode before any sound (Sprint 10
  evaluation — RESOLVED in Sprint 12, D51).** All three cold full-load paths now return a progressive
  `SampleBuffer::Streaming` immediately (`start_file_streaming_decode` in `src/cache/mod.rs`; the decoder is
  constructed synchronously so channels and the total estimate are right from the start), with
  stat-at-load-start freshness, a generation-guarded promotion, a `start_position` gate, and a
  reaper-driven upgrade of the playing voice to the promoted Complete buffer (the `UpgradeSampleBuffer`
  command) so seek/loop-crossfade/pitch regain full-decode semantics once the load finishes. Measured: a
  cold 300 s WAV is playable in ~0.2 ms vs ~210 ms before (x86). See
  `docs/sprints/sprint-12-first-start-latency.md` and the CHANGELOG.
- **`invalidate`/`cache_reload` does not abandon in-flight streaming loads (Sprint 10 trace — RESOLVED in
  Sprint 12, D52).** `invalidate` now removes the matching `active_loads` entry: completion no longer
  promotes stale content and new plays no longer join the abandoned stream (the playing voice keeps its own
  Arc and finishes normally). Discovered and fixed in the same pass: an **errored** streaming load used to
  linger in `active_loads` forever, so replays of a failed URL silently joined the dead buffer —
  `cleanup_completed_loads` now drops errored loads (without promotion) so replays retry fresh. The related
  suspicion about the background `revalidate_stale_http` tick was **refuted** — it only touches URLs both
  disk-cached and memory-resident, which an active load can never be.
- **Streaming-buffer promotion copies the whole buffer (Sprint 10 observation — LOW, not scheduled).**
  `cleanup_completed_loads` promotes via `guard.data().to_vec()` (`src/cache/mod.rs:483`), a transient 2×
  memory spike bounded by `full_load_max_bytes` (≤ ~64 MiB total at the 32 MiB default). Acceptable today;
  if it ever matters, promote by moving the storage out of the `StreamingBuffer` instead of copying. Sprint
  12 touches this code — re-evaluate in passing, do not redesign for it.

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
  additive test-gap group. **RESOLVED in Sprint 14 F3 (D61, owner-approved):** `handle_command` now extracts
  `Result<Json<Value>, JsonRejection>` and answers a non-JSON body with 400 + the `CommandResponse` JSON
  shape; the unreachable internal branch is gone; `test_command_non_json_body_returns_400` locks the new
  contract; `docs/webui/API-CONTRACT.md` updated.

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

- **Ducking `apply_target` clones the voice string on the RT thread on first duck (Sprint 5 residual —
  RESOLVED in Sprint 13 F1).** `DuckingApplier::with_ducked_voices` pre-populates the duck states from the
  configured rules' `ducked_voices` at construction (off-RT); `apply_target` is now `get_mut`-only and never
  inserts or allocates on the audio thread, even for the FIRST duck of a voice. The harness's
  warm-the-voice crutch is gone: `alloc_harness::first_duck_of_a_never_seen_voice_is_allocation_free` arms
  around the first duck itself.

- **Resampler (rubato) sinc interpolation stays `Linear` (Sprint 6 R1 — LOW, optional, no decision).** The
  sample-rate-conversion resampler (`src/audio/resampler.rs`, used when a file's rate differs from the device
  rate — not the playback-speed path) uses `SincInterpolationType::Linear` with the default `Fast` quality.
  The sprint lists `Cubic` as an optional, no-required-behavior-change "consider" item and there is no
  DECISIONS.md ruling forcing it (Sprint 6 decisions stop at D29). Switching the table to `Cubic` (cost is
  negligible — the table is precomputed) would change decoded PCM for every resampled file with no test
  pinning the result, so it was left as-is. `Fast` is intentionally not transparent; revisit with Sprint 9
  cleanup if transparency matters. The D27 cubic work above is the *playback-speed* interpolator, a separate
  code path. **CLOSED in Sprint 14 F1 as "stay Linear" (D59 overridden on measurement, per the Charter's
  evidence rule):** a least-squares tone-residual probe at the daemon's actual presets (sinc_len ≥ 64,
  oversampling ≥ 64, 15 kHz tone, 44.1k→48k) measured Linear and Cubic identical to ~0.015% of an already
  ≈-60 dB residual — the floor is the sinc filter, not the table interpolation, so the switch would change
  every rate-converted file's PCM for no measurable benefit and no pinnable test. The measured quality floor
  is now pinned by `resampler::tests::fast_preset_off_tone_residual_stays_below_minus_50_dbfs`, so R1 stops
  haunting this file either way.

- **Voice pool is a soft reserve, not a hard cap (Sprint 5, D17/D18 — RESOLVED in Sprint 13 F2,
  owner-approved 2026-06-09).** The D18 over-cap policy is implemented: the graveyard producer is threaded
  into `drain_commands`, and `add_sample_with_cap` (`src/rt_engine.rs`) holds the pool at `MAX_VOICES` —
  at the cap a new Play steals the OLDEST non-looping voice (lowest internal id, displaced to the graveyard
  for off-RT drop) or, if every slot loops, the new play is rejected to the graveyard. Alloc-free past the
  cap (`alloc_harness::over_cap_play_steals_alloc_free`); behavior locked by rt_engine unit tests and
  `soak_test::soak_past_the_voice_cap_steals_and_stays_capped`. Behavior change changelogged.

- **A Speed command that TOGGLES pitch correction still allocates/frees on the RT thread (Sprint 5/9 —
  RESOLVED in Sprint 13 F3, D56).** The daemon's Speed dispatcher now expands the selector control-side into
  one `SetSpeedWithCorrector` per matching sample, each shipping a control-built `PitchBundle` (corrector +
  pre-sized tail and scratch buffers, D58); the audio thread installs by move and, on disable, moves the
  displaced corrector into the husk for off-RT drop (`ActiveSample::apply_shipped_speed`). Rust-side
  alloc/free-freedom is proven by `alloc_harness::pitch_toggle_mid_play_is_rust_side_alloc_free`; the F10
  no-gap pre-roll parity by `render_harness_test::shipped_corrector_enable_matches_the_no_gap_behavior`.
  The selector-based `SetSpeedMatching` (which toggles on the applying thread) remains lib/test-only — the
  binary never sends it. The C++ (signalsmith) side stays invisible to the Rust harness as documented below;
  its construction now happens control-side, where allocation is fine.

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

- **`DiskCache` uses `DefaultHasher` for cache keys (Sprint 3 — RESOLVED, entry was stale).** Sprint 3
  shipped the fix (its tracker box "stable content hash replaces `DefaultHasher`" is checked) but this entry
  was never closed. Verified 2026-06-09: `src/cache/disk.rs:153-158` derives the filename from the first 12
  hex chars of a SHA-256 of the URL, stable across Rust versions and platforms. No work remains.

- **`CacheError` variant names (`IoError`/`JsonError`/`HttpError`) (Sprint 9).** clippy's
  `enum_variant_names` fires on the shared `Error` suffix. Renaming ripples through many match arms, so it
  is suppressed with a targeted `#[allow(clippy::enum_variant_names)]` for now (`src/cache/disk.rs`).

- **Two file-level `#![allow(dead_code)]` "Phase 10" banners remain in the streaming decode path (Sprint 9
  F2 — RESOLVED, entry was stale).** Verified 2026-06-09: no file-level `#![allow(dead_code)]` remains
  anywhere in `src/`, and the per-item dispositions below shipped with Sprint 9's final gate (its tracker
  box: "all three blanket banners removed; dead `bytes_available`/`Cancelled` deleted; test-only accessors
  narrow-allowed" — `bytes_available`/`Cancelled` are gone from `src/cache/http_stream.rs`; the remaining
  `#[allow(dead_code)]` annotations are the narrow, per-item, justified kind). The audit text below is kept
  for the record of what was dispositioned. No work remains.
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
  rewrite the resampler for this. *Scope note (Sprint 12, D51):* with all cold full-loads now progressive,
  every cold play of a rate-converted file takes the chunked path and its promoted cache entry keeps that
  PCM; the divergence remains tolerance-tested (`matches_full_decode`) and inaudible.

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

- **Dead config field `audio.channel_names` (web-UI doc reconciliation, out of scope, LOW).** `channel_names`
  is declared, deserialized, and unit-tested in `src/config.rs` (around `config.rs:84`) but is never read by
  the audio path — only `channel_aliases` (name → index) is used for routing/resolution and per-channel
  calibration. It is undocumented and appears to be a vestigial parallel to `channel_aliases`. Discovered while
  auditing the docs against the code for the web control app. Cleanup (remove the field, or wire it to
  something real and document it) is a future task; left untouched here to avoid an unscoped serde/behavior
  change. **RESOLVED in Sprint 14 F2 (D60, owner-approved):** the field, its default, and its assertions are
  removed; configs still carrying the key keep parsing (no struct opts into `deny_unknown_fields`), locked by
  `config::tests::removed_channel_names_key_still_parses`.

- **`/ws` never streams log lines — `WebSocketLogLayer` is not installed in the tracing subscriber (Sprint W1,
  daemon gap, MEDIUM).** `start_server` creates a `LogBroadcaster` (`src/http/mod.rs:123`) and `handle_socket`
  forwards whatever it broadcasts (`src/http/websocket.rs:57-103`), but the only producer of real frames,
  `WebSocketLogLayer::on_event` (`src/http/websocket.rs:144-169`), is `#[allow(dead_code)]` and referenced only
  by unit tests — it is **never added to the `tracing_subscriber` registry in `main.rs`**. So a `/ws` client
  receives the `{type:"connected"}` welcome frame and then nothing; the daemon's tracing output goes only to
  stdout/the MQTT log layer, never to the socket. Verified live: a running daemon logs `Processing command:
  StopAll` to stdout but emits no `{type:"log"}` frame. Impact: the web app's log console (Sprint W1) correctly
  renders connection state + the welcome (daemon version), but shows no log lines against a real daemon until
  this is wired. Fix (a daemon-side follow-up, out of scope for the frontend-only Sprint W1 per DW1): create the
  `LogBroadcaster` in `main.rs` before logging init, add `WebSocketLogLayer::new(broadcaster)` to the subscriber
  registry, and pass the same broadcaster into `start_server`. `docs/http-api.md` documents `/ws` log streaming
  as if it works, so it should be corrected or the layer wired. Discovered during Sprint W1 Lane B.
  **RESOLVED in Sprint 14 F4 (D62, owner-approved):** `main` now creates the `LogBroadcaster` before logging
  init, installs `WebSocketLogLayer` in the registry alongside the fmt/MQTT layers, and passes the same
  broadcaster into `start_server` — `/ws` clients stream real `{type:"log"}` frames
  (`websocket_test::ws_streams_live_tracing_log_lines_through_the_layer`); `docs/http-api.md` is now true.

- **No per-sample `windowed` flag on `/status/samples` (Sprint W5 F3 — RESOLVED, entry was stale).**
  Sprint W6 F4 shipped the real flag: verified 2026-06-09, `/status/samples` emits `"windowed": s.windowed`
  (`src/http/handlers.rs:749`). The web app's `total_frames === 0` inference was the interim heuristic. No
  work remains.

- **Sprint W7 telemetry — implemented scope vs deferred (LOW).** Sprint W7 shipped **output peak meters** (a
  per-output-channel atomic published from the limiter pass, alloc-free, gated) and the **`/ws/state` tick
  channel** (a ~15 Hz control-side timer broadcasting `{type:"tick", samples:[{internal_id,position_ms,
  progress_percent}], meters:{output:[...]}}` only when telemetry is on AND ≥1 client is subscribed) plus a
  `GET /status/meters` poll fallback. **Deferred (carry forward when wanted):** (1) **per-input capture-level
  meters** — needs a peak atomic in the live-input capture path (`input.rs`/`mix_live_input_into_output`) and a
  per-input field on the tick frame; (2) **RMS** alongside peak; (3) **discrete state-event frames** (separate
  `play`/`stop`/`seek`/`voice`/`ducking`/`sample_finished` messages) — the tick frame's sample list already
  carries the live state (a finished sample simply drops out of it), so the live experience works without them;
  emitting discrete events from the control-thread mutation points is an optimization. The web meters render the
  output bars live; per-input meters show nothing until (1) lands.

## Noticed while building the config editor (Sprint: config editor)

- **One binary unit test failed once under full-suite parallel load (name not captured).** During a full
  `cargo test`, the `--bin mqttaudio` unit suite reported `605 passed; 1 failed` exactly once; the same
  suite passed on the immediate rerun and on five consecutive isolated runs (606/606), and two further full
  runs were clean. The suite contains timing-sensitive audio/ramp tests, so a load-dependent flake is the
  likely shape. Nothing in the config-editor work touches those paths. If it recurs, capture the test name
  (`cargo test 2>&1 | tee` …) and pin it down.

- **`config.example.json` still documents the removed `audio.channel_names` field (RESOLVED).** The field
  was removed in Sprint 14 (D60); the live alias mechanism is `audio.channel_aliases`. The example parsed
  fine (unknown keys are tolerated) but taught a dead field. Both occurrences (top-level example and the
  nested production example) plus the `_notes` line now use `channel_aliases` with the correct
  name-to-index direction.

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

# Known Bugs and Rough Edges

Issues noticed while working on other things. Each entry says what is wrong and
why it was left alone, so a later change can pick it up deliberately.

A full quality review lives in `docs/quality-review-2026-08/`: its
`findings-*.md` files hold the verified issues, `triage.md` splits them into
fixed-vs-deferred, and `summary.md` is the short version. The deferred items
there (D1-D30) are the current backlog of known issues beyond this file.

## Shared physical capture (resolved in 2.1)

Matching input entries now share one capture stream and fan out to independent
logical strips. Device, latency, requested channel count, and requested sample
rate must match; routes and per-strip volume/voice remain independent.

## `fadeall` does not fade live inputs

`fadeall` fades the playing samples, matching what `stopall` stops. A live
microphone keeps running through it, which is usually what you want but is
worth knowing. `input_mute` is the per-microphone control.

## Audio/control lock contention (resolved in 2.1)

The real-time engine receives prepared commands through its ring. HTTP status and
microphone health use snapshots and atomics. The callback's parking_lot mutex is
not shared with control/HTTP work. Allocation and render harnesses cover this path.

## Input device reconnection still requires field validation

CPAL 0.18.2 recovers ALSA xruns. The output supervisor rebuilds permanently failed
output streams with backoff; it leaves recovered xruns and automatic route changes
running. Input streams do not yet have equivalent automatic rebuild on USB device
loss. Restart after reconnecting a lost microphone. Physical unplug/replug and
multichannel USB duplex behavior remain release-candidate field checks.

## Broker-backed tests

MQTT/TLS tests are enabled with `MQTTAUDIO_BROKER_TESTS=1` and
`MQTTAUDIO_TLS_CA`. The Linux validation container starts a private broker and
runs these checks. Device-opening checks require a separate explicit invocation.
