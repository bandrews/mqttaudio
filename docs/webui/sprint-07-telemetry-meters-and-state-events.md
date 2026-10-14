# Sprint 7 — Telemetry II: Meters & State-Event WebSocket

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 6 |
| Effort | L |
| Lanes | A, B, RA, RB |
| Subagents | YES (Rust meters + WS channel and the frontend meters/event client parallelize) |

## Goal

"Done and correct" means the daemon can publish output and per-input level meters and stream a second
WebSocket channel of typed state events plus throttled position/meter ticks, all under the same opt-in +
subscriber gate Sprint 6 established (DW3, DW12), and the web UI renders live meters and switches from polling
to event-driven updates the moment telemetry comes on. The RT thread writes peak/RMS into pre-sized relaxed
atomics once per audio block (the exact `clip_count` pattern — `mixer.rs:1167-1168`, `engine.rs:439`); a
control-side ~15–20 Hz timer reads those atomics plus the Sprint-6 position atomics and broadcasts compact
tick frames over a `StateBroadcaster` that mirrors `LogBroadcaster` (`websocket.rs:19-40`); discrete events
(play/stop/seek/voice_volume/input_mute/ducking/sample_finished) are emitted from the existing control-thread
mutation points. With telemetry off — or on but with no subscriber — the timer does not run, the meters are
not read, and the RT callback does exactly the work it did before this sprint.

This builds directly on Sprint 6's opt-in/subscriber gate and the per-sample position atomic, and it must not
break either of them: the position atomic and `/status/samples` real-position behavior stay as Sprint 6 left
them (this sprint *carries* position in tick frames, it does not re-implement it). It must not break the
existing `/ws` log stream, the RT no-alloc/no-free/no-lock contract (D22a — `rt_engine.rs:302-323`), or any
poll-based dashboard path: the UI's event-driven mode is an overlay that falls back cleanly to polling.

## Why

Two of the program's headline promises — "see signal/levels live" and "push updates instead of polling" —
have no data source today. The only RT→control atomics that exist are `clip_count` and `xruns`
(`mixer.rs:1167-1168`, `engine.rs:439`); there is no peak/RMS anywhere, and `/ws` carries **log lines only**
(`websocket.rs:49-103`, API-CONTRACT.md §6) — no playback or state events. So the dashboard cannot show a
meter moving with signal, and every dashboard update still costs a poll round-trip even when something just
changed a millisecond ago.

Now is the right time because Sprint 6 has already built the hard part: the opt-in control, the subscriber
gate, and the per-block relaxed-atomic position writer. Meters are the same lock-free pattern applied to the
output and capture stages, and the state channel is a second `broadcast::Sender` mirroring the one that
already streams logs. Doing it now — while the gate and the atomics pattern are fresh — keeps the mechanism
consistent and lets the frontend move from poll-only (Sprint 2) to event-driven without a later rewrite. The
mechanism must stay RT-safe by construction: the RT thread only writes atomics; a control-side ~15–20 Hz
timer reads and broadcasts (DW12). Nothing here may lock the mixer or allocate on the callback.

## Scope

**In scope**

- **Output meters (F1).** Per-channel peak (and/or RMS) accumulated per block into a pre-sized `AtomicU32`
  array (store the `f32` bits, relaxed) from the final output stage (`mixer.rs:1150-1166`). Cheap, lock-free,
  gated by the Sprint-6 telemetry-enabled flag.
- **Input capture meters (F2).** Per-input capture peak atomics off the live-input capture/mix path, for a
  signal-present / level indicator. Same lock-free, relaxed, gated pattern.
- **State-event WebSocket channel (F3).** A second broadcast channel — a `StateBroadcaster` mirroring
  `LogBroadcaster` (`websocket.rs:19-40`) — carrying typed JSON `{type:…}` frames, reached either by
  subscribing `handle_socket` to both broadcasters or via a separate `/ws/state` route. Gated:
  telemetry-enabled **AND** subscriber count > 0.
