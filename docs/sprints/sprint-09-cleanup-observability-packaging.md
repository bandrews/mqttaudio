# Sprint 9 — Cleanup, Observability & Packaging

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 1–8 |
| Effort | M |
| Lanes | A (Docker) |
| Subagents | Optional (cleanup / observability / proptest+test-gaps can parallelize) |

## Goal

Close out the program: delete the dev scaffolding and dead-code masking so the build is honestly
warning-free **without** blanket `allow(dead_code)`; enrich the status surface (per-voice ducking, limiter/clip,
dropout/xrun, uptime) and add real `/version` and `/metrics` endpoints; tighten config validation and document
the known feature-interaction footguns; add the remaining property/fuzz and HTTP/WebSocket tests that earlier
sprints deferred; and ship the production packaging bits (a systemd unit + structured JSON logging option).
This is the last sprint, so it must leave the tree clean, the gate green, and the docs truthful.

## Why

By the time this sprint runs, Sprints 1–8 have done the hard behavioral work. What remains are the
low-severity cleanup, observability, packaging, and test-coverage items the audit flagged but that did not
gate any earlier fix. They share one theme: **make the daemon honest and operable in production.** The
scaffolding and the stale "Phase 10" `allow(dead_code)` banner actively hide real warnings (the banner claims
code "will be integrated soon" when it is already used by `streaming_decoder.rs`), which violates the Charter's
zero-warning gate. The status endpoint reports almost nothing an operator needs. The untrusted JSON surfaces
(`parse_command`, `expand_macros`, `config::load`) have only example-based tests and no fuzzing, so a malformed
payload that panics would be an undetected DoS. Several HTTP/WebSocket error paths and status value mappings are
asserted only structurally.

## Scope

**In scope**
- Remove dev scaffolding in `engine.rs` (`init_test_sine_wave` / `play_file` / `test_mixer`, incl. hardcoded
  `/Users/bandrews` absolute paths) and the `--test-tone` / `--file` / `--test-mixer` CLI flags + their divergent
  inline mixing paths in `main.rs`.
- Delete the stale `#![allow(dead_code)]` "Phase 10 … will be integrated soon" banner in `chunked_resampler.rs`
  (the code is used) and get the build warning-free **without** a blanket file-level `allow(dead_code)`.
- Status/telemetry enrichment in `handlers.rs` + new `/version` and `/metrics` routes returning real data.
- Config schema/versioning field, silent-validation-gap fixes (bass/source-channel sanity; clearer messages),
  and documentation/warnings for the four low-severity feature interactions.
- Unique auto voice ids (behavior change).
- proptest fuzz of config + command JSON (no panic) wired into the Docker pipeline.
- New HTTP-handler-error-path, WebSocket, and real-value status tests.
- A systemd unit + structured (JSON) logging option, both documented in `README.md`.

**Out of scope** (owned by other sprints — coordinate, do not duplicate)
- `MIN_BUFFER_FRAMES` (`streaming.rs:270`) and `mark_playing` / `cleanup_completed_loads` (`cache/memory.rs:181`,
  `cache/mod.rs:292`): these are **Sprint 4** (tracker §Sprint 4). If Sprint 4 wired or removed them, they will
  already be warning-clean; if it left them dead, that is Sprint 4's box, not this one. This sprint must not
  delete or rewire them — only confirm the build is clean and, if a residual dead-code warning remains there,
  raise it with the partner rather than silencing it.
- `ducking_rules.target_volume` finite-∈[0,1] validation is **Sprint 2** (tracker §Sprint 2, `total_cmp` line).
  This sprint adds the *other* validation gaps (channel sanity, message clarity, schema version) and does not
  re-implement the ducking-rule numeric check.
- The actual limiter/clip counter (Sprint 6) and xrun/dropout counter (Sprint 5) are **produced** by those
  sprints; this sprint only **surfaces** them in `/metrics` and `/status`. If a counter does not exist when this
  sprint runs (a prior sprint slipped), surface what does exist and log the gap in `docs/bugs.md` — do not
  invent a fake number.

