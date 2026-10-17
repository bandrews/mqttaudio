# Sprint 0 — Validation Harness & Docker Pipeline

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | — |
| Effort | L |
| Lanes | A (Docker), B (native macOS) |
| Subagents | Recommended (harness / Docker pipeline / command-loop extraction in parallel) |

## Goal

Make audio behavior and a clean build **verifiable locally** before any production code is touched. This
sprint creates the test infrastructure every later sprint depends on: a repeatable Docker validation
pipeline, an offline audio render harness, a unit-testable command dispatch function, and a sane policy for
hardware-dependent tests. Nothing here changes runtime audio behavior — it is pure enablement — but it is the
single highest-leverage sprint because it lets us prove the rest.

## Why

The audit found there is **no automated validation at all** and that tests cover parsing, not the
integration heart of the daemon. Specifically (test-coverage findings, all verified):

- The **main command-processing loop** (`src/main.rs:635-1172`) — Play/Stop/VoiceVolume/Seek/Speed/… → cache,
  voice manager, ducking, mixer — has **zero tests**. `tests/http_api_test.rs` only asserts JSON reaches the
  channel; `commands.rs` tests only assert `parse_command` output. The wiring (alias resolution, voice-active/
  ducking notification, start-position clamping, fade/crossfade application) is unverified.
- **`mix_audio` is never exercised with ducking or bass management enabled** — all 39 `MixerState::new(2)` in
  tests set `ducking_engine: None` and `bass_management: None` (`mixer.rs:527-536`). The features most likely
  to glitch have no integration test.
- **No offline render / spectral / click-detection harness** exists (`mixer.rs:546-585`).
- **Loop-crossfade blend** (`mixer.rs:713-735`) and device/ALSA-matching logic (`engine.rs:115-356`) are
  untested.
- **MQTT client tests depend on a live broker** (`client.rs:102-144`) and fail in a bare `cargo test`; the
  reconnect/error path is untested.
- No property/fuzz tests for the untrusted config + command JSON surface (`commands.rs`).

## Scope

**In scope**
- `docker/validate.Dockerfile` + `scripts/validate.sh` (Lanes A & B).
- An offline render harness (drive `mix_audio` deterministically, capture output) + assertion helpers
  (RMS, peak, FFT band energy, inter-sample delta / click detection).
- Extract per-command dispatch from `main()` into `async fn handle_command(...)` and add command-loop tests.
- Containerized mosquitto so broker-dependent tests run in Lane A; gate them out of bare `cargo test`.
- Gate device-opening tests behind `--ignored`/env so they run only in Lane B.

**Out of scope** (later sprints, but the harness must be built to support them)
- Any behavior fix. If you discover a bug while building tests, write a failing test that documents it,
  `#[ignore]` it with a comment pointing at the owning sprint, and log it in `docs/bugs.md` — do **not** fix
  it here.

## Tasks (ordered, TDD where applicable)

1. **Render harness.** Add a test-support module (e.g. `tests/support/render.rs` or
   `src/audio/test_support.rs` under `#[cfg(test)]`) exposing:
   - a builder for `MixerState` with samples / live inputs / `DuckingEngine` / `BassManagement` wired up;
   - `render(state, frames_per_block, blocks) -> Vec<f32>` that calls `mix_audio` block-by-block (mirroring the
     real callback cadence) and concatenates output;
   - assert helpers: `rms(buf, channel, channels)`, `peak(buf)`, `band_energy(buf, sr, lo, hi)` (small DFT/Goertzel
     is fine — no heavy dep needed), `max_inter_sample_delta(buf, channel, channels)` for click detection.
   - Reuse the existing test WAV generators / `tests/audio/*.wav` fixtures.
2. **Two seed tests** using the harness (these double as regression anchors for Sprints 6–7): one mixing two
   samples and asserting additive RMS + peak; one asserting a fade-out produces a monotonic RMS decay with no
   inter-sample click above threshold.
3. **Extract `handle_command`.** Move the body of the `while let Some(payload) = cmd_rx.recv().await` match
   (`main.rs:645-1171`) into `async fn handle_command(cmd: AudioCommand, ctx: &CommandCtx)` where `CommandCtx`
   bundles `cache_manager`, `voice_manager`, `mixer_state`, `active_voices`, `output_sample_rate`, `&config`.
   `main()` calls it. **Smallest-change refactor — no behavior change.** Confirm the build is identical
   behaviorally (existing tests still pass).
