# Sprint 2 — Control-plane Reliability

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A |
| Subagents | optional |

## Goal

Make the daemon survive the operational events a long-running, MQTT-controlled audio player actually hits:
a **broker restart**, a **slow/large decode**, a **panicking handler**, and **process shutdown** —
without going silently deaf, stalling the command pipeline, bricking audio, or cutting output with a click.
This sprint fixes the control plane (MQTT ingestion, the command loop, lock hygiene, and shutdown). It does
**not** redesign the real-time audio path — that is Sprint 5, which removes the callback locks entirely. The
fixes here are the smallest reasonable changes that close confirmed reliability holes and harden two cheap
panic surfaces, all verifiable in Lane A (Docker + containerized mosquitto).

## Why

The audit's RELIABILITY findings (all re-verified against current source for this sprint) describe a daemon
that *looks* healthy while being non-functional:

- After **any broker restart or transient disconnect**, the daemon reconnects but receives **zero commands**
  — a silent core-function outage requiring a full restart.
- A single **Play of a large local file** blocks a tokio worker thread for the whole decode+resample while
  holding the shared `cache_manager` mutex, stalling the command loop and the HTTP status endpoints.
- A **panic in any lock holder** poisons a shared `std::sync::Mutex`; the next audio-callback `.lock().unwrap()`
  then panics and audio is **permanently silenced** (no `panic = "abort"` in `Cargo.toml`).
- `systemctl stop` / Ctrl-C **kills the daemon mid-buffer** (audible click/pop) with no fade-out and no cache
  flush — the production path has no signal handler at all.
- A **stalled consumer back-pressures the bounded command channel**, which blocks `eventloop.poll()`, stops
  MQTT keepalive, and gets the broker to drop the connection — compounding the no-resubscribe bug.
- Two cheap panic surfaces (`partial_cmp().unwrap()` on an RT-reachable path; `resolve_*().expect()` at
  startup) should be hardened while we are here.

## Scope

**In scope**
- Re-subscribe on MQTT `Packet::ConnAck` (survive broker restart).
- Drop the `cache_manager` guard before `.await`; run `decode_file` via `spawn_blocking`.
- Make RT-shared state non-poison-bricking (interim hardening that pairs with Sprint 5).
- SIGINT/SIGTERM handler in the production daemon: fade active samples, drain, flush cache metadata, exit.
- Ensure a >100-command burst cannot stall `eventloop.poll()`.
- `total_cmp` for the ducking min; validate `ducking_rules[*].target_volume` finite ∈ [0,1]; replace startup
  `resolve_*().expect()` with a graceful error + exit.

**Out of scope**
- Removing the callback locks and the entire RT redesign — **Sprint 5**. The lock-hardening here is explicitly
  an *interim* measure; do not over-build it.
- Atomic disk-cache writes + size/checksum validity check. The RELIABILITY dump lists a "non-atomic disk cache
  write" item (`disk.rs:289,180-187,134-139`); **that fix lives in Sprint 3 (Security & file safety)**. This
  sprint's shutdown handler will *flush cache metadata*, but the atomic-write fix is referenced only, not
  implemented here. See `sprint-03-security-and-file-safety.md`.
- Output/input **device-error recovery** (rebuild the stream with backoff). The "device error callbacks only
  log" finding (`main.rs:583-585`) is owned by **Sprint 1** (tracker acceptance: "Device-error callback
  rebuilds the stream with backoff `[A]`", see `sprint-01-device-format-compatibility.md`). Do not touch it
  here; the SIGTERM handler below is shutdown, not device-loss recovery.

## Findings addressed

### F1 — MQTT does not re-subscribe after reconnect (daemon goes permanently deaf) `[HIGH, confirmed]`
- **Statement:** With `clean_session=true` and a single startup subscribe, the daemon reconnects after a
  broker restart but is never re-subscribed, so it receives no further command messages.
- **Where:** `src/mqtt/client.rs:40` (`set_clean_session(true)`), `:51-54` (single startup `client.subscribe`),
  `:62-96` (`process_mqtt_events`), `:79-81` (the `Packet::ConnAck` arm only logs `"MQTT connected"`).