- **Discrete events + tick timer (F4).** Discrete events emitted at the existing control-thread mutation
  points (`start_sample` ~`main.rs:1289`, finish reconciliation ~`main.rs:1007-1057`, `refresh_snapshot`
  ~`main.rs:833-848`, ducking notify ~`main.rs:926-941`); a `~15–20 Hz` tokio interval samples the Sprint-6
  position atomics + the meter atomics and broadcasts compact tick frames. The RT thread writes atomics only;
  the timer (control side) broadcasts (DW12).
- **Frontend meters + event-driven updates (F5).** A state-channel client; live output + per-input meters;
  discrete events applied to the dashboard without polling; automatic fallback to polling when telemetry is
  off.

**Out of scope** (owned elsewhere — coordinate, do not duplicate)

- **The per-sample position atomic itself** — delivered in **Sprint W6** (tracker §Sprint 6, DW12). This
  sprint *reads* that atomic in the tick timer and carries the value in tick frames; it does not create or
  alter the position writer. Coordinate, do not duplicate.
- **The opt-in telemetry control and the subscriber-gate mechanism** — established in **Sprint W6** (DW3).
  This sprint reuses that exact flag and gate for meters and the state channel; it must not fork a second
  gating mechanism.
- **`GET /config` and the config/tuning panels** — owned by **Sprint W8** (DW11). Meters and events do not
  read or expose config; do not add config surface here.
- **Live position progress bars / playhead rendering** — the position-on-the-scrubber UI is **Sprint W6**'s
  box. This sprint feeds position via the state channel but does not re-own the scrubber playhead component.

## Work items

### F1 — Output peak/RMS meters from the final output stage

- **Statement:** Publish per-output-channel level (peak, and/or RMS) from the mixer's final output stage into
  pre-sized relaxed atomics, written once per block, with zero allocation/free and no lock, active only when
  telemetry is enabled.
- **Where:** `src/audio/mixer.rs:1150-1166` (the final output stage — the `for frame in output.chunks_exact_mut(channels)` loop where calibration gain, the finite-guard, the limiter and the hard clamp are applied). The
  atomic store mirrors `clip_count` at `mixer.rs:1167-1168` (`state.clip_count.fetch_add(_, Ordering::Relaxed)`).
- **Severity:** feature (telemetry data source; RT-path-touching → `[RA]`/`[RB]`).
- **Evidence:** The output stage already iterates every sample of every frame to apply gain and the limiter,
  and already accumulates `clip_events` per block before a single relaxed store — adding a per-channel
  peak/RMS accumulation is the same shape of work on data already in registers. There is no level atomic
  anywhere today; the only RT→control atomics are `clip_count`/`xruns`.
- **Approach:** Add a pre-sized `Arc<[AtomicU32]>` (one slot per output channel, sized at engine construction
  from `output_channels`) to the callback state alongside `clip_count`. In the existing output-stage loop,
  accumulate a per-channel block peak (`max(abs(sample))`) — and RMS as a running sum-of-squares if RMS is
  carried — in stack locals; after the loop, **gate on the Sprint-6 telemetry-enabled flag** and, only when
  set, store each channel's value bits with `store(bits, Ordering::Relaxed)`. No `Vec` growth, no heap touch,
  no lock — the array is pre-sized and lives behind an `Arc`. When the flag is clear, skip the stores entirely
  (the accumulation is a few register ops; do not add a branch per sample). Prove 0 alloc / 0 free on the
  callback path with the alloc harness.

### F2 — Per-input capture-level meters

- **Statement:** Publish a per-input capture peak atomic from the live-input capture/mix path, giving the UI a
  signal-present / capture-level indicator per configured input, under the same lock-free gated pattern.
- **Where:** the live-input capture/mix path (where captured input frames are read and mixed in — the input
  side that feeds the same callback state the output stage writes from). The atomic array is sized from the
  configured input count at construction, mirroring F1.
- **Severity:** feature (telemetry data source; RT-path-touching → `[RA]`/`[RB]`).
- **Evidence:** `/status/inputs` reports `muted` (= `volume == 0.0`) and `volume`, but there is no *level* —
  the UI cannot tell a live, silent mic from a live, hot one. A capture peak atomic closes that with the same
  pattern as F1.
