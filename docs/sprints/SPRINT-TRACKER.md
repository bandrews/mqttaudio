# mqttaudio Quality Sprint Tracker

This is the control document for the quality-improvement program. It sequences the work from the
senior code review into ten sprints, tracks their status, and holds the acceptance criteria for each.
It is meant to be driven by `/goal "complete all sprints and finish the sprint tracker"`.

Each sprint has its own self-contained file in this directory (`sprint-NN-*.md`) with the full detail:
findings, evidence, fixes, an ordered TDD task list, files to touch, and a verification plan. Work the
sprints **in order**; do not start a sprint whose dependencies are not `Done`.

---

## ⚠️ Charter — read before every sprint. No shortcuts.

These rules are non-negotiable and override any urge to move fast. They mirror `CLAUDE.md`, especially
its MOST IMPORTANT NOTE.

**Decisions are pre-locked in [`DECISIONS.md`](DECISIONS.md).** It resolves every design/behavior choice in
this program; any "sign-off", "partner", "decide", or "consult" wording left in a sprint file is **superseded**
by it — treat those as already decided and keep moving. You may **overrule** a locked decision only when
implementation uncovers new evidence that a different choice is clearly better (record it per `DECISIONS.md`).
**Never block the program waiting on a human.** If something truly needs human intervention and has no other
resolution — a genuine last resort — add it to [`NEEDS-HUMAN.md`](NEEDS-HUMAN.md), skip **only** that item, and
complete everything else in the sprint.

- **Doing it right beats doing it fast.** You are not in a rush. Tedious, systematic work is usually the
  correct solution. Never skip steps.
- **Never fake completion.** The worst thing you can do is declare a sprint finished when it is not.
  Do not mark an acceptance box checked unless you have *actually verified* it.
- **Never disable, delete, `#[ignore]`, or comment out a test or code path to make things "pass."**
  If a test fails, fix the root cause. If you believe a test is wrong, don't silently weaken it — log it in
  `NEEDS-HUMAN.md` and move on. (Gating device-opening tests behind the documented `--ignored` flag for Lane B is
  the *only* sanctioned use of `#[ignore]`, and it is set up once in Sprint 0.)
- **Root cause only.** No symptom patches or workarounds. Follow the systematic debugging process in
  `CLAUDE.md`: investigate → reproduce → single hypothesis → minimal change → verify.
- **TDD.** For every behavior change: write the failing test first, confirm it fails, write the minimum
  code to pass, confirm it passes, refactor green.
- **Smallest reasonable change.** Match surrounding style. Do not rewrite implementations without explicit
  permission (this matters most in Sprint 5).
- **Green gate, every sprint.** A sprint is not `Done` until `scripts/validate.sh` (Lane A, Docker) and
  `scripts/validate.sh --native` (Lane B, this Mac) both exit 0, with **zero build warnings** and clippy
  clean. `cargo build --release` building without warnings is a hard rule.
- **Don't halt the program.** Resolve blockers from `DECISIONS.md` and the code; override a locked decision if
  new evidence demands it. Only as a true last resort — when something genuinely needs a human and has no other
  path — log it in `NEEDS-HUMAN.md`, skip that one item, and finish everything else in the sprint.
- **Log out-of-scope discoveries** in `docs/bugs.md`; commit each
  sprint on a branch with a clear message.

If you cannot honestly check every box for a sprint, leave it `In progress` or `Blocked` and explain why.
A half-done sprint marked `Done` is a failure of the whole program.

---

## How to run this program

1. Pick the lowest-numbered sprint that is `Not started` and whose dependencies are all `Done`.
2. Open its `sprint-NN-*.md`, set its status here to `In progress`.
3. Implement it (TDD), using subagents where the sprint file says it helps.
4. Run Lane A (`scripts/validate.sh`) and Lane B (`scripts/validate.sh --native`). Both must be green.
5. Append any Windows steps the sprint produced to `MANUAL-VERIFICATION.md`.
6. Tick every acceptance box below for that sprint. Commit your work atomically to the branch with a clear message.
7. Set the sprint status to `Done`. Go to step 1.
8. When all sprints are `Done`, complete the **Final gates** at the bottom.