- **Evidence:** On a poll error the loop sleeps 5s and continues (`:89-93`). rumqttc-0.24 `poll()`
  auto-reconnects, but `state.clean()` only re-queues outgoing `Publish`/`PubRel` into `pending` — **not** the
  `Subscribe`. With `clean_session=true` the broker keeps no subscription across sessions, and the `ConnAck`
  arm never re-subscribes. Net: reconnect succeeds (logs healthy), commands stop arriving.
- **Fix:** Re-issue `client.subscribe(topic, QoS::AtLeastOnce)` on `Packet::ConnAck`. This requires passing the
  `AsyncClient` and `topic` into `process_mqtt_events` (today only the `EventLoop` and `command_tx` are passed;
  the client is dropped at the call site — `main.rs:619` destructures `(_, eventloop)` and discards the client).
- **Verify against a broker RESTART**, not just initial connect.

### F2 — `std::sync::Mutex` guard on `cache_manager` held across `.await` with synchronous decode `[HIGH, confirmed]`
- **Statement:** The command loop holds a `std::sync::MutexGuard` across an `.await` while a CPU-heavy
  synchronous `decode_file` runs on the async runtime.
- **Where:** `src/main.rs:652-654` (Play: lock at 652, `get_or_load_streaming(...).await` at 653, `drop` at
  654), `src/main.rs:918-919` (Precache: lock at 918, `precache_streaming(...).await` at 919). Decode lives in
  `src/cache/mod.rs`: local-file branch calls `decoder::decode_file` synchronously at `:131-135`, disk-cache-hit
  branch at `:113-117`. `decode_file` is `pub fn` at `src/audio/decoder.rs:64` (symphonia decode + rubato
  resample — blocking).
- **Evidence:** A large local-file Play blocks the tokio worker thread for the whole decode while holding the
  shared mutex; the same mutex is locked by the HTTP handlers (`src/http/handlers.rs:570-572,601,645,652,671`),
  so a slow decode stalls `handle_status`/`handle_cache_status` too. The command loop is single-consumer
  (`main.rs:635`).
- **CAVEAT (do not overstate):** The HTTP-streaming **download/decode already runs in `spawn_blocking` outside
  the lock** (`src/cache/mod.rs:186-188`); `start_streaming_load` returns the streaming buffer immediately at
  `:190`. Under the guard the HTTP path only awaits `start_http_stream` (connection open, `:149-151`). So the
  problems are **(a)** the local-file / disk-hit synchronous `decode_file`, and **(b)** holding the guard across
  the HTTP connect. Do not "fix" the HTTP download — it is already offloaded.
- **Fix:** Drop the guard before awaiting (copy out what you need), and run `decode_file` via
  `tokio::task::spawn_blocking`.

### F3 — Shared `std::sync::Mutex` with `.lock().unwrap()` — one panicking holder poisons and bricks audio `[HIGH, confirmed]`
- **Statement:** `mixer_state`, `voice_manager`, `active_voices`, and `cache_manager` are `std::sync::Mutex`
  accessed via `.lock().unwrap()`. A panic in any holder poisons the mutex; the next callback `.lock().unwrap()`
  panics and audio is permanently silenced.
- **Where:** `src/main.rs:394` (`use std::sync::{Arc, Mutex}` — std, not tokio). Callback locks at
  `src/main.rs:556` (`mixer_state_clone.lock().unwrap()`) and `:580` (`active_voices_clone.lock().unwrap()`).
  Command/HTTP handler `.lock().unwrap()` sites: `main.rs:477,519,536,652,700,780,787,795,809,828,836,854,862,
  880,887,918,931,953,967,1002,1043,1076,1110,1140` (26 total in `main.rs`) plus
  `src/http/handlers.rs:570-572,601,645,652,671`. `Cargo.toml [profile.release]` (`:88-90`) sets only `lto` and
  `codegen-units` — **no `panic = "abort"`**, so default unwind poisons on panic.