- **Approach:** Add a pre-sized `Arc<[AtomicU32]>` (one slot per configured input) to the callback/capture
  state. In the capture/mix loop, accumulate a per-input block peak in a stack local; after the loop, gate on
  the same telemetry-enabled flag and store the bits relaxed. Same no-alloc/no-free/no-lock guarantees; same
  alloc-harness proof. Keep the capture-side accumulation symmetric with F1 so a single meter-frame schema
  carries both output and input levels.

### F3 — State-event WebSocket broadcast channel

- **Statement:** Add a second WebSocket broadcast channel carrying typed JSON state frames (`{type:…}`),
  mirroring `LogBroadcaster`, and gate its publish path on telemetry-enabled AND subscriber-count > 0.
- **Where:** `src/http/websocket.rs` (a `StateBroadcaster` next to `LogBroadcaster` — `websocket.rs:19-40` is
  the template, including `subscribe()`/`broadcast()` and the `broadcast::channel` buffer-size constant),
  `src/http/routes.rs` (wire a `/ws/state` route or extend the existing `/ws` handler to subscribe to both),
  `src/http/mod.rs` (hold the `Arc<StateBroadcaster>` in `AppState` alongside `log_broadcaster`).
- **Severity:** feature (new transport surface; `[RA]` for the Rust side, `[A]`/`[B]` for the client).
- **Evidence:** `LogBroadcaster` is a thin wrapper over a single `broadcast::Sender<String>`
  (`websocket.rs:20-40`) and `handle_socket` already subscribes to it and forwards `{type:"log",message}`
  frames (`websocket.rs:77-103`). A second channel for state is the same construct; `handle_socket` can
  `select!` over both receivers, or a dedicated `/ws/state` socket can subscribe to just the state channel.
- **Approach:** Define `StateBroadcaster` with a `broadcast::Sender<String>` (serialized typed frames) exactly
  like `LogBroadcaster`. Expose `subscriber_count()` (from `broadcast::Sender::receiver_count`) so the
  control-side publisher (F4) can honor the **AND subscriber-count > 0** half of the DW3 gate without a
  separate registry. Prefer a separate `/ws/state` route (cleaner client subscription, keeps the log socket
  unchanged) unless extending `handle_socket` proves simpler — either way, the existing `/ws` log behavior and
  its welcome/`{type:"connected"}` frame are untouched. The route stays gated by `require_auth` like the
  existing `/ws` (the handler itself does no token check; the proxy/auth layer does — API-CONTRACT.md §6).
  Frames are typed JSON objects with a `type` discriminator; document the schema in API-CONTRACT.md §7.

### F4 — Discrete events at mutation points + the ~15–20 Hz tick timer

- **Statement:** Emit discrete typed events from the existing control-thread mutation points, and run a
  control-side ~15–20 Hz tokio interval that samples the Sprint-6 position atomics and the F1/F2 meter atomics
  and broadcasts compact tick frames — all behind the DW3 gate (telemetry-enabled AND ≥1 state subscriber).
- **Where:** `src/main.rs` control mutation points — `start_sample` (~`main.rs:1289`, where a new
  `SampleStatus` is built and inserted into `playing`), finish reconciliation (~`main.rs:1007-1057`, where
  finished samples decrement voice activity and trigger ducking restore), `refresh_snapshot`
  (~`main.rs:833-848`, the status-snapshot rebuild), and ducking notify (~`main.rs:926-941`,
  `notify_voice_activity` / the ducking-snapshot write). The tick interval is a new `tokio::time::interval`
  spawned on the control runtime, sampling the position atomic (Sprint 6) and the F1/F2 meter atomics.
- **Severity:** feature (control-side wiring; RT-adjacent but **not** on the RT callback → `[RA]` proves the
  RT path is untouched).
- **Evidence:** These four sites already run on the control thread at exactly the moments the UI needs to
  hear about: `start_sample` builds the `SampleStatus` and inserts it into `playing` (`main.rs:1289-1313`);
  finish reconciliation removes finished samples and restores ducking (`main.rs:1007-1057`); `refresh_snapshot`
  is the existing snapshot rebuild (`main.rs:833-848`); ducking notify already writes the per-voice ducking
  snapshot (`main.rs:926-941`). Emitting an event there is a single `broadcaster.broadcast(frame)` call at a
  point that is already mutating the very state the event describes — no new locking, no RT involvement.
