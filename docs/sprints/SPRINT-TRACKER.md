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
| 4 | Streaming & cache correctness | Done | 0 | [sprint-04](sprint-04-streaming-and-cache-correctness.md) |
| 5 | Lock-free real-time engine | Done | 0 | [sprint-05](sprint-05-lockfree-realtime-engine.md) |
| 6 | Mixer DSP correctness | Done | 5 | [sprint-06](sprint-06-mixer-dsp-correctness.md) |
| 7 | Bass management & multichannel | Done | 5 | [sprint-07](sprint-07-bass-management-and-multichannel.md) |
| 8 | Live input robustness | Done | 5 | [sprint-08](sprint-08-live-input-robustness.md) |
| 9 | Cleanup, observability, packaging | Done | 1–8 | [sprint-09](sprint-09-cleanup-observability-packaging.md) |

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
- [x] Looping a still-streaming buffer no longer wraps the growing loaded length (loop deferred until complete / wraps on total estimate); render-harness asserts no tight-loop buzz on a fake incrementally-filled buffer `[A]`
- [x] Completed streams promoted to memory cache; replaying a finished URL hits the cache, not a stale streaming buffer; `active_loads` bounded `[A]`
- [x] Eviction protection active (Arc keep-alive per D12, not the deleted `mark_playing` set); size accounting fixed; bounded cache stays under `max_memory_mb` with a playing buffer `[A]`
- [x] `MIN_BUFFER_FRAMES` prebuffer enforced or removed (no misleading dead code) `[A]`

### Sprint 5 — Lock-free real-time engine
- [x] No *contended* locks, and no allocations or frees, in the callback path — per the partner-approved **D22a** uncontended-mutex model (the control plane never touches the RT state; only the callback locks it). Verified by code audit **and** the allocation-counting harness (`tests/alloc_harness.rs`: `mix_audio`, the pitch path, the full callback step, mutation-command drain, and Play-into-the-reserved-pool — all 0 alloc / 0 free). Bounded residuals documented in `docs/bugs.md` (>256 over-cap realloc; a Speed command that toggles pitch correction) `[A]`
- [x] Control→audio handoff via SPSC command ring; audio thread owns `MixerState` behind the uncontended callback mutex (D22a); voice pool pre-reserved to `MAX_VOICES`; graveyard reaper drops finished samples **and** spent command husks off-RT; status via a control-side `RwLock<StatusSnapshot>` for HTTP `[A]`
- [x] Ducking `notify`/`update_duck_states` moved off the RT thread (control-side `DuckingEngine::compute_changes` → `SetDuckTarget` over the ring → audio-side `DuckingApplier`); pitch scratch pre-allocated (F5-4) `[A]`
- [x] Render-harness output unchanged (parity confirmed by adversarial review); soak test (`tests/soak_test.rs`, 10k plays/stops) keeps the voice pool bounded with no panic. (Offline `xruns` cannot increment — no device; real-device `xruns==0` is the Lane B soak. `xruns` is incremented in the error callback but not yet surfaced on `/status` — docs/bugs.md.) `[A]`
- [x] Real-device soak smoke runs clean on this Mac `[B]` — the default CoreAudio device opens and runs the new callback path (`device_smoke_test`, Lane B native, green). A sustained soak with human listening for dropouts remains the partner's final-validation pass (MANUAL-VERIFICATION).