## Findings addressed

Severities are from the audit dump (`[reviewer/triage]`). All file:line citations below were re-verified
against the current tree before writing.

### F1 — Dev scaffolding: test sine/file/mixer functions + hardcoded user paths + divergent CLI mixing paths
- **Statement:** Phase-1/2/4 scaffolding remains in the production binary, including absolute paths to one
  developer's home directory and three CLI flags that run their own ad-hoc mixing/playback paths separate from
  the real command loop.
- **Verified at:** `src/audio/engine.rs:358-412` (`init_test_sine_wave`), `:414-502` (`play_file`),
  `:504-599` (`test_mixer`), hardcoded paths at `:521-524`
  (`"/Users/bandrews/src/mqttaudio/tests/audio/test_440hz_2s.wav"` etc.); `src/main.rs:61-71` (`--test-tone`/
  `--file`/`--test-mixer` arg defs), `:205-230` / `:232-256` / `:258-283` (their handlers).
- **Severity:** suggestion / cleanup.
- **Evidence:** `test_mixer` builds its own `MixerState { … ducking_engine: None, bass_management: None }` and
  runs a private callback (`engine.rs:566-591`) — a second mixing path that diverges from the real `main.rs`
  callback and the real command loop. The hardcoded paths break on any other machine. The flags are user-facing
  (`#[arg(long)]`) but only exercise scaffolding.
- **Fix:** Delete the three `engine.rs` functions and their imports; delete the three `Args` fields and their
  three handler blocks in `main.rs`; remove now-unused imports (`AtomicU32`, `Mutex`/`MixerState`/`ActiveSample`
  if no longer referenced in `engine.rs`). Build must stay warning-free afterward.

### F2 — Stale `#![allow(dead_code)]` "Phase 10" banner masks real warnings (the code is used)
- **Statement:** `chunked_resampler.rs` carries a file-level `#![allow(dead_code)]` with a comment claiming the
  code "will be integrated soon," but `ChunkedResampler` is already imported and used by `streaming_decoder.rs`.
- **Verified at:** `src/audio/chunked_resampler.rs:4-6` (the banner + `#![allow(dead_code)]`); usage at
  `src/audio/streaming_decoder.rs:16` (`use crate::audio::chunked_resampler::{ChunkedResampleError, ChunkedResampler}`),
  `:84` (field), `:152` (`ChunkedResampler::new(`).