- **Approach:** At each mutation point, build the matching typed frame (`play` at `start_sample`,
  `sample_finished` + `stop` at finish reconciliation, `voice_volume`/`input_mute`/`seek` at their command
  handlers, `ducking` at the ducking-notify snapshot write) and call `state_broadcaster.broadcast(frame)` —
  **but only when telemetry is enabled** (reuse the Sprint-6 flag) to keep the off-path free. For continuous
  values, spawn one `tokio::time::interval` at ~15–20 Hz; on each tick, **first check the gate** (telemetry
  enabled AND `state_broadcaster.subscriber_count() > 0`) and skip the whole body when it fails; when it
  passes, read the position atomic(s) and the meter atomics (relaxed loads, no RT lock — D22a) and broadcast
  one compact tick frame carrying positions + meter levels. The RT thread is never asked to broadcast; the
  per-block RT writes are throttled to the wire by this timer, not by the RT thread (DW12). Do **not** poll the
  atomics faster than the publish rate, and do **not** broadcast per audio block.

### F5 — Frontend: live meters + event-driven dashboard, with polling fallback

- **Statement:** The UI subscribes to the state channel, renders live output + per-input meters, applies
  discrete events to the dashboard without polling, and automatically falls back to poll-based updates when
  telemetry is off.
- **Where:** `webui/src/features/telemetry/stateChannel.ts` (the state-channel client, over the
  `DaemonConnection` WS seam — no direct `WebSocket` per DW2), `webui/src/features/meters/OutputMeters.tsx` and
  `webui/src/features/meters/InputMeters.tsx` (meter components), and the dashboard wiring under
  `webui/src/features/dashboard/` that consumes discrete events and toggles its data source.
- **Priority:** high (the user-facing payoff of the whole telemetry track; `[A]` headless + `[B]` live).
- **Rationale:** Sprint 2's dashboard is poll-only and shows "live position unavailable" until telemetry;
  Sprint 6 added position; this sprint adds meters and the event-driven path so the dashboard reflects a
  play/stop/duck/finish the instant it happens, and shows signal moving on a meter — which a 1–2 s poll cannot.
- **Approach:** Add a state-channel client that connects through the `DaemonConnection` transport (DW2),
  parses typed frames by their `type` discriminator, and exposes them to the dashboard via the existing
  query/cache layer (TanStack Query, DW4) — applying discrete events to the cached dashboard state and
  feeding tick frames to the meter components. Meters render output peak/RMS per channel and per-input capture
  level; a finished sample is removed from the now-playing board on its `sample_finished` event (no poll).
  When telemetry is off — or the state socket is not connected — the dashboard transparently falls back to the
  Sprint-2 polling path (DW6): polling remains the always-available source, the event path is an overlay. Do
  not fabricate meter values when telemetry is off; show meters as inactive/unavailable, not as zeroed signal.

## Caveats (do not chase ghosts / do not break)

- **The RT thread writes atomics only; the control-side timer broadcasts.** The RT callback must never call
  `broadcast(...)`, never touch the `broadcast::Sender`, never spawn or wake a task. If you find yourself
  emitting a frame from inside `run_mix_callback`, stop — that is the bug DW12 exists to prevent.
- **No alloc / no free / no lock on the RT path.** The meter atomics are pre-sized at construction and live
  behind an `Arc`; the output/capture loops may only do register-level accumulation and relaxed stores. No
  `Vec` growth, no boxing, no `Mutex`/`RwLock` acquire on the callback. This is proven by the alloc-counting
  harness, **not** by inspection (Charter).
- **Never lock the RT mutex from control.** The tick timer reads atomics with relaxed loads; it must not lock
  the mixer mutex to read levels or positions (D22a — `rt_engine.rs:302-323`). The whole point of the atomics
  is to avoid that lock.
- **Gate everything (DW3).** With telemetry off, none of this runs: no meter stores in the RT loop, no tick
  timer body, no discrete-event broadcasts. With telemetry on but **zero** state subscribers, the tick timer
  must still short-circuit (subscriber-count gate) so it costs nothing when nobody is watching. Off must be a
  genuine no-op, provable on the real device (`[RB]`).