### Validation lanes
- **Lane A — Docker (Linux):** `scripts/validate.sh` → build `-D warnings`, clippy `-D warnings`,
  `fmt --check`, tests, render-harness, broker tests vs containerized mosquitto. No real audio device.
- **Lane B — Native macOS (this machine):** `scripts/validate.sh --native` → host build/lint/tests **plus**
  device-opening smoke tests on the real default CoreAudio device.
- **Lane C — Manual Windows (partner):** steps accumulated in `MANUAL-VERIFICATION.md`, run once at the end.

---

## Status board

| # | Sprint | Status | Depends on | File |
|---|--------|--------|-----------|------|
| 0 | Validation harness & Docker pipeline | Done | — | [sprint-00](sprint-00-validation-harness.md) |
| 1 | Device & format compatibility | Done | 0 | [sprint-01](sprint-01-device-format-compatibility.md) |
| 2 | Control-plane reliability | Done | 0 | [sprint-02](sprint-02-control-plane-reliability.md) |
| 3 | Security & file safety | Done | 0 | [sprint-03](sprint-03-security-and-file-safety.md) |
| 4 | Streaming & cache correctness | In progress | 0 | [sprint-04](sprint-04-streaming-and-cache-correctness.md) |
| 5 | Lock-free real-time engine | Not started | 0 | [sprint-05](sprint-05-lockfree-realtime-engine.md) |
| 6 | Mixer DSP correctness | Not started | 5 | [sprint-06](sprint-06-mixer-dsp-correctness.md) |
| 7 | Bass management & multichannel | Not started | 5 | [sprint-07](sprint-07-bass-management-and-multichannel.md) |
| 8 | Live input robustness | Not started | 5 | [sprint-08](sprint-08-live-input-robustness.md) |
| 9 | Cleanup, observability, packaging | Not started | 1–8 | [sprint-09](sprint-09-cleanup-observability-packaging.md) |

Status values: `Not started` · `In progress` · `Blocked` · `Done`.

---

## Acceptance criteria

Tick a box only when genuinely verified. `[A]` = Lane A/Docker, `[B]` = Lane B/native macOS,
`[C]` = Lane C/manual Windows.

### Sprint 0 — Validation harness & Docker pipeline
- [x] `docker/validate.Dockerfile` + `scripts/validate.sh` exist; Lane A runs build `-D warnings`, clippy, `fmt --check`, tests, render-harness, broker tests, and exits non-zero on any failure `[A]`
- [x] `scripts/validate.sh --native` runs the host suite incl. the real-device smoke test and is green on this Mac `[B]`
- [x] Offline render harness exists (pumps buffers through `mix_audio`, concatenates output) with RMS/peak/FFT-band/inter-sample-delta assert helpers, used by ≥2 new cross-feature tests `[A]`
- [x] Command dispatch extracted to a testable `async fn handle_command(...)`; tests assert Play/Stop/VoiceVolume effects on `mixer_state`/voice/ducking state `[A]`
- [x] Live-broker MQTT tests run in Docker (with mosquitto) and are skipped in a bare `cargo test`; event-translation refactored into a pure tested fn `[A]`
- [x] Device-opening tests gated behind `--ignored`/env flag: run in Lane B, skipped in Lane A `[A][B]`

### Sprint 1 — Device & format compatibility
- [x] Pure-helper unit tests for sample-format / rate / channel selection pass; a forced-i16 path proves no panic `[A]`
- [x] Output builds a typed stream matching the device's native format (I16/U16/I32/F32) with an f32 mix bus + convert shim; `.expect` replaced with graceful error/fallback `[A]`
- [x] `find_output_config` filters by `sample_format` and returns it; nearest-supported discrete rate chosen; channel-count fallback (next-larger + zero-fill) `[A]`
- [x] `audio.buffer_size` honored via `BufferSize::Fixed` within device range `[A]`
- [x] Device-error callback rebuilds the stream with backoff `[A]`
- [x] `--list-devices` + play smoke runs on the real CoreAudio device via the format dispatch `[B]`
- [ ] WASAPI shared (i16/i32) `--list-devices` + play smoke documented and run `[C]` — documented (W-1); pending the final partner Windows pass

