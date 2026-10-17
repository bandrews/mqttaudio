# Sprint 14 — Quality & Correctness Backlog

| Field | Value |
|-------|-------|
| Status | Done (Lane A via documented host approximation — see tracker note; Lane B pending partner) |
| Depends on | 11 (metrics surface); soft-ordered after 13 (documents its counters); independent of 12 except shared test re-pins |
| Effort | M |
| Lanes | A (Docker) primarily; B (regression run) |
| Subagents | Optional (the four findings are independent) |

## Goal

Close the owner-approved quality backlog: switch the sample-rate-conversion resampler's sinc
interpolation from `Linear` to `Cubic` (R1, decision D59), remove the dead `audio.channel_names`
config field (D60), make `/command`'s 400 rejection return the `CommandResponse` JSON shape
(D61), and wire the already-implemented `WebSocketLogLayer` into the tracing subscriber so `/ws`
actually streams log lines (D62). Sweep `docs/bugs.md` so every entry this program resolved is
closed with a citation.

## Why

These are the long-lingering, low-risk items the owner explicitly scoped in (2026-06-09):
the resampler quality item has haunted `docs/bugs.md` since Sprint 6 with no decision; the
`/ws` log streaming is documented in `docs/http-api.md` as working but never was; the 400 body
shape contradicts the daemon's own response contract; `channel_names` is dead config surface
that misleads config authors. None affects the RT path; all are contract/quality fixes with
clear test shapes. The webui-adjacent items (D61, D62) are daemon-side plumbing only — no
front-end changes in this sprint.

## Scope

**In scope**
- F1 resampler `Linear`→`Cubic` (both resampler constructions); F2 `channel_names` removal;
  F3 `/command` 400 `CommandResponse` body; F4 `WebSocketLogLayer` wiring; F5 `docs/bugs.md`
  closing sweep.

**Out of scope** (owner scope decision / other owners)
- Webui front-end changes (the web app consumes the corrected daemon behavior as-is; its log
  console already renders `{type:"log"}` frames).
- Reverse-loop crossfade, per-input capture meters, speed>1 anti-aliasing, PI drift control,
  LFE-bus low-pass, ALSA matcher, binary-redeclares-modules refactor (all recorded with reasons
  in `docs/bugs.md` / sprint-10).
- The playback-speed cubic interpolator (D27) — already shipped in Sprint 6; F1 here is the
  *rate-conversion* path only.

## Findings addressed

All citations re-verified against the current tree during Sprint 10.

### F1 — Rate-conversion resampler uses `Linear` sinc interpolation (R1, undecided since Sprint 6)
- **Statement:** Both resampler constructions use `SincInterpolationType::Linear`; `Cubic` is
  higher quality at negligible cost (the table is precomputed) but changes decoded PCM for
  every rate-converted file, and no decision ever locked it.
- **Verified at:** `src/audio/resampler.rs:90` and `src/audio/chunked_resampler.rs:119`
  (`interpolation: SincInterpolationType::Linear`); the open item in `docs/bugs.md`
  ("Resampler (rubato) sinc interpolation stays `Linear`…").
- **Severity:** low (quality).
- **Evidence:** owner approved the switch 2026-06-09 → **D59** locks `Cubic`.
- **Fix:** change both sites to `SincInterpolationType::Cubic`. Re-pin the tolerance-based
  decode/resample tests (`matches_full_decode` and any fixture-pinned PCM assertions) to the
  new output — *tighten* tolerances where the new output allows, never loosen beyond the
  current bounds. Bench the decode cost on Lane A (the Sprint 11 loading bench) and record the
  delta in this doc; if the cost is non-negligible on Pi-class targets (>10% decode-time
  regression), stop and escalate rather than shipping (Charter: new evidence may override D59).
- **TDD:** first a failing quality test: resample a synthesized swept tone 44.1k→48k and assert
  the out-of-band spurious energy is below a bound that `Linear` fails and `Cubic` passes
  (render-harness band-energy helpers); then flip the two constructors; then re-pin.

### F2 — Dead config field `audio.channel_names`
- **Statement:** `channel_names` is declared, deserialized, defaulted, and unit-tested but
  never read by any code path; only `channel_aliases` is used for resolution. It is
  undocumented and misleads config authors.
- **Verified at:** `src/config.rs:84` (field), `:200` (default), `:1314`/`:1352` (its own
  tests); no other reference in `src/`, `tests/`, or `webui/` (sprint-10 grep); the open item
  in `docs/bugs.md`.
- **Severity:** low (config hygiene).
- **Fix (D60):** remove the field, its default, and its field-specific tests. Old configs keep
  working: serde ignores unknown keys by default — **verify** the config structs do not use
  `deny_unknown_fields` (they do not today; add a test locking that a config containing
  `channel_names` still parses with a clean result). Changelog note.