- **Throttle the wire to ~15–20 Hz.** Do not broadcast a tick per audio block (that would be hundreds of Hz
  and floods the socket); the per-block RT writes are decoupled from the publish rate by the control timer.
- **Do not fabricate position.** Position comes from Sprint 6's atomic; carry exactly what it reports. Do not
  re-hard-code `0` and do not re-implement the position writer here.
- **Do not break the `/ws` log stream.** The existing log socket, its `{type:"connected"}` welcome frame, and
  its lag/close handling (`websocket.rs:58-127`) must be untouched. The state channel is additive.
- **Label meters honestly.** Output level is signal level, not "clips"; keep the Sprint-2 label hygiene —
  `xruns` is "stream errors / rebuilds", `clip_count` is cumulative — and do not conflate a hot meter with a
  clip event.

## Tasks (ordered, TDD-first)

Each behavior task writes the failing test first, confirms it fails, writes the minimum code to pass, then
confirms green. **Subagents:** the Rust side (F1–F4: meter atomics, `StateBroadcaster`, mutation-point events,
tick timer) and the frontend side (F5: state client + meter components + event-driven dashboard) parallelize
cleanly — split them, with API-CONTRACT.md §7's frame schema as the shared contract the two agents agree on
before coding. Backend tasks use the existing Rust suite + the alloc-counting harness; frontend tasks use
Vitest + React Testing Library (+ Playwright for E2E against the mock backend).

1. **Frame schema first (shared contract).** Before either subagent writes code, pin the typed-frame schema
   in API-CONTRACT.md §7: the discrete-event shapes (`play`/`stop`/`seek`/`voice_volume`/`input_mute`/
   `ducking`/`sample_finished`) and the tick-frame shape (positions + output/input meter arrays), each with a
   `type` discriminator. This is the contract both subagents code against.

2. **Output meter atomics (F1) — failing test first.** Write a Rust test that drives the output stage with a
   known signal and asserts the per-channel meter atomics hold the expected peak (within tolerance) **when
   telemetry is enabled**, and are left untouched **when disabled**. Confirm it fails (no atomics exist).
   Implement the pre-sized `Arc<[AtomicU32]>` and the gated relaxed stores in `mixer.rs:1150-1168`. Confirm
   green. **Run the alloc harness on the callback path and confirm 0 alloc / 0 free** with telemetry both on
   and off.

3. **Input capture meter atomics (F2) — failing test first.** Write a Rust test that feeds known input frames
   and asserts the per-input capture-peak atomics, gated identically. Confirm it fails, implement the capture
   loop store, confirm green, and re-run the alloc harness (0 alloc / 0 free).

4. **`StateBroadcaster` (F3) — failing test first.** Mirror the existing `LogBroadcaster` tests
   (`websocket.rs:197-216`): write failing tests for `StateBroadcaster::new`/`broadcast`/`subscribe` and for
   `subscriber_count()` reflecting connected receivers. Confirm they fail, implement `StateBroadcaster` and
   wire it into `AppState`, confirm green.

5. **State `/ws/state` route + frame round-trip (F3) — failing test first.** Add an axum WebSocket
   integration test (router with the state socket enabled) that upgrades `/ws/state`, broadcasts a typed
   frame via the shared `StateBroadcaster`, and asserts the client receives the exact `{type:…}` JSON.
   Confirm it fails, wire the route in `routes.rs`/`websocket.rs`, confirm green. Assert the existing `/ws`
   log socket and its welcome frame are unchanged.

6. **Discrete events at mutation points (F4) — failing test first.** Write Rust tests that drive each control
   mutation point (`start_sample`, finish reconciliation, ducking notify) and assert the matching typed frame
   is broadcast **only when telemetry is enabled**, and **nothing** is broadcast when disabled. Confirm they
   fail, emit the gated `broadcast(frame)` calls at `main.rs:1289`, `:1007-1057`, `:926-941`, confirm green.