### Sprint 2 — Control-plane reliability
- [x] Re-subscribe on `Packet::ConnAck`; integration test proves commands arrive after a broker restart `[A]`
- [x] Cache guard dropped before `.await`; `decode_file` runs in `spawn_blocking` `[A]`
- [x] RT-shared state no longer poison-bricks audio (non-poisoning or PoisonError-recovering locks); test proves a poisoned non-RT lock doesn't kill the callback path `[A]`
- [x] SIGINT/SIGTERM handler fades active samples, drains, flushes cache metadata; test verifies clean shutdown `[A]`
- [x] Burst >100 commands does not stall `eventloop.poll()` `[A]`
- [x] `total_cmp` replaces `partial_cmp().unwrap()`; `ducking_rules.target_volume` validated finite ∈[0,1]; `resolve_*` exits gracefully instead of `expect` `[A]`

### Sprint 3 — Security & file safety
- [x] `allowed_directories` enforced **when configured** (canonicalize + reject); empty list = allow-all preserved with a startup warning; `/etc/passwd` refused under a configured allowlist; tests cover both `[A]`
- [x] **Opt-in** MQTT TLS (`mqtt.tls`); default transport stays plain TCP (incl. 8883); TLS connect verified against mosquitto+TLS; plain connect still works `[A]`
- [x] Open HTTP mode preserved by default; opt-in `http.require_auth` enforces (incl. status/ws); loud non-fatal warning on non-loopback bind without auth; constant-time token compare; `?token=` convenience kept `[A]`
- [x] Disk-cache writes are temp-file+rename with size verify on load; kill-mid-write leaves no "valid" truncated file; stable content hash replaces `DefaultHasher` `[A]`
- [x] HTTP revalidation implemented (If-None-Match/If-Modified-Since) **or** dead `revalidate_after_seconds` knob removed `[A]`

### Sprint 4 — Streaming & cache correctness
- [ ] Looping a still-streaming buffer no longer wraps the growing loaded length (loop deferred until complete / wraps on total estimate); render-harness asserts no tight-loop buzz on a fake incrementally-filled buffer `[A]`
- [ ] Completed streams promoted to memory cache; replaying a finished URL hits the cache, not a stale streaming buffer; `active_loads` bounded `[A]`
- [ ] `mark_playing` eviction protection wired; size accounting fixed; bounded cache stays under `max_memory_mb` with a playing buffer `[A]`
- [ ] `MIN_BUFFER_FRAMES` prebuffer enforced or removed (no misleading dead code) `[A]`

### Sprint 5 — Lock-free real-time engine
- [ ] No locks, allocations, or frees in the callback path — verified by code audit **and** an allocation-counting harness around `mix_audio`/the callback shim `[A]`
- [ ] Control→audio handoff via SPSC command ring; audio thread owns `MixerState`; pre-allocated voice pool; graveyard reaper drops finished payloads off-RT; status via snapshot for HTTP `[A]`
- [ ] Ducking notify + voice bookkeeping moved off the RT thread; pitch scratch pre-allocated `[A]`
- [ ] Render harness shows within-tolerance output vs pre-redesign for a fixed scene; soak test (many plays/stops) shows no xrun-counter increments `[A]`
- [ ] Real-device soak smoke runs clean on this Mac `[B]`