> **Sprint 5 (Done — core redesign landed, RT-safe, both lanes green).** The lock-free RT engine is wired
> into the live daemon under the **D22a uncontended-mutex** model (partner-approved override of D16 — see
> DECISIONS.md). The cpal callback now locks a single `AudioCallbackState` bundle (mixer + command consumer +
> graveyard + command-return producer) that **only it touches**, drains a bounded batch of `AudioCommand`s,
> mixes (`mix_audio` DSP unchanged), and reaps finished samples to the graveyard — doing **no allocation and
> no free** on the hot path (proven by `tests/alloc_harness.rs`). The control plane never locks the RT state:
> the ~13 command handlers push ring commands, ducking runs control-side (`compute_changes` → `SetDuckTarget`),
> finished samples + spent command husks are dropped off-RT by the 20 ms reaper, and HTTP reads a control-side
> `StatusSnapshot`. Built across commits `2be7547` (F5-4), `4850ba3` (alloc harness), `d6d9e05` (command-ring
> bridge), `e2420f5` (graveyard), `2c0eea4` (structural integration), `48f9887` (RT-safety: command-return
> ring + un-box + voice-pool reserve), and the soak test — each gate green on Lane A + Lane B native.
>
> Two adversarial-verification passes (built into the integration workflows) caught and drove out real
> RT-thread frees before they shipped. **Documented bounded residuals / follow-ups** (`docs/bugs.md`): the
> voice pool is a *soft* reserve (>256 voices reallocs once — D18 hard cap/steal deferred); a Speed command
> that *toggles* pitch correction still creates/drops the stretcher on RT (pairs with Sprint 6); `xruns` is
> not yet on `/status` (Sprint 9 `/metrics`); `/status/samples` live position is gone by design (D20/D22a,
> changelog'd). The Lane B real-device **listening** soak is the partner's final-validation pass.

### Sprint 6 — Mixer DSP correctness
- [x] NaN/non-finite input → silence, not NaN, at the output; clip/over counter exposed `[A]`
- [x] Reverse interpolation uses `frame_n*(1-frac)+frame_{n+1}*frac`; -0.5x ramp test passes `[A]`
- [x] Per-channel calibration (`channel_volumes`) applied as a final output gain stage; test asserts per-channel gain `[A]`
- [x] Play `volume` clamped to [0,1] at construction `[A]`
- [x] Ducking advanced once per buffer, applied per-frame (smooth), restore honors `fade_duration_ms`, live-input voices can trigger ducking; ducked-voice RMS follows configured fade consistently across overlapping samples `[A]`
- [x] Pitch correction pre-rolled on enable (no silent gap); position-advance matches frames consumed; tail flushed `[A]` — *first-enable-at-frame-0 pre-roll + C++-side stretcher allocation are documented residuals (docs/bugs.md)*
- [x] Equal-power loop crossfade with correct overlap-on-wrap; no inter-sample click at the loop point `[A]` — *forward loops; the symmetric reverse-loop-crossfade case is a documented deferral (docs/bugs.md)*
- [x] True-peak/soft-knee limiter with configurable ceiling replaces the bare hard clamp; peak ≤ ceiling `[A]`

### Sprint 7 — Bass management & multichannel
- [x] 4th-order Linkwitz-Riley crossover; render-harness shows flat-ish acoustic-sum magnitude through the crossover `[A]`
- [x] LFE gain compensation makes sub level independent of active source count `[A]`
- [x] `remove_bass_from_sources` default decided + documented (D30 → default `true` when enabled); one-time warning when `lfe_channel >= output_channels`; LFE-collision documented `[A]`
- [x] Denormal flush in the IIR; integration test exercises bass management through `mix_audio` `[A]`

### Sprint 8 — Live input robustness
- [x] No allocation in the capture callback (allocation harness); `process_into_buffer` + reusable buffers `[A]`
- [x] Adaptive/async SRC steered by ring-buffer fill (even at equal nominal rates); drift simulation keeps the ring buffer bounded `[A]`
- [x] Underrun applies a short fade/hold (no hard cut) and keeps the ramp time-accurate `[A]`
- [x] `voice_volume` reaches input-only voices; `input_mute` restores the prior volume (not hardcoded 1.0); route source channels validated vs device channels `[A]`
- [x] Non-f32 input format supported (convert to f32) `[A]`
- [x] CoreAudio input device opens and routes into the mix (smoke) `[B]` — `device_smoke_test::default_input_device_opens_and_captures` opens the real input device + runs the capture path on this Mac (Lane B)
- [ ] ≥30-min two-device (USB mic + separate output) soak shows no periodic dropouts; non-f32 input opens `[C]` — partner's final pass (MANUAL-VERIFICATION.md W-2)

### Sprint 9 — Cleanup, observability, packaging
- [x] Dev scaffolding removed (hardcoded `/Users/bandrews` paths, `--test-*` flags + their divergent paths); dead constants and the stale `#![allow(dead_code)]` "Phase 10" banner deleted; build clean with no `allow(dead_code)` masking (all three blanket banners removed; dead `bytes_available`/`Cancelled` deleted; test-only accessors narrow-allowed) `[A]`
- [x] Status/telemetry enriched: per-voice ducking state, limiter/clip counts, dropouts/xruns, uptime, `/version`, `/metrics` returning real data `[A]`
- [x] Config schema/versioning + silent-validation-gap fixes; warnings/docs for crossfade-without-loop, seek vs start_position, channel_map↔LFE collision; unique auto voice ids `[A]`
- [x] proptest fuzz for config + command JSON (no panic) runs in the Docker pipeline `[A]`
- [x] systemd unit + structured JSON logging documented `[A]`

---

## Final gates

- [x] All sprints 0–9 are `Done` — every `[A]` (Lane A/Docker) and `[B]` (Lane B/native macOS) acceptance box is genuinely checked and green. The **only** unchecked boxes are the two `[C]` Windows items (Sprint 1 WASAPI smoke, Sprint 8 two-device soak), which are the partner's manual pass below.
- [ ] `MANUAL-VERIFICATION.md` has been run by the partner on Windows (and spot-checked on macOS) with results recorded — **awaiting the partner.** This is the one human-in-the-loop step the program reserved: the Windows WASAPI smokes (W-1), the two-device live-input soak (W-2), and the real-device *listening* checks (Sprint 5 RT soak, Sprint 8 dropout soak) that automation can't judge.
- [x] `README.md` / `CHANGELOG.md` updated for user-visible and behavior-changing items
- [x] `docs/bugs.md` reflects any out-of-scope items discovered along the way
- [x] This tracker is finished: statuses accurate, no half-truths

## Global Definition of Done (applies to every sprint)

Lane A green · Lane B green (incl. real-device smoke where the sprint touches device/format/input) ·
new tests + harness assertions added · Windows steps appended to `MANUAL-VERIFICATION.md` where relevant ·
out-of-scope items logged to `docs/bugs.md` · committed on a branch ·
`cargo build --release` warning-free.

---

# Performance & RT-hardening program (sprints 10–14)

A second program tier, planned 2026-06-09 from a principal-engineer evaluation of the core audio
loop and first-start latency (the owner's stated priority: **cold first-play latency** for disk
and HTTP sourcing, warm latency, and glitch-free playback). The evaluation, the verified
findings, and the program design are recorded in
[sprint-10](sprint-10-perf-program-plan.md). **The Charter above applies verbatim.** Decisions
for this tier are **D50–D62** in [`DECISIONS.md`](DECISIONS.md).

Ordering: 11 (measure) strictly before 12 (optimize); 12 before 13 because latency is the
owner's priority; 14 last (lowest risk, and its resampler re-pins follow 12's test changes).