4. **Command-loop tests.** With a temp WAV (via the existing generator), drive `handle_command` and assert:
   Play lands an `ActiveSample` in `mixer_state` with expected voice_id/volume/position/fade/crossfade_samples;
   Stop/StopAll set a fade and the callback-driven `retain()` removes them; VoiceVolume ramps targets on
   matching samples; ducking `notify_voice_active(true/false)` fires on activation/deactivation.
5. **MQTT test gating + pure fn.** Refactor the `Event::Incoming(Packet::Publish)` → payload-string mapping in
   `process_mqtt_events` (`client.rs:70-77`) into a pure function and unit-test it. Put the live-broker tests
   behind a `cfg`/env gate (e.g. `MQTTAUDIO_BROKER_TESTS=1`) so a bare `cargo test` skips them and Lane A sets
   the var (with mosquitto running).
6. **Device-test gating.** Mark any test that opens a cpal device `#[ignore]` with a comment
   `// Lane B only (real device)`; add an env (e.g. `MQTTAUDIO_DEVICE_TESTS=1`) that `--native` sets.
7. **Docker pipeline.** `docker/validate.Dockerfile` (rust + the Linux build deps from `README.md`:
   `libasound2-dev libssl-dev pkg-config build-essential clang libclang-dev`, plus `mosquitto`/
   `mosquitto-clients`). `scripts/validate.sh`:
   - default (Lane A): build the image and run inside it `cargo build --release` (RUSTFLAGS `-D warnings`),
     `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, start mosquitto, run `cargo test` with
     `MQTTAUDIO_BROKER_TESTS=1` (device tests skipped), exit non-zero on any failure.
   - `--native` (Lane B): run the same cargo steps on the host plus `cargo test -- --ignored` with
     `MQTTAUDIO_DEVICE_TESTS=1` for the real-device smoke. (No Docker.)
8. **Wire `cargo fmt`/clippy clean** so the gate is meaningful from day one (fix any existing warnings; the
   repo currently has a stale `#![allow(dead_code)]` banner in `chunked_resampler.rs` — leave that for Sprint 9
   but note it).

## Files to create / touch

- Create: `docker/validate.Dockerfile`, `scripts/validate.sh` (chmod +x), render-harness module + its tests.
- Touch: `src/main.rs` (extract `handle_command`, add `CommandCtx`), `src/mqtt/client.rs` (pure mapping fn +
  test gating), `Cargo.toml` (dev-deps if a tiny FFT/test helper is wanted — prefer hand-rolled Goertzel to
  avoid new deps), test files under `tests/` or `#[cfg(test)]` modules.

## Verification

- **Lane A:** `scripts/validate.sh` builds the image and runs the full suite; exits non-zero on any failure;
  broker test passes against in-container mosquitto.
- **Lane B:** `scripts/validate.sh --native` is green on this Mac, including `cargo test -- --ignored` opening
  the real CoreAudio device.
- No new behavior to verify on Windows (Lane C) this sprint.

## Acceptance criteria (mirror the tracker)

- [ ] `docker/validate.Dockerfile` + `scripts/validate.sh` exist; Lane A runs build `-D warnings`, clippy,
  `fmt --check`, tests, render-harness, broker tests, and exits non-zero on any failure `[A]`
- [ ] `scripts/validate.sh --native` runs the host suite incl. the real-device smoke test and is green `[B]`
- [ ] Render harness exists with RMS/peak/FFT-band/inter-sample-delta helpers, used by ≥2 new tests `[A]`
- [ ] `handle_command` extracted; tests assert Play/Stop/VoiceVolume effects on mixer/voice/ducking state `[A]`
- [ ] Live-broker MQTT tests run in Docker, skipped in bare `cargo test`; event mapping is a pure tested fn `[A]`
- [ ] Device-opening tests gated to Lane B, skipped in Lane A `[A][B]`

## Behavior-change / changelog notes

None (enablement only). The `handle_command` extraction must be behavior-preserving; if it isn't, you changed
too much.

## Definition of Done

Lane A green · Lane B green · render harness + ≥2 tests landed · `handle_command` tests landed · broker/device
test gating works · committed atomically to the branch as units complete · `cargo build --release` warning-free.