- **Severity:** suggestion / cleanup (but it defeats the Charter's zero-warning gate).
- **Evidence:** The comment is false; the blanket allow suppresses *all* dead-code warnings in the file, so any
  genuinely-dead item there is invisible. Sprint 0 explicitly deferred this banner to Sprint 9 (sprint-00 task 8).
- **Fix:** Delete the banner comment and the `#![allow(dead_code)]`. Build with `-D warnings`; if individual
  items are now genuinely dead, remove them (or add a *narrow, justified* `#[allow(dead_code)]` on the specific
  item with an accurate comment) rather than re-blanketing the file. The same scrutiny applies to the file-level
  blanket in `streaming_decoder.rs:6` and `cache/http_stream.rs:6` if removing the `chunked_resampler` banner
  surfaces them — but only touch those if they break the gate; otherwise note them in `docs/bugs.md`.

### F3 — Status/telemetry is threadbare; no `/version` or `/metrics`
- **Statement:** `handle_status` returns counts only; there is no per-voice ducking state, no limiter/clip count,
  no dropout/xrun count, no uptime, and no `/version` or `/metrics` route.
- **Verified at:** `src/http/handlers.rs:569-598` (`handle_status` — emits `status`, `version`,
  `active_samples/inputs/voices`, `output_channels`, cache mem/disk only); `src/http/routes.rs:84-89` (status
  routes: `/status`, `/status/samples`, `/status/voices`, `/status/cache`, `/status/inputs`) and `:92` (`/health`)
  — **no `/version`, no `/metrics`**.
- **Severity:** suggestion / observability.
- **Evidence:** `handle_status` already embeds `version` (`handlers.rs:582`) but there is no standalone
  `/version`. No counter for clips/xruns/dropouts exists anywhere in `src/` (grep returned none — they arrive
  from Sprints 5/6). No process uptime is tracked in `main.rs`.
- **Fix:** Add a `start_time: Instant` (or `SystemTime`) captured at daemon start and threaded into `AppState`;
  add `/version` (returns `{name, version, git_sha?}`) and `/metrics` returning the real Sprint-5 xrun/dropout
  counter, the Sprint-6 limiter clip/over counts, per-voice ducking multiplier/state, and uptime. Enrich
  `/status` (or `/status/voices`) with per-voice ducking state. Where a counter is not yet produced by an earlier
  sprint, surface what exists and record the gap in `docs/bugs.md` (never emit a placeholder number).

### F4 — Config: no schema version, silent validation gaps, terse messages
- **Statement:** `Config::validate` checks rate/buffer/channel_volumes/log-level/bass/input/http but has no
  schema-version field and several quiet gaps; messages are minimal.
- **Verified at:** `src/config.rs:683-775` (`validate`); config is **JSON** (`serde_json::from_str` at
  `config.rs:460` in `from_file`); `ducking_rules: Vec<DuckingRule>` declared at `config.rs:422` is **never
  validated** in `validate()`.
- **Severity:** suggestion / robustness.
- **Evidence:** Bass `source_channels` are validated only for alias-resolution (`config.rs:727-731`), not for
  duplicates or for collision with `lfe_channel`. There is no `schema_version`. The ducking-rule numeric checks
  are **Sprint 2's** responsibility (see Out of scope).
- **Fix:** Add an optional `schema_version` field (default to current) and warn on an unknown/newer value; add
  bass source-channel sanity (no duplicate sources, source not equal to the resolved LFE channel) and clearer
  error strings (include the offending value). Hot-reload is **YAGNI** — note it as a possible future, do not
  build it.

### F5 (FEATURE-CONFLICT, low) — Auto voice id `_auto_<millis>` collides within a millisecond
- **Statement:** Two Plays without an explicit voice in the same millisecond get the same `_auto_<millis>` id,
  merging independent sounds into one voice for `voice_stop`/`voice_volume`/ducking purposes.
- **Verified at:** `src/main.rs:659-664` (`voice.unwrap_or_else(|| format!("_auto_{}", SystemTime::now()…as_millis()))`).
- **Severity:** `[low/high]`.
- **Evidence:** Both MQTT and HTTP Plays funnel through this loop; same-ms plays group under one voice, so a
  later `voice_stop`/`voice_fade_out`/`voice_volume` on one affects the other and ducking tracks them together.
- **Fix:** Append a uniqueness suffix — the monotonic `VoiceManager` sample id or a process-global
  `AtomicU64` counter — e.g. `_auto_<millis>_<n>`. **Behavior change** (the emitted voice-id string format
  changes); changelog + partner sign-off (see below).

### F6 (FEATURE-CONFLICT, low) — `seek` clamps to loaded frames while `start_position_ms` uses the total estimate
- **Statement:** Seeking forward in a still-loading stream lands at the loaded edge, inconsistent with
  `start_position_ms`, which intentionally allows positions in the not-yet-loaded region.
- **Verified at:** `src/main.rs:1054-1056` (seek: `target_frame.min(sample.buffer.frames().saturating_sub(1))`)
  vs `:755-762` (start_position: clamps against `buffer.total_frames_or_estimate().unwrap_or(usize::MAX)`).
- **Severity:** `[low/medium]`.
- **Evidence:** `frames()` for a streaming buffer returns only loaded frames, so seek silently lands short; the
  mixer already tolerates unloaded positions by returning silence, which is why `start_position` can use the
  estimate.
- **Fix:** Make seek clamp against `total_frames_or_estimate()` like `start_position_ms`, so the two paths agree.
  **Behavior change** (seek target for streaming buffers); changelog note.

### F7 (FEATURE-CONFLICT, low) — `channel_map` destination colliding with the LFE channel bypasses crossover
- **Statement:** If a Play's `channel_map` routes a source channel to the configured LFE destination, that
  full-range content lands on the sub unfiltered and bass management then *adds* extracted bass on top.
- **Verified at:** `src/audio/mixer.rs:580-582` (bass management runs after all mixing);
  `src/audio/bass_management.rs:216` (`output[lfe_idx] += lfe_sum`, additive); guard `if lfe_ch >= output_channels`
  at `bass_management.rs:188`.
- **Severity:** `[low/medium]`.
- **Evidence:** The LFE index is not in `source_channels`, so user content routed there is never high-passed; it
  sums with the crossover output. There is no validation preventing the collision.
- **Fix:** Document the interaction in `README.md`, and add a **one-time startup warning** when bass management is
  enabled and any input/`channel_map` configuration could route to the LFE index (or at minimum document that
  `channel_map` destinations equal to the LFE channel bypass the crossover). Documentation/validation only — do
  not change the additive mixing behavior here.

### F8 (FEATURE-CONFLICT, low) — `crossfade_ms` silently ignored without `loop:true` or when > half the buffer
- **Statement:** The crossfade blend runs only when `loop_mode && crossfade_samples > 0 && buffer_frames >
  crossfade_samples * 2`; a `crossfade_ms` on a one-shot, or longer than half the clip, is silently dropped.
- **Verified at:** `src/audio/mixer.rs:714`
  (`if loop_mode && sample.crossfade_samples > 0 && buffer_frames > sample.crossfade_samples * 2`).
- **Severity:** `[low/high]`.
- **Evidence:** `crossfade_ms` is parsed independently of `loop` in the command model and the HTTP `PlayParams`
  (`handlers.rs:111-112`), so a user can request it where it does nothing, with no feedback.
- **Fix:** Emit a `tracing::warn!` at command-dispatch time when `crossfade_ms` is set without `loop:true`, or
  when `crossfade_samples * 2 >= buffer length`, and document the loop-boundary scope in `README.md`. No change
  to the blend math (that is Sprint 6's territory).

### F9 (TEST-COVERAGE, low) — No property/fuzz tests for the untrusted config + command JSON surface
- **Statement:** `parse_command`, `expand_macros`, and `config::load` consume arbitrary untrusted input but have
  only fixed-example tests; a panic on malformed input is an undetected DoS.
- **Verified at:** `src/mqtt/commands.rs:418` (`pub fn parse_command`), `:358` (`pub fn expand_macros`);
  `src/config.rs:487` (`pub fn load`) / `:456` (`from_file`, JSON); `Cargo.toml:75-77` dev-deps are only
  `criterion` and `tempfile` — no proptest/quickcheck/arbitrary.
- **Severity:** `[low/high]`.
- **Evidence:** No fuzz/property harness exists; macro precedence/recursion, malformed channel maps, extreme
  numerics, and deeply-nested JSON are only spot-checked.
- **Fix:** Add `proptest` as a dev-dependency and property tests asserting `parse_command`, `expand_macros`, and
  `Config` JSON parsing **never panic** (return `Ok`/`Err`) on arbitrary input, run in the Docker pipeline.

### F10 (TEST-COVERAGE, low) — HTTP handler error paths and malformed-body responses unasserted
- **Statement:** Handlers map send failure to 500 and invalid JSON to 400, but the suite only covers happy-path
  200 and auth 401; no test posts malformed JSON (→400), a body missing a required field (→4xx), or drops
  `cmd_rx` to force the 500 branch.
- **Verified at:** `src/http/handlers.rs:76-80` (400 on invalid JSON in `handle_command`), `:85-88` (500 on send
  failure); `PlayParams.file` is required (`handlers.rs:98`, no `#[serde(default)]`) so a missing `file` is an
  axum rejection.
- **Severity:** `[low/high]`.
- **Evidence:** Per CLAUDE.md, expected error output must be captured and asserted; today it is not.
- **Fix:** Add `http_api_test` cases for: POST `/command` non-JSON → 400 + error shape; POST `/play` and
  `/volume` missing required fields → 4xx; drop `cmd_rx` then POST → 500 with `success:false`.

### F11 (TEST-COVERAGE, low) — WebSocket `handle_socket` and `LogVisitor` formatting untested
- **Statement:** Only `LogBroadcaster` send/subscribe is tested; the welcome frame, broadcast-forward, lag/close
  handling, `WebSocketLogLayer::on_event`, and `LogVisitor` message extraction are untested.
- **Verified at:** `src/http/websocket.rs:58-128` (`handle_socket`: welcome `:62-67`, Lagged branch `:95`, close
  `:99-101`/`:113-115`), `:149-170` (`on_event` format), `:178-194` (`LogVisitor`); existing tests `:200-219`.
- **Severity:** `[low/medium]`.
- **Evidence:** `handle_websocket` is not reached by `http_api_test` (router built with `websocket_enabled=false`,
  `routes.rs:107` is the only `/ws` wiring).
- **Fix:** Unit-test `LogVisitor` by recording synthetic events and asserting the extracted message; unit-test
  `WebSocketLogLayer::on_event` output format; add an axum WebSocket integration test that upgrades, asserts the
  welcome JSON, broadcasts, and asserts the client receives the `{type:"log"}` frame.

### F12 (TEST-COVERAGE, low) — Status handlers asserted structurally, not by value
- **Statement:** `test_samples_endpoint`, `test_voices_endpoint`, `test_cache_status_endpoint`,
  `test_inputs_endpoint` assert only `is_array()`/`is_object()` on empty state; they never populate state and
  check serialized values.
- **Verified at:** `tests/http_api_test.rs:264` (`test_samples_endpoint`, `is_array()` at `:283`), `:287`/`:306`
  (voices), `:310` (cache), `:334`/`:353` (inputs); only `test_samples_endpoint_returns_position_ms` (`:549`)
  populates state and checks computed values.
- **Severity:** `[low/medium]`.
- **Evidence:** `handle_inputs` computes `muted: input.volume == 0.0` (`handlers.rs:683`) and `handle_samples`
  computes `progress_percent` (`handlers.rs:632-636`) — neither is value-asserted on populated state.
- **Fix:** Mirror the `..._returns_position_ms` approach: populate voices/inputs/cache and assert real values
  (`muted` toggles with volume, channels reported, cache entry counts/sizes), not just container type.

### F13 — Packaging: no systemd unit; no structured (JSON) logging option
- **Statement:** There is no systemd service file and no JSON-logging mode for production log aggregation.
- **Verified at:** `src/main.rs:133-165` (logging init: human `fmt` layer only, optional MQTT layer; no JSON);
  no systemd unit in the tree (grep of `README.md`/`docs/` found only the tracker referencing it).
- **Severity:** suggestion / ops.
- **Evidence:** The default subscriber is `tracing_subscriber::fmt()` (human format); MQTT log layer is the only
  alternative sink.
- **Fix:** Add a `mqttaudio.service` systemd unit (documented in `README.md`) and a structured-JSON logging
  option (config `logging.format = "json"` or a CLI flag) using `tracing_subscriber::fmt().json()`; document both.

## Caveats (refuted / over-stated — do not chase ghosts)

- **`MIN_BUFFER_FRAMES`, `mark_playing`, `cleanup_completed_loads` are NOT this sprint's to fix.** They are
  Sprint 4 (streaming/cache). The dump's general cleanup ask mentions them, but the tracker assigns them to
  Sprint 4. Verified they still exist (`streaming.rs:270`; `cache/memory.rs:181`; `cache/mod.rs:292`) and carry
  `#[allow(dead_code)]`. Only confirm the build is clean; if a residual warning there breaks the gate, raise it
  with the partner — do **not** silence or rewire it under this sprint.
- **`ducking_rules.target_volume` finite-∈[0,1] validation is Sprint 2**, not here. This sprint's config work is
  schema versioning, channel-sanity, message clarity, and the four feature-interaction warnings/docs only.
- **F7 (channel_map↔LFE) and F8 (crossfade-without-loop) are documentation/warning items**, not behavior fixes.
  The additive-LFE and loop-boundary-only-crossfade behaviors are intended; do not "fix" the DSP here.
- **`/status` already embeds `version`** (`handlers.rs:582`) — the gap is a *standalone* `/version` route and a
  `/metrics` route, not a missing version string.
- **Limiter/clip and xrun/dropout counters are produced by Sprints 6 and 5.** If they are present, surface them;
  if a prior sprint slipped and they are absent, surface what exists and log the gap — never fabricate a value.

## Tasks (ordered, TDD-first)

Each behavior-affecting task writes the failing test first, confirms it fails, then writes the minimum code.
Cleanup/removal tasks are guarded by the existing test suite + the `-D warnings` gate.

1. **proptest harness (F9) — do this first; it guards everything else.** Add `proptest` to
   `Cargo.toml` `[dev-dependencies]`. Write failing property tests (new file under `tests/`, e.g.
   `tests/fuzz_command_config.rs`) asserting `parse_command(arbitrary_json)`, `expand_macros(arbitrary_msg,
   arbitrary_macros)`, and parsing a `Config` from arbitrary JSON each return `Ok`/`Err` and **never panic**.
   Run, confirm green (or fix any panic discovered — if a panic is a real bug owned by another sprint, write the
   failing case, `#[ignore]` it with a pointer, and log in `docs/bugs.md` per the Charter). Wire the suite into
   `scripts/validate.sh` so Lane A runs it.

2. **HTTP error-path tests (F10).** In `tests/http_api_test.rs`, add failing tests: POST `/command` with a
   non-JSON body asserting `400` + the `CommandResponse::error` shape; POST `/play` and `/volume` with a missing
   required field asserting the 4xx rejection; a test that drops `cmd_rx` and posts a command asserting `500`
   with `success:false`. Confirm they fail against the current router, then confirm they pass (the handlers
   already return these codes — these tests lock the contract).

3. **WebSocket / LogVisitor tests (F11).** Add failing unit tests for `LogVisitor` (record a synthetic
   `message` field and a non-`message` first field; assert the extracted string) and for
   `WebSocketLogLayer::on_event` output format. Add an axum WebSocket integration test (router built with
   `websocket_enabled=true`) that upgrades `/ws`, asserts the welcome `{"type":"connected", … "version":…}`
   JSON, broadcasts via the shared `LogBroadcaster`, and asserts the client receives `{"type":"log","message":…}`.

4. **Real-value status tests (F12).** Mirror `test_samples_endpoint_returns_position_ms`: populate voices,
   inputs (assert `muted` toggles with `volume == 0.0`, channels reported), and cache (assert entry counts/sizes)
   and assert serialized **values**, not just `is_array()`.

5. **Unique auto voice id (F5).** Write a failing test driving two Plays without a `voice` (through
   `handle_command` from Sprint 0's extraction, or the existing dispatch) and asserting the two resulting
   `ActiveSample.voice_id` strings differ even when generated in the same millisecond (inject/stub the counter
   or call the id-builder directly). Implement by appending a process-global `AtomicU64` (or the `VoiceManager`
   sample id) to `_auto_<millis>` at `main.rs:659-664`. **Behavior change — changelog + sign-off.**

6. **Seek/start_position consistency (F6).** Write a failing test on a streaming buffer asserting that a forward
   `seek` past the loaded edge lands at the requested frame (clamped to `total_frames_or_estimate()`), matching
   `start_position_ms`. Change `main.rs:1056` to clamp against `total_frames_or_estimate()` instead of
   `frames()`. **Behavior change — changelog note.**

7. **crossfade-without-loop warning (F8).** Write a failing test (capturing log output, per CLAUDE.md) asserting
   a `tracing::warn!` is emitted when a Play sets `crossfade_ms` without `loop:true`, or when
   `crossfade_samples * 2 >= buffer length`. Emit the warning at dispatch; document in `README.md`.

8. **channel_map↔LFE warning + docs (F7).** Add a one-time startup `tracing::warn!` when bass management is
   enabled and a configured route/`channel_map` destination can equal the LFE channel; document the bypass in
   `README.md`. (Validation/log + docs only.)

9. **Config schema-version + validation gaps (F4).** Write failing tests: parsing a config with a newer
   `schema_version` warns/errs as designed; a config with a duplicate bass `source_channel`, or a source channel
   equal to the resolved LFE channel, is rejected with a clear message including the offending value. Add the
   `schema_version` field and the new `validate()` branches in `config.rs:683-775`. **Do not** touch the
   ducking-rule numeric validation (Sprint 2).

10. **Telemetry enrichment + `/version` + `/metrics` (F3).** Write failing tests asserting `/version` returns
    name+version JSON; `/metrics` returns real fields (uptime > 0; xrun/dropout from Sprint 5; clip/over from
    Sprint 6; per-voice ducking state). Capture `start_time` at daemon start, thread it (and references to the
    Sprint-5/6 counters and the ducking engine snapshot) into `AppState`; add the two routes in `routes.rs`
    and handlers in `handlers.rs`; enrich `/status` (or `/status/voices`) with per-voice ducking state. Where a
    counter is absent (prior sprint slipped), surface what exists and log the gap in `docs/bugs.md` — never a
    placeholder value. Use the Sprint 0 render harness only where a value is audio-derived; the counters here are
    integer telemetry, so assert exact serialized values (e.g. `metrics["uptime_seconds"].as_u64() >= 0`,
    `metrics["clips"].is_number()`).

11. **Structured JSON logging (F13).** Write a failing test (or a documented manual check) that selecting JSON
    logging produces line-delimited JSON records. Add a `logging.format` option (`"text"` default | `"json"`)
    and build a `tracing_subscriber::fmt().json()` layer in `main.rs:133-165` when selected; keep the MQTT layer
    composable. Document in `README.md`.

12. **systemd unit (F13).** Add `packaging/mqttaudio.service` (a `Type=simple` unit running the release binary
    with a config path, `Restart=on-failure`, and a hardening stanza). Document install/enable steps in
    `README.md`. No test (config file); validated by Lane A documentation check + manual install note.

13. **Remove dev scaffolding (F1).** Delete `init_test_sine_wave`, `play_file`, `test_mixer` from `engine.rs`
    (incl. the hardcoded paths) and their now-unused imports; delete the three `Args` fields (`main.rs:61-71`)
    and their three handler blocks (`:205-283`). Run the full suite + `-D warnings`. This must remove code only —
    no behavior the daemon's real command path relies on.

14. **Drop the "Phase 10" banner + de-mask warnings (F2).** Remove `chunked_resampler.rs:4-6` (the comment +
    `#![allow(dead_code)]`). Build `-D warnings`. Remove any item that is now genuinely dead, or apply a narrow,
    accurately-commented `#[allow(dead_code)]` on the specific item — never re-blanket the file. If removing it
    surfaces the file-level blankets in `streaming_decoder.rs:6` / `cache/http_stream.rs:6` and they break the
    gate, address them the same way; otherwise log in `docs/bugs.md`.

15. **Docs + changelog.** Update `README.md` (systemd, JSON logging, `/version`+`/metrics`, the four
    feature-interaction notes, `schema_version`), `CHANGELOG.md` (behavior changes: F5 voice-id format, F6 seek),
    and `docs/bugs.md` (any deferred/surfaced gaps).

## Files to create / touch

- **Create:** `tests/fuzz_command_config.rs` (proptest), `packaging/mqttaudio.service`, new WebSocket/status
  test files or modules as needed.
- **Touch:** `Cargo.toml` (proptest dev-dep), `src/audio/engine.rs` (delete scaffolding),
  `src/main.rs` (remove flags+handlers, unique voice id, seek clamp, crossfade warning, JSON logging, `start_time`
  into `AppState`), `src/audio/chunked_resampler.rs` (drop banner), `src/config.rs` (schema version + validation),
  `src/http/handlers.rs` (`/version`, `/metrics`, enriched status), `src/http/routes.rs` (new routes),
  `tests/http_api_test.rs` (error-path + real-value tests), `src/http/websocket.rs` (tests),
  `scripts/validate.sh` (run proptest suite), `README.md`, `CHANGELOG.md`, `docs/bugs.md`.

## Verification

### Lane A (Docker / Linux) — the only lane this sprint
- `cargo build --release` with `RUSTFLAGS=-D warnings` is clean **with no file-level `allow(dead_code)` banner**
  in `chunked_resampler.rs` (and no new blanket allows introduced).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass.
- The proptest suite (F9) is green in the pipeline (`scripts/validate.sh` runs it).
- New HTTP error-path (F10), WebSocket/LogVisitor (F11), and real-value status (F12) tests pass.
- `/version` and `/metrics` return real data; `/status` shows per-voice ducking state.
- The systemd unit and JSON-logging option are documented in `README.md`.

### Lane B (native macOS real device)
- No new device behavior this sprint; Lane B simply runs the host suite green to confirm the cleanup did not
  regress anything (`scripts/validate.sh --native`).

### Lane C (manual Windows)
- None required. If the systemd unit or JSON-logging doc needs a Windows equivalent note, append it to
  `MANUAL-VERIFICATION.md`; otherwise no Windows steps.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 9)

- [ ] Dev scaffolding removed (hardcoded `/Users/bandrews` paths, `--test-*` flags + their divergent paths); dead constants and the stale `#![allow(dead_code)]` "Phase 10" banner deleted; build clean with no `allow(dead_code)` masking `[A]`
- [ ] Status/telemetry enriched: per-voice ducking state, limiter/clip counts, dropouts, uptime, `/version`, `/metrics` returning real data `[A]`
- [ ] Config schema/versioning + silent-validation-gap fixes; warnings/docs for crossfade-without-loop, seek vs start_position, channel_map↔LFE collision; unique auto voice ids `[A]`
- [ ] proptest fuzz for config + command JSON (no panic) runs in the Docker pipeline `[A]`
- [ ] systemd unit + structured JSON logging documented `[A]`

## Behavior-change / changelog notes

These items change observable behavior — record in `CHANGELOG.md`:
- **F5 — auto voice-id format changes** from `_auto_<millis>` to `_auto_<millis>_<n>`. Any client or script
  relying on the exact auto-id string, or on same-ms plays sharing a voice, is affected. **Requires partner
  sign-off** before implementing (it is a user-visible string contract).
- **F6 — `seek` on a streaming buffer** now clamps to `total_frames_or_estimate()` rather than the loaded-edge,
  so forward seeks into a still-loading region land at the requested time (silence until loaded) instead of the
  last loaded frame. Changelog note.
- **F8 / F7 — new warnings** when `crossfade_ms` is set without `loop:true` (or exceeds half the buffer), and on
  the bass-management LFE/`channel_map` collision. Log-output change only; document in `README.md`.
- **F3 — new `/version` and `/metrics` routes** and enriched `/status` payload (additive fields). Document.
- **F4 — `schema_version`** config field (optional, defaulted). Document.
- **F13 — JSON logging option** and systemd unit. Document.

The scaffolding removal (F1), banner removal (F2), and the new tests (F9–F12) are **not** behavior changes —
they must be invisible to the running daemon's real command path.

## Definition of Done

Lane A green (build `-D warnings` with **no** `chunked_resampler` `allow(dead_code)` banner and no new blanket
allows · clippy · fmt · full test suite · proptest suite · new HTTP/WebSocket/status tests) · Lane B green
(host suite, no regression) · `/version` + `/metrics` return real data · systemd unit + JSON logging documented
in `README.md` · behavior changes (F5, F6) in `CHANGELOG.md` with F5 partner sign-off obtained · out-of-scope/
surfaced gaps logged in `docs/bugs.md` · committed atomically to the branch as units complete ·
`cargo build --release` warning-free.