7. **Tick timer (F4) — failing test first.** Write a Rust test that, with telemetry on and ≥1 subscriber,
   asserts the ~15–20 Hz timer reads the position + meter atomics and broadcasts a tick frame carrying them;
   and that with telemetry off, **or** on with zero subscribers, the timer body does not broadcast. Confirm
   it fails, implement the gated `tokio::time::interval` sampling the atomics via relaxed loads (no RT lock),
   confirm green.

8. **Frontend state-channel client (F5) — failing test first.** Vitest/RTL test with a mock socket: the
   client connects through `DaemonConnection`, parses each typed frame by `type`, and surfaces discrete events
   + tick frames; on a malformed/unknown frame it ignores rather than throws. Confirm it fails, implement
   `stateChannel.ts`, confirm green.

9. **Frontend meters (F5) — failing test first.** RTL tests: `OutputMeters`/`InputMeters` render a level per
   channel/input from tick frames and show an inactive/unavailable state when telemetry is off (no fabricated
   signal). Confirm they fail, implement the components, confirm green.

10. **Frontend event-driven dashboard + polling fallback (F5) — failing test first.** RTL + Playwright
    (headless, mock backend): a `play` event adds a sample without a poll; a `sample_finished` event removes
    it without a poll; with telemetry off, the dashboard falls back to the Sprint-2 polling path. Confirm
    they fail, wire the event application + the source toggle, confirm green.

11. **Docs + changelog.** Document the new state channel and the frame schema in API-CONTRACT.md §7; record
    the additive, gated-off-by-default behavior change in `CHANGELOG.md` per DW12/DW3; update the webui README
    notes where the telemetry path is described.

## Files to create / touch

**Create:**
- `webui/src/features/telemetry/stateChannel.ts` — the state-channel client over the `DaemonConnection` seam.
- `webui/src/features/meters/OutputMeters.tsx`, `webui/src/features/meters/InputMeters.tsx` — meter components.
- `webui/src/features/telemetry/stateChannel.test.ts`, `webui/src/features/meters/*.test.tsx` — Vitest/RTL.
- `webui/tests/e2e/telemetry-meters.spec.ts` — Playwright headless E2E (event-driven update + fallback).
- Rust test modules/files for the meter atomics, `StateBroadcaster`, mutation-point events, and the tick timer
  (alongside the existing suites, e.g. extending `tests/http_api_test.rs` for the `/ws/state` round-trip and
  the relevant `src/` unit-test modules).

**Touch:**
- `src/audio/mixer.rs` — output-stage meter atomics (`:1150-1168`).
- the live-input capture/mix path — per-input capture meter atomics (F2).
- `src/audio/engine.rs` / the callback-state construction — size and thread the meter atomic arrays
  (mirroring `clip_count`/`xruns`, `engine.rs:439`).
- `src/http/websocket.rs` — add `StateBroadcaster`; the `/ws/state` socket handler.
- `src/http/routes.rs` — wire the `/ws/state` route.
- `src/http/mod.rs` — hold the `Arc<StateBroadcaster>` in `AppState`.
- `src/main.rs` — discrete-event broadcasts at the mutation points (`:1289`, `:1007-1057`, `:833-848`,
  `:926-941`) and the ~15–20 Hz tick `interval` spawn (reading the Sprint-6 position atomics + the meter
  atomics).
- `webui/src/features/dashboard/` — consume discrete events; toggle event-driven vs polling source (DW6).
- `docs/webui/API-CONTRACT.md` (§7 — frame schema), `CHANGELOG.md`, the webui README notes.

## Verification

### Lane A (CI / headless)
- `pnpm build`, `tsc --noEmit`, and eslint **warnings-as-errors** pass.
- Vitest/RTL: the state-channel client parses each typed frame by `type` and ignores malformed frames; meter
  components render per-channel/per-input levels from tick frames and show inactive/unavailable when telemetry
  is off (no fabricated signal); a `play` event adds and a `sample_finished` event removes a sample with no
  poll.
- Playwright (headless, mock backend): the event-driven dashboard updates from frames, and with telemetry off
  it falls back to the Sprint-2 polling path.