- **Evidence:** Verified: 26 `.lock().unwrap()` in `main.rs` including the two callback sites; no `panic = "abort"`.
  Poison semantics: after a panic-while-held, every `.lock().unwrap()` (including `main.rs:556`) panics.
- **Fix (interim — pairs with Sprint 5):** Stop poison-bricking the **RT-shared** state. Either a non-poisoning
  lock (`parking_lot::Mutex` — new dep; `parking_lot` is *not* in `Cargo.toml` today) or a PoisonError-recovering
  helper (`.lock().unwrap_or_else(|e| e.into_inner())`) for `mixer_state` / `active_voices` (and the others, for
  consistency). Keep the callback's lock sections panic-free regardless. **Sprint 5 removes the callback locks
  entirely** — do not redesign here; pick the smaller of the two options and apply it uniformly to the shared
  mutexes.
- **DECISION NEEDED (partner sign-off):** parking_lot (new dependency, cleanest) vs. a recovering-lock helper
  (no new dep, more call-site churn). Pick one and note it in the changelog.

### F4 — No SIGINT/SIGTERM handler or fade-out on exit in the production daemon `[MEDIUM]`
- **Statement:** The production command loop has no signal handler; on SIGINT/SIGTERM the process is killed
  immediately, cutting audio mid-buffer with no fade and no cache flush.
- **Where:** `src/main.rs:635` (`while let Some(payload) = cmd_rx.recv().await { ... }` runs to ~`:1172` with no
  signal handling). `ctrlc::set_handler` exists **only** in the dev/test branches at `main.rs:214,240,267`
  (`--test-tone`, `--file`, `--test-mixer`). `ctrlc = "3.4"` and `tokio = { features = ["full"] }` are in
  `Cargo.toml` (so `tokio::signal` is available). StopAll's fade is command-driven only.
- **Evidence:** On `systemctl stop` / Ctrl-C, audio is cut mid-buffer (audible click/pop) and in-flight cache
  metadata is not flushed.
- **Fix:** Install a `tokio::signal` handler in the daemon loop (`tokio::signal::ctrl_c()` and, on unix,
  `signal(SignalKind::terminate())`). On signal: apply a short fade-out to `active_samples`, let the callback
  drain that fade, flush cache metadata, then exit. Use `tokio::select!` over the signal future and
  `cmd_rx.recv()` so the loop exits cleanly.

### F5 — Bounded command channel (100) with blocking `.send().await` stalls the MQTT poll loop `[MEDIUM]`
- **Statement:** A stalled consumer fills the bounded channel; the MQTT producer's blocking `send().await` then
  blocks `eventloop.poll()`, stopping keepalive and getting the broker to drop the connection.
- **Where:** `src/main.rs:593` (`mpsc::channel::<String>(100)`). MQTT producer `command_tx.send(payload).await`
  at `src/mqtt/client.rs:75`; HTTP producer at `src/http/handlers.rs` (`state.cmd_tx.send(...).await`). Single
  consumer at `main.rs:635`.
- **Evidence:** This compounds F1: when `poll()` stops, no keepalive pings flow and the broker drops the
  session — then F1 means no resubscribe on the eventual reconnect.
- **Fix (root cause is F2):** The primary fix is making the consumer non-blocking — F2 (spawn_blocking decode,
  no guard across `.await`) removes the multi-second stalls that fill the channel. Additionally, decouple MQTT
  ingestion so `poll()` is never blocked: use `try_send` for the MQTT producer with an explicit overflow policy
  (log + drop, or count drops) so a backlog cannot back-pressure the event loop. Keep HTTP on the blocking
  `send().await` (an HTTP request hanging is acceptable; an MQTT keepalive stall is not).

### F6 — Ducking `update_duck_states` uses `partial_cmp().unwrap()` (NaN-panic) on a callback-reachable path `[LOW]`
- **Statement:** A NaN `target_volume` would make `partial_cmp` return `None`, and the `.unwrap()` panics —
  on a path reachable from the audio callback, which would poison the mixer mutex (see F3).