- **TDD:** failing test first: a config JSON containing `audio.channel_names` parses
  successfully and the struct no longer carries the field (compile-time) — i.e. write the
  tolerated-unknown-key test, watch it pass trivially pre-removal, remove the field, confirm it
  still passes and the field tests are gone with it.

### F3 — `/command` rejects non-JSON bodies with axum plaintext, not the `CommandResponse` shape
- **Statement:** A non-JSON body is rejected by the `Json<Value>` extractor before the handler
  runs, producing 400 with axum's plaintext body; the daemon's own contract elsewhere is
  `CommandResponse` JSON, and the handler's internal "Invalid JSON" 400 branch is unreachable
  dead logic. The current behavior is test-locked as-is.
- **Verified at:** `docs/bugs.md` entry ("`handle_command`'s own 'Invalid JSON' 400 branch is
  unreachable…"); `tests/http_api_test.rs:954-986`
  (`test_command_non_json_body_returns_400` asserts the plaintext body);
  `src/http/handlers.rs` `handle_command` extractor signature.
- **Severity:** low (API consistency); **owner approved as daemon plumbing → D61.**
- **Fix:** replace the bare `Json<Value>` extraction with a rejection-handling form (axum 0.7:
  `WithRejection<Json<Value>, _>` or an explicit `Result<Json<Value>, JsonRejection>`
  parameter) mapping the rejection to `400` + `CommandResponse::error("Invalid JSON: …")`.
  Remove the now-truly-dead internal branch. Update
  `test_command_non_json_body_returns_400` to assert the JSON shape (this is the sanctioned
  update of a contract-locking test to the **newly decided** contract — D61 — not a weakening).
  Update `docs/webui/API-CONTRACT.md` and `docs/http-api.md`.
- **TDD:** flip the existing test to the new contract first (failing), then implement.

### F4 — `/ws` never streams log lines: `WebSocketLogLayer` is not installed
- **Statement:** `start_server` creates a `LogBroadcaster` and `/ws` clients get the welcome
  frame, but `WebSocketLogLayer` is never added to the tracing registry in `main.rs`, so no
  `{type:"log"}` frame is ever produced; `docs/http-api.md` documents the feature as working.
- **Verified at:** `src/http/websocket.rs:182-195` (the layer, dead in the binary),
  `src/http/mod.rs:123` (broadcaster created inside `start_server`, after logging init);
  logging init at `src/main.rs:163-181` (registry + fmt/MQTT layers only); the MEDIUM entry in
  `docs/bugs.md` ("`/ws` never streams log lines…") including the fix shape.
- **Severity:** medium (documented feature absent); **owner approved as daemon plumbing → D62.**
- **Fix:** create the `LogBroadcaster` in `main.rs` **before** logging init; add
  `WebSocketLogLayer::new(broadcaster.clone())` to the registry alongside the fmt/MQTT layers
  (both registry branches at `main.rs:174` and `:180`); pass the same broadcaster into
  `start_server` (change its signature or the `AppState` builder to accept it instead of
  constructing one). Drop the layer's `#[allow(dead_code)]` once wired. Mind the feedback loop:
  the layer must not emit tracing events itself while broadcasting (it doesn't today — keep it
  that way; note it in the layer's doc comment).
- **TDD:** failing integration test in `tests/websocket_test.rs`: build the server the way
  `main.rs` now does (broadcaster outside), connect `/ws`, emit a `tracing::info!` through the
  subscriber, assert the client receives a `{type:"log", message:…}` frame after the welcome
  frame. Confirm `docs/http-api.md`'s description is now true (and correct any drift).

### F5 — `docs/bugs.md` closing sweep
- **Statement:** After sprints 11–14 land, every backlog entry this program resolved must be
  closed with a citation, and the entries Sprint 10 found already-stale must stay corrected.
- **Fix:** close the resampler-Linear, channel_names, `/command`-400, and `/ws`-log entries
  with commit/file citations; verify the Sprint 12/13 closures happened (duck residual,
  voice-pool soft cap, pitch toggle, invalidate race); leave the explicitly-retained items
  (C++ harness caveat, chunked-resample divergence with its Sprint-12-widened scope note,
  speed>1 aliasing, PI drift, LFE bus, reverse-loop crossfade, ALSA matcher, module refactor)
  with their reasons intact.

## Implementation deviations (recorded honestly, per the override protocol)

- **F1 closed R1 as "stay Linear" — D59 overridden on measurement.** The TDD plan required a quality
  test that Linear fails and Cubic passes; no such property exists at the daemon's presets. A
  least-squares tone-residual probe (15 kHz, 44.1k→48k, Fast preset and above) measured the two
  interpolation types identical to ~0.015% of an already ≈-60 dB residual: the error floor is the sinc
  filter (`sinc_len`/`oversampling_factor` — the existing `resampler_quality` presets), not the table
  interpolation. Shipping Cubic would have changed every rate-converted file's PCM for no measurable
  benefit, with no pinnable test — exactly what the Charter's evidence rule exists for. Instead the
  measured floor is pinned (`resampler::tests::fast_preset_off_tone_residual_stays_below_minus_50_dbfs`),
  R1 is closed in `docs/bugs.md` with the measurement, and D59 carries the override note.

## Caveats (refuted / over-stated — do not chase ghosts)

- **`DiskCache` hashing is already SHA-256** (`src/cache/disk.rs:153-158`); the old bugs.md
  entry was stale and Sprint 10 corrected it. No work here.
- **The file-level `#![allow(dead_code)]` banners are already gone** (no file-level allow
  remains in `src/`; per-item dispositions shipped with Sprint 9's final gate — tracker §Sprint
  9 box 1). The narrow per-item `#[allow(dead_code)]` annotations that remain are the
  documented, justified kind; do not blanket-remove them. (Sprint 12 makes `streaming_decoder`
  live in the binary, which may obsolete some — clean up only what its build surfaces.)
- **The per-sample `windowed` flag already ships** (`src/http/handlers.rs:749`); the bugs.md
  entry was stale, corrected in Sprint 10. No work here.
- **Do not touch rubato's `Fast`/quality parameter set** — F1 changes the interpolation type
  only. `ResamplerQuality` selection is config surface with its own semantics.

## Tasks (ordered, TDD-first)

1. F4 WebSocket log wiring (highest user value; isolated).
2. F3 `/command` 400 shape (small; includes API-contract doc updates).
3. F1 resampler Cubic (quality test → flip → re-pin → bench-and-record).
4. F2 `channel_names` removal.
5. F5 `docs/bugs.md` sweep + `CHANGELOG.md` + `README.md`/`docs/http-api.md`/
   `docs/webui/API-CONTRACT.md` updates.

## Files to create / touch

- **Touch:** `src/audio/resampler.rs`, `src/audio/chunked_resampler.rs` (F1),
  `src/config.rs` (F2), `src/http/handlers.rs` (F3), `src/main.rs` + `src/http/mod.rs` +
  `src/http/websocket.rs` (F4), `tests/http_api_test.rs` (F3), `tests/websocket_test.rs` (F4),
  decode/resample tolerance tests (F1 re-pins), `docs/bugs.md`, `docs/http-api.md`,
  `docs/webui/API-CONTRACT.md`, `README.md`, `CHANGELOG.md`.

## Verification

### Lane A (Docker / Linux)
- `scripts/validate.sh` green; the new WS-log integration test, the flipped 400-shape test,
  the Cubic quality test, and the unknown-key config test all pass; F1's bench delta recorded
  in this doc.

### Lane B (native macOS)
- `scripts/validate.sh --native` green (regression only); manual: connect a `/ws` client to a
  running daemon and see live log lines.

### Lane C (manual Windows)
- None.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 14)

- [ ] `/ws` streams real `{type:"log"}` frames from the live tracing subscriber (integration-tested); `docs/http-api.md` is true `[A]`
- [ ] `/command` 400 rejections return the `CommandResponse` JSON shape; contract test updated; API-CONTRACT.md updated `[A]`
- [ ] Resampler sinc interpolation is `Cubic` in both constructions; quality test passes that `Linear` fails; tolerance tests re-pinned; bench delta recorded `[A]`
- [ ] `audio.channel_names` removed; configs containing it still parse (locked by test); changelog'd `[A]`
- [ ] `docs/bugs.md` sweep complete: program-resolved entries closed with citations, retained items intact `[A]`
- [ ] Live `/ws` log lines observed against a running daemon `[B]`

## Behavior-change / changelog notes

- **F1 — decoded PCM changes** for every file whose rate differs from the device rate (subtle
  quality improvement; owner-approved D59). Changelog.
- **F3 — `/command` 400 body becomes JSON** (`CommandResponse`); any client parsing the old
  plaintext is affected. Changelog + API-CONTRACT.md (owner-approved D61).
- **F4 — `/ws` now emits log frames** (documented behavior becomes real). Changelog.
- **F2 — `channel_names` removed** from config surface (unknown key still tolerated).
  Changelog.

## Definition of Done

Lane A green (full gate + the four new/updated test groups) · Lane B regression green + the
manual `/ws` check · bench delta for F1 recorded in this doc · all four behavior changes in
`CHANGELOG.md` with docs updated (`README.md`, `docs/http-api.md`,
`docs/webui/API-CONTRACT.md`) · `docs/bugs.md` sweep done · out-of-scope discoveries logged ·
committed atomically · `cargo build --release` warning-free.