### Lane B (real browser + live daemon)
- Against a real daemon behind the sidecar with telemetry on: output and per-input meters **move with signal**
  (start a play and watch the level rise; stop it and watch it fall); a discrete change (play/stop/duck/
  voice_volume/input_mute) updates the UI **without a poll**; a finished sample **disappears on its
  `sample_finished` event**, not on the next poll.
- With telemetry off: meters read inactive/unavailable, the state socket carries no ticks, and the dashboard
  is driven entirely by polling.

### Rust Lane A [RA]
- `cargo build --release` with `-D warnings`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo fmt --check` pass.
- The meter atomics (F1/F2) publish correct peak/RMS under a known signal **only when telemetry is enabled**,
  via relaxed stores, with **0 alloc / 0 free on the callback path** (alloc harness) — telemetry on *and* off.
- The `StateBroadcaster` round-trips a typed frame over `/ws/state`; the existing `/ws` log socket and welcome
  frame are unchanged.
- Discrete events fire at each mutation point only when enabled; the ~15–20 Hz tick timer broadcasts
  position + meter frames only when telemetry is enabled **AND** ≥1 state subscriber is connected, and is a
  no-op otherwise. The control side never locks the RT mutex (D22a).

### Rust Lane B [RB]
- On the native macOS CoreAudio device, with telemetry on under load, the state channel + meters run clean
  (advancing positions, meters tracking real signal, no audible regression); with telemetry off, the whole
  mechanism stops entirely — no meter stores, no tick timer, no events — and the RT path is the pre-sprint
  no-op.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 7)

- [ ] Output peak/RMS and per-input capture-level meters are published via relaxed atomics from the output/capture stages, gated by the same opt-in/subscriber mechanism (0 alloc/free, no RT lock) `[RA]`
- [ ] A second WebSocket channel carries typed state events (play/stop/seek/voice_volume/input_mute/ducking/sample_finished) plus throttled (~15–20 Hz) tick frames for position/meters, emitted from existing control-thread mutation points + a control-side timer (DW12) `[RA]`
- [ ] On the real device, the state channel + meters run clean under load with telemetry on, and stop entirely with telemetry off `[RB]`
- [ ] The UI subscribes to the state channel and renders live output + per-input meters and event-driven updates, auto-falling back to polling when telemetry is off `[A]`
- [ ] Against a real daemon, meters move with signal, discrete events update the UI without a poll, and a finished sample disappears on its `sample_finished` event `[B]`

## Behavior-change / changelog notes

- **New WebSocket state channel + meter telemetry (additive, gated OFF by default).** A second WebSocket
  channel (`/ws/state` or the `/ws` extension) now carries typed state events plus throttled ~15–20 Hz tick
  frames for position and meters; output peak/RMS and per-input capture-level meters are published via relaxed
  RT atomics. All of it is gated by the DW3 opt-in + subscriber mechanism and costs nothing on the RT path
  when off. Record in `CHANGELOG.md` per **DW12** (the lock-free-atomics + control-side-broadcaster mechanism)
  and **DW3** (opt-in, subscriber-gated, off by default).
- **API-CONTRACT.md §7** gains the concrete state-channel frame schema (discrete-event shapes + the tick-frame
  shape) — document it as part of this sprint so the frontend client and the daemon agree on the wire format.
- No existing endpoint or the `/ws` log stream changes behavior; this is purely additive.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless
against the mock backend) · Lane B green (live daemon behind the sidecar: meters move with signal, discrete
events update without a poll, `sample_finished` removes its sample) · Rust Lane A green (`cargo build
--release -D warnings` · clippy `-D warnings` · `fmt --check` · full Rust suite · the **alloc harness showing
0 alloc / 0 free** on the callback path with telemetry on *and* off · the `/ws/state` round-trip and gated
event/tick tests) · Rust Lane B green (native CoreAudio: clean under load on, total no-op off) · the control
side never locks the RT mutex (D22a) and the RT thread never broadcasts (DW12) · new tests added with no
coverage reduction · zero build/type/lint warnings · the state-channel frame schema documented in
API-CONTRACT.md §7 and the additive gated-off behavior change recorded in `CHANGELOG.md` per DW12/DW3 ·
out-of-scope discoveries logged in `docs/bugs.md` (tagged by owning web sprint + finding) · committed on a
branch with a clear message.