- **Where:** `src/audio/ducking.rs:213-214`
  (`.min_by(|a, b| a.partial_cmp(b).unwrap())` then `.unwrap()` at `:214`; the fade `.min().unwrap()` at `:219`
  is `u32` and safe). `update_duck_states` is called from `notify_voice_active` (`ducking.rs:151`), which the
  audio callback invokes at `main.rs:575` while the mixer mutex is held. `Config::validate`
  (`src/config.rs:683`) does **not** validate `ducking_rules[*].target_volume` (it checks `channel_volumes` and
  `bass_management` but never ducking targets).
- **Evidence:** Standard JSON cannot represent NaN and serde_json rejects a bare `NaN` literal, so this is hard
  to trigger via config — hence **low** — but it sits on an RT-reachable, mutex-held path.
- **Fix:** Replace `partial_cmp().unwrap()` with `f32::total_cmp` (a NaN-safe total order). Add validation in
  `Config::validate` that each `ducking_rules[*].target_volume` is finite and ∈ [0.0, 1.0]. (`DuckingRule` is
  defined at `src/audio/ducking.rs:9`, `target_volume: f32` at `:17`; `config.ducking_rules: Vec<DuckingRule>`
  at `src/config.rs:422`.)

### F7 — `config.resolve_*().expect()` at startup `[LOW]`
- **Statement:** Two startup `expect()`s panic if validation and resolution diverge; prefer a graceful error +
  `std::process::exit`.
- **Where:** `src/main.rs:407-408` (`config.resolve_bass_management().expect(...)`) and `:456-457`
  (`config.resolve_input_routes(&input_config.routes).expect(...)`). Both run after `config.validate()`
  (`main.rs:286`), which validates bass management and input-route alias resolution, so they should not fire in
  normal operation.
- **Evidence:** Low: a panic here only occurs on a validate/resolve divergence (a bug), at startup before audio
  serves, so it crashes cleanly rather than corrupting a stream. Worth tidying to match surrounding error
  handling.
- **Fix:** Replace `expect()` with `match` → log the error and `std::process::exit(1)` (or propagate an error),
  matching the surrounding startup error-handling style.

## Caveats (refuted / over-stated — don't chase ghosts)

- **F2 over-statement:** The dump's wording "holds the guard across the network download" is too strong. The
  HTTP **decode** is already in `spawn_blocking` (`cache/mod.rs:186-188`) and `start_streaming_load` returns at
  `:190`; only the HTTP **connect** (`start_http_stream`, `:149-151`) and the **local/disk-hit synchronous
  `decode_file`** run under the guard. Fix those; leave the HTTP download offload alone.
- **F3 secondary claim:** "likely crashes the audio thread" is host-dependent (cpal's behavior on a panicking
  callback varies). The **silencing-via-poison** impact stands on its own; do not build anything around the
  thread-crash claim.
- **F4 vs. device loss:** The shutdown handler is for SIGINT/SIGTERM only. **Device-loss recovery** (rebuild the
  stream) is Sprint 1, not here.
- **Disk-write atomicity:** The atomic temp-file+rename and size/checksum validity fix is **Sprint 3**. Here we
  only *flush* metadata on shutdown; we do not change the write path.

## Tasks (ordered, TDD)

> TDD throughout: write the failing test, confirm it fails, write the minimum code to pass, confirm green,
> refactor. Lane A (Docker + containerized mosquitto) is the gate. Broker tests use the `MQTTAUDIO_BROKER_TESTS=1`
> gating established in Sprint 0.

1. **F1 — Re-subscribe on ConnAck (integration test against a broker restart).**
   - *Failing test first:* in `tests/` add `mqtt_resubscribe_test.rs` (broker-gated). Connect a client + the
     daemon's `process_mqtt_events` against containerized mosquitto; publish a command and assert it arrives;
     **restart mosquitto** (or force a disconnect — kill the broker process / `docker restart`); after
     reconnect, publish a *second* command and assert it **also** arrives on `command_tx`. With the current
     code the second assert fails (no resubscribe). This is the load-bearing test for the whole finding.
   - *Minimal code:* change `process_mqtt_events(eventloop, command_tx)` →
     `process_mqtt_events(client: AsyncClient, topic: String, eventloop: EventLoop, command_tx)`; in the
     `Packet::ConnAck` arm (`client.rs:79-81`) call `client.subscribe(&topic, QoS::AtLeastOnce).await` (log on
     error, do not panic). Update the call site `main.rs:619-623` to pass the client + topic (today it
     destructures `(_, eventloop)` and discards the client).
   - *Note:* keep the existing startup subscribe in `connect_mqtt` (it makes the first connect work before any
     ConnAck is processed) — or remove it and rely solely on ConnAck; if removed, the initial-connect broker
     test must still pass. Smallest change: keep startup subscribe **and** add ConnAck resubscribe.