### Sprint 6 — Mixer DSP correctness
- [ ] NaN/non-finite input → silence, not NaN, at the output; clip/over counter exposed `[A]`
- [ ] Reverse interpolation uses `frame_n*(1-frac)+frame_{n+1}*frac`; -0.5x ramp test passes `[A]`
- [ ] Per-channel calibration (`channel_volumes`) applied as a final output gain stage; test asserts per-channel gain `[A]`
- [ ] Play `volume` clamped to [0,1] at construction `[A]`
- [ ] Ducking advanced once per buffer, applied per-frame (smooth), restore honors `fade_duration_ms`, live-input voices can trigger ducking; ducked-voice RMS follows configured fade consistently across overlapping samples `[A]`
- [ ] Pitch correction pre-rolled on enable (no silent gap); position-advance matches frames consumed; tail flushed `[A]`
- [ ] Equal-power loop crossfade with correct overlap-on-wrap; no inter-sample click at the loop point `[A]`
- [ ] True-peak/soft-knee limiter with configurable ceiling replaces the bare hard clamp; peak ≤ ceiling `[A]`

### Sprint 7 — Bass management & multichannel
- [ ] 4th-order Linkwitz-Riley crossover; render-harness shows flat-ish acoustic-sum magnitude through the crossover `[A]`
- [ ] LFE gain compensation makes sub level independent of active source count `[A]`
- [ ] `remove_bass_from_sources` default decided + documented; one-time warning when `lfe_channel >= output_channels`; LFE-collision documented `[A]`
- [ ] Denormal flush in the IIR; integration test exercises bass management through `mix_audio` `[A]`

### Sprint 8 — Live input robustness
- [ ] No allocation in the capture callback (allocation harness); `process_into_buffer` + reusable buffers `[A]`
- [ ] Adaptive/async SRC steered by ring-buffer fill (even at equal nominal rates); drift simulation keeps the ring buffer bounded `[A]`
- [ ] Underrun applies a short fade/hold (no hard cut) and keeps the ramp time-accurate `[A]`
- [ ] `voice_volume` reaches input-only voices; `input_mute` restores the prior volume (not hardcoded 1.0); route source channels validated vs device channels `[A]`
- [ ] Non-f32 input format supported (convert to f32) `[A]`
- [ ] CoreAudio input device opens and routes into the mix (smoke) `[B]`
- [ ] ≥30-min two-device (USB mic + separate output) soak shows no periodic dropouts; non-f32 input opens `[C]`

### Sprint 9 — Cleanup, observability, packaging
- [ ] Dev scaffolding removed (hardcoded `/Users/bandrews` paths, `--test-*` flags + their divergent paths); dead constants and the stale `#![allow(dead_code)]` "Phase 10" banner deleted; build clean with no `allow(dead_code)` masking `[A]`
- [ ] Status/telemetry enriched: per-voice ducking state, limiter/clip counts, dropouts, uptime, `/version`, `/metrics` returning real data `[A]`
- [ ] Config schema/versioning + silent-validation-gap fixes; warnings/docs for crossfade-without-loop, seek vs start_position, channel_map↔LFE collision; unique auto voice ids `[A]`
- [ ] proptest fuzz for config + command JSON (no panic) runs in the Docker pipeline `[A]`
- [ ] systemd unit + structured JSON logging documented `[A]`

---

## Final gates

- [ ] All sprints 0–9 are `Done` with every box above genuinely checked
- [ ] `MANUAL-VERIFICATION.md` has been run by the partner on Windows (and spot-checked on macOS) with results recorded
- [ ] `README.md` / `CHANGELOG.md` updated for user-visible and behavior-changing items
- [ ] `docs/bugs.md` reflects any out-of-scope items discovered along the way
- [ ] This tracker is finished: statuses accurate, no half-truths

## Global Definition of Done (applies to every sprint)

Lane A green · Lane B green (incl. real-device smoke where the sprint touches device/format/input) ·
new tests + harness assertions added · Windows steps appended to `MANUAL-VERIFICATION.md` where relevant ·
out-of-scope items logged to `docs/bugs.md` · committed on a branch ·
`cargo build --release` warning-free.