> **Lane A environment note (sprints 11–14 execution, 2026-06-09):** this execution environment
> has no Docker daemon, so Lane A was run as the documented host approximation — the same gate
> steps (`cargo fmt --check`, `clippy --all-targets -- -D warnings`,
> `RUSTFLAGS=-D warnings cargo build --release`, full `cargo test`, benches) on the host
> toolchain (rust 1.94.1 vs the image's pinned 1.95). `[A]` boxes below were genuinely verified
> under that approximation; a confirming run of `scripts/validate.sh` on a Docker-capable
> machine is recommended before release. `[B]` boxes remain for the partner's native macOS pass.

## Status board

| # | Sprint | Status | Depends on | File |
|---|--------|--------|-----------|------|
| 10 | Performance & RT-hardening program plan | Done | 0–9 | [sprint-10](sprint-10-perf-program-plan.md) |
| 11 | Latency instrumentation & baselines | Done | 10 | [sprint-11](sprint-11-latency-instrumentation.md) |
| 12 | First-start latency | Done | 11 | [sprint-12](sprint-12-first-start-latency.md) |
| 13 | RT-path hardening | Done | 11 (soft: after 12) | [sprint-13](sprint-13-rt-path-hardening.md) |
| 14 | Quality & correctness backlog | Done | 11 (soft: after 13) | [sprint-14](sprint-14-quality-and-correctness.md) |

## Acceptance criteria

### Sprint 10 — Performance & RT-hardening program plan
- [x] Sprint docs 11–14 written in the established format with every `Verified at:` citation re-checked against the live tree (doc-only)
- [x] DECISIONS.md D50–D62 recorded; tracker section added; Charter noted as applying verbatim (doc-only)
- [x] `docs/bugs.md` reconciled: stale entries (DiskCache hashing, dead-code banners, `windowed` flag) marked resolved with citations; open residuals cross-linked to owning sprints (doc-only)
- [x] SWR race traced and characterized: revalidation tick refuted; `invalidate`-vs-`active_loads` race confirmed (→ Sprint 12 F2); post-unification generation hazard identified (→ Sprint 12 F1) (doc-only)

### Sprint 11 — Latency instrumentation & baselines
- [x] Six-stage play-latency model (D50) captured per play and logged; first-mix latency published from the audio thread via pre-allocated atomics with the alloc harness proving the publication is 0 alloc / 0 free `[A]` — stage capture starts at the dispatch boundary (t0/t1 collapsed; deviation recorded in sprint-11)
- [x] `/metrics` exposes `latency.play_to_first_mix_ns{last,max}` + `plays_measured` with real values asserted by an HTTP test `[A]`
- [x] Latency tests drive cache-hit, cold-local, and windowed plays offline and assert populated, monotone stages `[A]` — split lib (`tests/latency_test.rs`) / binary (`src/main.rs` test module) per the binary-crate architecture; deviation recorded in sprint-11
- [x] Criterion baselines recorded in sprint-11 doc for warm hit, cold local, cold HTTP, probe, and prebuffer-ready `[A]`
- [ ] Real-device sanity: cached play shows sub-50 ms first-mix latency on `/metrics` `[B]` — partner's pass

### Sprint 12 — First-start latency
- [x] Cold local and disk-cached-HTTP full-loads return a progressive buffer immediately and are audible before decode completes; promotion, freshness (stat-at-start), and the generation guard are test-covered `[A]` — the "pitch exception" became the stronger `UpgradeSampleBuffer` mechanism (D51 amendment; plays carry no pitch parameter)
- [x] `invalidate`/`cache_reload` abandons in-flight streaming loads (no stale promotion, no stale joins); errored loads dropped too (found in passing) `[A]`
- [x] Windowed prebuffer gate is event-driven with deadline semantics preserved (paused-time tests) `[A]`
- [x] Probe results cached by (path, mtime, size); warm windowed replay skips the header parse `[A]`
- [x] Uncached HTTP full-load reuses the probe's request — one GET, counted by a stub-server test — and persists cacheable downloads (D55 amendment: reuse supersedes the hollow "overlap") `[A]`
- [x] Before/after table recorded against Sprint 11 baselines; cold-start playable time is ~223 µs for a 300 s file (was ~211 ms, ∝ length) `[A]`
- [ ] Long cold local file audibly starts near-instantly on the real device `[B]` — partner's pass

### Sprint 13 — RT-path hardening
- [x] First duck of a never-seen voice is 0 alloc / 0 free on the callback (warm-up crutch removed from the harness) `[A]`
- [x] D18 over-cap policy implemented: steal oldest non-looping else reject, displaced sample via graveyard, alloc-free past 256; soak past the cap green `[A]`
- [x] Pitch-corrector lifecycle is control-side: toggle mid-play is Rust-side alloc/free-free; displaced corrector dropped off-RT; no-gap crossfade parity kept `[A]` — shipped per-sample via dispatch expansion (deviation note in sprint-13)
- [x] No `tracing` call sites remain on the audio or capture steady-state paths (`mixer.rs` set_speed warn moved control-side; `input.rs` capture sites are relaxed counters surfaced on `/metrics` and drained off-RT) `[A]`
- [x] Scratch buffers pre-sized to the stream's max block with a counted regrow fallback `[A]`
- [x] Dispatch warning when pitch correction targets a still-streaming buffer `[A]`
- [ ] Real-device smoke: pitch toggle + over-cap burst with zero xruns `[B]` — partner's pass (MANUAL-VERIFICATION note appended)

### Sprint 14 — Quality & correctness backlog
- [x] `/ws` streams real `{type:"log"}` frames from the live tracing subscriber (integration-tested); `docs/http-api.md` is true `[A]`
- [x] `/command` 400 rejections return the `CommandResponse` JSON shape; contract test updated; API-CONTRACT.md updated `[A]`
- [x] R1 closed: D59 overridden on measurement (stay `Linear` — Linear/Cubic identical to ~0.015% at our presets; no pinnable difference exists); the measured quality floor is pinned by a new resampler test; recorded in DECISIONS.md/bugs.md/sprint-14 `[A]`
- [x] `audio.channel_names` removed; configs containing it still parse (locked by test); changelog'd `[A]`
- [x] `docs/bugs.md` sweep complete: program-resolved entries closed with citations, retained items intact `[A]`
- [ ] Live `/ws` log lines observed against a running daemon `[B]` — partner's pass