2. **F2 — Drop guard before `.await`; `decode_file` in `spawn_blocking`.**
   - *Failing test first:* unit-test the cache path. Add a test that a local-file `get_or_load_streaming`
     completes without holding the `cache_manager` mutex across the blocking decode — assert via a structural
     test: lock `cache_manager` from a second task/thread while a decode of a known temp WAV (Sprint 0 WAV
     generator) is in flight and assert the second lock acquires promptly (no multi-second stall). Confirm it
     fails on current code (guard held across the synchronous decode).
   - *Minimal code:* in `src/cache/mod.rs`, wrap the synchronous `decoder::decode_file` calls (`:131-135` local,
     `:113-117` disk-hit) in `tokio::task::spawn_blocking(...).await`. In `src/main.rs:652-654` and `:918-919`,
     restructure so the `cache_manager` guard is dropped before the `.await` (copy out the resampler quality /
     paths needed, or move the decode entirely behind an `async` method that does not hold the std guard across
     the blocking work). Do **not** touch the HTTP `spawn_blocking` at `cache/mod.rs:186-188`.
   - *Harness check:* no audio-output assertion needed (this is control-plane), but confirm an existing Sprint 0
     `handle_command` Play test still lands the expected `ActiveSample` (behavior-preserving).

3. **F3 — RT-shared state no longer poison-bricks audio (partner-signed lock choice).**
   - *Failing test first:* `tests/poison_recovery_test.rs` — construct the shared mixer state the way `main`
     does; spawn a task that locks it and panics while holding the guard (poisoning it under std semantics);
     then assert the audio-callback lock path still acquires and runs `mix_audio` without panicking. On current
     code (`.lock().unwrap()`) this panics; after the fix it succeeds.
   - *Minimal code:* per the partner-chosen option — either swap the RT-shared mutexes to `parking_lot::Mutex`
     (add `parking_lot` to `Cargo.toml`; non-poisoning, `.lock()` returns the guard directly) **or** replace
     `.lock().unwrap()` with `.lock().unwrap_or_else(|e| e.into_inner())` on the shared mutexes. Apply uniformly;
     keep callback lock sections panic-free. **Get partner sign-off on the choice before implementing** (new dep
     vs. call-site churn) — see DECISION NEEDED in F3.

4. **F6 — `total_cmp` + validate ducking `target_volume`.** (Do this near F3 — same panic-poison family.)
   - *Failing test first:* a `ducking.rs` unit test that drives `update_duck_states` with a rule whose
     `target_volume` is `f32::NAN` and asserts no panic (it currently panics at `:214`). Plus a `config.rs`
     validation test asserting `validate()` returns an error for `target_volume = 1.5` and for a non-finite
     value (currently `validate()` accepts both — there is no ducking-target check at `config.rs:683+`).
   - *Minimal code:* replace `partial_cmp(b).unwrap()` at `ducking.rs:213-214` with
     `a.total_cmp(b)`. In `Config::validate` (`config.rs:683`) add: for each `ducking_rules[*].target_volume`,
     push an error if `!v.is_finite() || !(0.0..=1.0).contains(&v)`.

5. **F4 — SIGINT/SIGTERM graceful shutdown.**
   - *Failing test first:* a test that exercises the shutdown routine as an extracted function, e.g.
     `async fn shutdown(ctx: &CommandCtx)` (reuse the Sprint 0 `CommandCtx`): seed `mixer_state` with an
     `ActiveSample`, call `shutdown`, and assert (a) every active sample has a fade-out set, and (b) cache
     metadata flush was invoked (assert the on-disk metadata file is written, using the real disk cache against
     a temp dir — no mocks). Drive the faded samples through the Sprint 0 render harness and assert the tail
     **RMS decays monotonically** and `max_inter_sample_delta` stays below the click threshold (no abrupt cut).
   - *Minimal code:* extract `shutdown(...)` (fade `active_samples`, allow drain, flush cache metadata). In the
     daemon loop (`main.rs:635`), wrap the `recv()` in `tokio::select!` against `tokio::signal::ctrl_c()` and
     (unix) `signal(SignalKind::terminate())`; on signal call `shutdown(...)` then break/exit. Mirror the
     cleanliness of the existing `--test` modes; do **not** reuse `ctrlc` (use `tokio::signal` in the async
     loop).

6. **F5 — Burst >100 commands does not stall `eventloop.poll()`.**
   - *Failing test first:* broker-gated integration test: publish a burst of >100 commands while the consumer is
     artificially slow (or while a real decode is in flight pre-F2), and assert that MQTT `poll()` keeps running
     (keepalive continues / the broker does not drop the session) and commands are not lost beyond the declared
     overflow policy. With current blocking `send().await` and a stalled consumer this regresses.
   - *Minimal code:* F2 already removes the multi-second consumer stalls. Additionally switch the **MQTT**
     producer (`client.rs:75`) to `try_send` with an explicit overflow policy (log + count drops) so a backlog
     can never block `poll()`. Leave HTTP's `send().await` as-is.

7. **F7 — Replace startup `resolve_*().expect()` with graceful exit.**
   - *Failing test first:* this path is `main()`-level startup; assert via a small unit test on an extracted
     helper if one is cheap, otherwise verify by code review + Lane A build. (Low-risk; do not over-engineer a
     test harness around `process::exit`.)
   - *Minimal code:* at `main.rs:407-408` and `:456-457`, replace `expect(...)` with `match` → `tracing::error!`
     + `std::process::exit(1)`, matching surrounding startup error handling.

8. **Changelog + commit.** Commit this work atomically to the branch with a clear message. Note
   behavior-changing items (below) and the F3 lock-choice decision.

## Files to create / touch

- **Create:**
  - `tests/mqtt_resubscribe_test.rs` (broker-gated; broker-restart integration test — F1).
  - `tests/poison_recovery_test.rs` (F3).
  - Shutdown test (F4) — co-locate with the Sprint 0 `handle_command` tests or a new `tests/shutdown_test.rs`.
  - F5 burst test (broker-gated) — may live in `mqtt_resubscribe_test.rs` or its own file.
- **Touch:**
  - `src/mqtt/client.rs` — `process_mqtt_events` signature + `Packet::ConnAck` resubscribe (F1); `try_send`
    overflow policy (F5).
  - `src/main.rs` — call site at `:619-623` (pass client+topic, F1); drop `cache_manager` guard before `.await`
    at `:652-654` / `:918-919` (F2); RT-shared lock change at callback `:556,580` and handler sites (F3);
    `tokio::signal` shutdown around the `:635` loop + extracted `shutdown(...)` (F4); `:407-408,456-457`
    `expect` → graceful exit (F7).
  - `src/cache/mod.rs` — `spawn_blocking` around `decode_file` at `:131-135` and `:113-117` (F2).
  - `src/audio/ducking.rs` — `total_cmp` at `:213-214` (F6).
  - `src/config.rs` — validate `ducking_rules[*].target_volume` in `validate()` at `:683+` (F6).
  - `src/http/handlers.rs` — RT-shared lock change at `:570-572,601,645,652,671` if the F3 fix is applied
    uniformly (parking_lot swap touches these; the recovering-helper option may leave them as-is).
  - `Cargo.toml` — add `parking_lot` **iff** that F3 option is chosen.

## Verification

**Lane A (Docker + containerized mosquitto) — the only lane for this sprint:**
- `scripts/validate.sh` (build `-D warnings`, clippy `-D warnings`, `fmt --check`, full `cargo test` with
  `MQTTAUDIO_BROKER_TESTS=1`) exits 0.
- **Broker-restart integration test (F1)** passes: a command published *after* mosquitto is restarted is
  received — the central proof for this sprint. Not just initial-connect.
- **F2** structural test proves `cache_manager` is not held across the blocking decode (second locker acquires
  promptly).
- **F3** poison-recovery test proves a poisoned non-RT lock does not kill the callback path.
- **F4** shutdown test proves fade-out is applied and cache metadata is flushed; render-harness asserts
  monotonic RMS decay + no inter-sample click on the faded tail.
- **F5** burst test proves >100 commands do not stall `poll()` (keepalive continues, session held).
- **F6** tests: NaN `target_volume` does not panic; `validate()` rejects out-of-range/non-finite targets.

**Lane B (native macOS):** Not required this sprint — no device/format/RT-output behavior changes. (The repo's
global rule still wants `scripts/validate.sh --native` green for the build/lint/test gate, but no new
device-specific assertion is added here.)

**Lane C (manual Windows):** Nothing to add to `MANUAL-VERIFICATION.md` this sprint.

## Acceptance criteria (mirrors the tracker, Sprint 2 — verbatim)

- [ ] Re-subscribe on `Packet::ConnAck`; integration test proves commands arrive after a broker restart `[A]`
- [ ] Cache guard dropped before `.await`; `decode_file` runs in `spawn_blocking` `[A]`
- [ ] RT-shared state no longer poison-bricks audio (non-poisoning or PoisonError-recovering locks); test proves a poisoned non-RT lock doesn't kill the callback path `[A]`
- [ ] SIGINT/SIGTERM handler fades active samples, drains, flushes cache metadata; test verifies clean shutdown `[A]`
- [ ] Burst >100 commands does not stall `eventloop.poll()` `[A]`
- [ ] `total_cmp` replaces `partial_cmp().unwrap()`; `ducking_rules.target_volume` validated finite ∈[0,1]; `resolve_*` exits gracefully instead of `expect` `[A]`

## Behavior-change / changelog notes

- **F1 (behavior change):** The daemon now re-subscribes on every (re)connect, so it recovers MQTT command
  handling after a broker restart instead of going silently deaf. User-visible reliability change — changelog it.
- **F3 (behavior change + PARTNER SIGN-OFF):** RT-shared mutexes change from poison-propagating
  `.lock().unwrap()` to either `parking_lot::Mutex` (**new dependency**) or a PoisonError-recovering lock.
  Recovering from a poisoned lock means continuing past a prior panic with possibly-inconsistent state — an
  intentional trade chosen so a single handler panic cannot permanently silence audio (Sprint 5 removes these
  locks). **Get partner approval on the lock choice; record it in the changelog.**
- **F4 (behavior change):** SIGINT/SIGTERM now triggers a short fade-out + cache-metadata flush before exit
  (previously an immediate hard kill). Changes shutdown audio behavior (no more mid-buffer click) — changelog it.
- **F5 (behavior change):** The MQTT producer may now **drop** commands under sustained overflow (with logging/
  counting) rather than back-pressuring `poll()`. This is a deliberate availability-over-delivery choice for the
  keepalive path — changelog the overflow policy. (HTTP delivery is unchanged.)
- **F6 (behavior change):** Configs with a non-finite or out-of-range `ducking_rules[*].target_volume` are now
  rejected at validation instead of accepted. Note in changelog (could reject a previously-"valid" config).
- **F7:** Internal robustness only (graceful exit instead of panic on an unreachable startup edge) — minor
  changelog note.

## Definition of Done

Lane A green (build `-D warnings`, clippy clean, `fmt --check`, full test suite incl. the broker-restart
integration test) · all six acceptance boxes genuinely checked · F3 lock choice approved by the partner and
recorded · new tests landed for F1–F6 (F7 by review) · the disk-write atomicity item left to Sprint 3 and the
device-error recovery left to Sprint 1 (referenced, not implemented) · out-of-scope discoveries logged to
`docs/bugs.md` · committed atomically to the branch as units complete · `cargo build --release` warning-free.
