# Sprint 6 — Telemetry I: Live Position + Opt-In Gating

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0, 5 |
| Effort | L |
| Lanes | A, B, RA, RB |
| Subagents | YES (Rust engine work and the frontend progress UI parallelize) |

## Goal

Make sample progress **real** — the opt-in, subscriber-gated way (DW3). Today `/status/samples` reports
`position`, `position_ms`, and `progress_percent` as a hard-coded `0` (`handlers.rs:721-730`); the live value
exists only on the RT-owned `ActiveSample` (`Voice.position`, `mixer.rs:115`, advanced in `advance_position`,
`mixer.rs:605-643`) and is never copied to the control side (D20). This sprint surfaces that value without
violating the RT contract: each active sample publishes its current frame position into a pre-sized relaxed
atomic, written **once per audio block** at the advance point (`mixer.rs:1112`), mirroring the `clip_count`
pattern (`mixer.rs:1167-1168`) and the `xruns` pattern (`engine.rs:439`); the control thread **reads** that
atomic in `handle_samples` and reports real `position`/`position_ms`/`progress_percent`. The frontend then
renders real progress bars on the now-playing board and a live playhead on the transport scrubber.

"Done and correct" means: telemetry is **OFF by default** and costs ~nothing on the audio path when off (DW3) —
with telemetry off, the RT path does no new work and `/status/samples` returns `0` exactly as today; when
telemetry is on, the position advances and matches audible playback for normal and looped samples; the
control thread **never** locks the RT mutex (D22a, `rt_engine.rs:4-9`) and the audio callback does **0 alloc /
0 free** (proven by the alloc-counting harness, not by inspection). This builds directly on Sprint 5's
transport scrubber (the playhead overlays the existing seek control) and the Sprint 5 windowed-gating logic; it
must not regress the lock-free RT engine, the existing `clip_count`/`xruns` telemetry, or the Sprint-5 seek/
speed/reverse gating. It is the foundation for Sprint W7 (meters + the state-event WebSocket): the position
atomic this sprint adds is the value the W7 throttled tick frames will carry.

## Why

The partner's explicit "see sample progress live" goal has no real implementation. Sprint W2 deliberately shows
"live position unavailable" on the now-playing board, and Sprint W5 builds a seek scrubber whose playhead has
nothing to track, because the daemon hard-codes position to `0` (`handlers.rs:721-730`). The data the UI needs
already exists on the RT thread (`ActiveSample.position`, `mixer.rs:115`) but is private to the audio callback's
`MixerState` (D20) — the control-side `StatusSnapshot` is rebuilt by `refresh_snapshot` (`main.rs:835`) and the
audio thread never touches it, so position is structurally absent from `SampleStatus` (`http/mod.rs:25-36`).

The naive fix — have the control thread reach into the mixer to read positions — is forbidden: the Sprint-5 RT
engine made the audio callback own `MixerState` lock-free, and the control thread is barred from locking the RT
mutex (D22a, `rt_engine.rs:4-9`). So the value must be **published outward** by the RT thread into atomics the
control thread can read without locking, exactly as `clip_count` (`mixer.rs:1167`) and `xruns` (`engine.rs:439`)
already do. And per the partner's performance constraint (DW3), even that publish must not run unless someone is
watching: telemetry is opt-in **and** subscriber-gated, so the off state is genuinely free. This sprint closes
the position gap; it also closes the lingering Sprint W5 F3 item by carrying an explicit is-windowed flag on
`/status/samples`, so the UI can gate seek/speed/reverse on streamed voices from the status payload instead of
inferring it.

## Scope

**In scope**

- **A telemetry-enable mechanism (DW3).** OFF by default; an explicit opt-in surface (a `POST /telemetry/enable`
  control plus an `AtomicBool telemetry_enabled`), and a subscriber gate so the publish only runs when opted in
  **and** ≥1 telemetry client is observing. With telemetry off, neither the RT publish nor the handler read does
  any new work, and `/status/samples` returns `0` as today.
- **An RT position publish (alloc-free, lock-free).** A per-sample relaxed `AtomicUsize` written once per audio
  block at the advance point (`mixer.rs:1112` / `advance_position`, `mixer.rs:605-643`), mirroring `clip_count`
  (`mixer.rs:1167`). 0 alloc / 0 free / no lock on the callback path.
- **An `internal_id`-keyed side-map**, pre-reserved at `AddSample`, read by `handle_samples`. When telemetry is
  on, `handle_samples` reports real `position`, `position_ms`, and `progress_percent = position / total_frames`.
- **An is-windowed flag on `/status/samples`** (closes Sprint W5 F3) so the UI gates transport reliably for
  streamed voices, sourced from the resolved load mode rather than inferred from `total_frames == 0`.
- **Frontend progress + playhead.** Real progress bars on the now-playing board and a live playhead on the
  Sprint-5 scrubber when telemetry is on; a clean "unavailable" fallback (feature-detected) when off.

**Out of scope**

- **Output/input meters and the state-event WebSocket channel** — owned by **Sprint W7** (tracker §Sprint 7);
  coordinate, do not duplicate. This sprint delivers the position atomic that the W7 throttled tick frames will
  carry, and the DW3 opt-in/subscriber gate that W7 reuses; W7 adds the broadcast channel and the meter atomics.
- **`GET /config` and the config tuning panels** — owned by **Sprint W8** (tracker §Sprint 8); coordinate, do
  not duplicate.
- **The throttled ~15–20 Hz control-side broadcaster and the second WebSocket** — owned by **Sprint W7** (DW12).
  This sprint exposes position over the **existing poll** path (`/status/samples`); push delivery is W7.

## Work items

### F1 — Telemetry gating mechanism (DW3)

- **Statement:** Add an opt-in, subscriber-gated telemetry switch. Telemetry is OFF by default; it activates
  only when the operator has explicitly opted in **and** at least one telemetry client is subscribed. When off,
  neither the RT publish (F2) nor the handler read (F3) does any new work, and `/status/samples` reports `0` as
  today.
- **Where:** `src/http/mod.rs` (`AppState` gains a `telemetry_enabled: Arc<AtomicBool>` and a subscriber
  counter `Arc<AtomicUsize>`); `src/http/routes.rs:78-96` (register a `POST /telemetry/enable` command route
  alongside the existing command endpoints); `src/main.rs` (`:549` callback-state wiring) to thread the
  `AtomicBool` into the RT `MixerState` so the publish in F2 can cheaply early-out.
- **Priority:** gates the whole sprint — F2/F3 must read this flag before doing any new work.
- **Rationale:** The partner's explicit constraint (DW3): telemetry "may impact performance", so it must not run
  unless someone is actively watching. The opt-in `AtomicBool` is the cheap RT-readable gate (one relaxed load
  per block when off — no map walk, no atomic stores); the subscriber counter prevents work when opted in but
  nobody is observing. This is the same gate Sprint W7 reuses for meters and the state channel.
- **Approach:** Add `telemetry_enabled: Arc<AtomicBool>` (default `false`) and `telemetry_subscribers:
  Arc<AtomicUsize>` to `AppState` and to the RT `MixerState` (the publish reads the bool; the control side owns
  the subscriber count). `POST /telemetry/enable` sets the bool; the effective gate is `enabled &&
  subscribers > 0`. For this sprint the poll readers (`handle_samples`) count as observers — model the poll
  reader as a transient subscriber (increment on a `/status/samples` read with an `?telemetry=1` opt-in, or hold
  a short TTL after the enable call); the durable WebSocket subscriber model is Sprint W7. **Record the chosen
  opt-in surface**: if it deviates from a `POST /telemetry/enable` + the poll-subscriber model described here
  (e.g. a config flag, or a query parameter), note the deviation and the evidence in `DECISIONS.md` per its
  override rule. Do **not** invent durable WebSocket subscription here — that is W7's seam.

### F2 — RT position publish (alloc-free, lock-free)

- **Statement:** From the audio callback, publish each active sample's current frame position into a per-sample
  relaxed `AtomicUsize`, written once per audio block at the advance point. The write is gated on the F1 flag:
  when telemetry is off, the callback does at most one relaxed `load` of the gate `AtomicBool` and skips the
  store entirely.
- **Where:** `src/audio/mixer.rs:1112` (the per-sample advance call in `mix_audio`), reading the position that
  `advance_position` (`mixer.rs:605-643`) just computed; the atomic lives alongside the sample (a handle on
  `ActiveSample`, or in the side-map of F3 indexed by `internal_id`). Mirror `clip_count`
  (`mixer.rs:1046`/`:1167-1168`) and `xruns` (`engine.rs:421`/`:439`).
- **Severity:** RT-critical — this is the only work item that touches the audio callback.
- **Evidence:** `advance_position` already computes `self.position` each block (`mixer.rs:627`/`:637`); the loop
  at `mixer.rs:1100-1114` already iterates every active sample once per block. Publishing is a single
  `store(self.position, Relaxed)` per sample, structurally identical to the existing `clip_count.fetch_add(…,
  Relaxed)` at `mixer.rs:1168`. No allocation, no free, no lock.
- **Approach:** After the advance at `mixer.rs:1112` (or inside the loop, immediately after `advance_position`),
  if the telemetry gate is set, `store` `sample.position` into the sample's `Arc<AtomicUsize>` with
  `Ordering::Relaxed`. The `Arc<AtomicUsize>` itself is **moved in** with the sample at `AddSample`
  (`rt_engine.rs:36`, the move-in `AudioCommand`) so the callback never allocates it; the control side holds a
  clone (F3). **NEVER** lock the RT mutex from control (D22a); **NEVER** alloc or free on the callback — the
  atomic is pre-allocated control-side and moved in, never constructed on the audio thread. Verify with the
  alloc harness, not by inspection.

### F3 — `internal_id` side-map + handler read

- **Statement:** Maintain a control-side map from `internal_id` to the per-sample position atomic, pre-reserved
  so the RT side never reallocates, populated when a sample is added and dropped when the sample is reaped.
  `handle_samples` reads the live position from this map (when telemetry is on) instead of writing `0`.
- **Where:** `src/http/mod.rs` (`AppState` gains the map, e.g. `positions: Arc<RwLock<HashMap<u64,
  Arc<AtomicUsize>>>>`, keyed by `internal_id`); `src/main.rs` populate at the sample-start sites
  (`:1241`/`:1313` for streamed, the sample-backed start path that builds `SampleStatus`) and remove at the
  reap/finish reconciliation; `src/http/handlers.rs:702-736` (`handle_samples`) to read it.
- **Severity:** correctness — wrong keying or a stale entry yields a wrong or absent position.
- **Evidence:** `handle_samples` builds each sample's JSON from the control-side `SampleStatus`
  (`handlers.rs:707-733`) and currently writes `position: 0`, `position_ms: 0`, `progress_percent: 0.0`
  (`:721-722`/`:730`); `total_frames` is already present on `SampleStatus` (`http/mod.rs:30`), so
  `progress_percent` is a pure division. The control side already owns the `internal_id` keyspace
  (`main.rs:1241` `add_sample_to_voice`, `ctx.playing.insert(status.internal_id, …)` at `main.rs:1313`), so the
  side-map keys align with the snapshot.
- **Approach:** Pre-reserve the map's capacity (the engine's max concurrent samples) so neither side reallocates
  under steady state. At each sample start, after building `SampleStatus`, insert `internal_id ->
  Arc::new(AtomicUsize::new(start_position))` and move a clone of that `Arc` into the `AddSample` payload (F2).
  At finish reconciliation (where the control thread already removes the sample from `ctx.playing`), remove the
  map entry. In `handle_samples`, if telemetry is on, look up the atomic by `internal_id`, `load(Relaxed)` the
  position, and compute `position_ms = position * 1000 / sample_rate` (guarding `sample_rate == 0`, mirroring
  the existing `total_ms` guard at `handlers.rs:711-715`) and `progress_percent = position as f64 /
  total_frames as f64 * 100.0` (guarding `total_frames == 0`, i.e. streamed/unbounded → leave `0.0`). When
  telemetry is off, keep the current `0` literals. Read-only: control **never** locks the RT mutex (D22a) — it
  reads the atomic the RT thread publishes.

### F4 — Is-windowed flag on `/status/samples`

- **Statement:** Carry the resolved load mode on each sample status so the UI reliably gates seek/speed/reverse
  for streamed/windowed voices, instead of inferring it from `total_frames == 0` (which is fragile). Closes the
  Sprint W5 F3 deferral.
- **Where:** `src/http/mod.rs:25-36` (`SampleStatus` gains a `windowed: bool` — or a `load_mode` enum if a third
  state is needed); `src/main.rs:1289-1300` (the streamed-source start path that builds `SampleStatus` with
  `total_frames: 0`) sets it `true`; the sample-backed (full-load) start path sets it `false`;
  `src/http/handlers.rs:707-733` (`handle_samples`) emits the field.
- **Priority:** medium — unblocks reliable Sprint-5 transport gating; without it the UI guesses.
- **Evidence:** Streamed plays already construct a distinct `SampleStatus` with `total_frames: 0`
  (`main.rs:1294`) and forward-only seek (`API-CONTRACT.md §2`, "Unsupported on `mode:stream`"); the load mode
  is resolved on the control thread at play time, so it is known precisely where the status is built — the UI
  should not re-derive it.
- **Approach:** Add `windowed: bool` to `SampleStatus` (`Default` → `false`). Set it `true` at the streamed
  construction site (`main.rs:1289`), `false` on the full-load path. Emit `"windowed": s.windowed` in
  `handle_samples`'s JSON. The Sprint-5 transport UI keys its seek/speed/reverse/loop-crossfade gating off this
  field rather than `total_frames === 0`.

### F5 — Frontend progress + playhead

- **Statement:** Render real progress bars on the now-playing board and a live playhead on the transport
  scrubber when telemetry is on; show a clean "live position unavailable" fallback when off. Feature-detect off
  vs on — never fake or interpolate a position the daemon did not report.
- **Where:** `webui/src/features/dashboard/NowPlayingBoard.tsx` (progress bars per active sample),
  `webui/src/features/transport/SampleTransport.tsx` (the live playhead overlaid on the Sprint-5 seek scrubber);
  a telemetry-state hook (`webui/src/features/telemetry/useTelemetry.ts`) that owns the opt-in toggle and reads
  whether position is available; the typed API client (`webui/src/api/`) gains the `POST /telemetry/enable`
  call and the `windowed`/`position`/`position_ms`/`progress_percent` fields on the `/status/samples` shape.
- **Priority:** the user-facing payoff of the sprint.
- **Rationale:** This is the partner's "see sample progress live" goal made real. The board and scrubber from
  Sprints W2/W5 are already in place; this wires the now-real fields into them.
- **Fix:** Add a telemetry opt-in toggle (calls `POST /telemetry/enable`); when on, `NowPlayingBoard` renders an
  MUI `LinearProgress` (determinate, `value={progress_percent}`) per sample and shows `position_ms` / `total_ms`;
  `SampleTransport` overlays a playhead marker on the scrubber at `position_ms / total_ms`. Feature-detect: if
  the daemon reports `position === 0 && progress_percent === 0` for every sample while telemetry is off (or the
  `/telemetry/enable` route 404s on an older daemon), render "live position unavailable" exactly as Sprint W2
  does — do not invent a clock-driven estimate. For `windowed === true` samples, suppress the progress bar/
  playhead (forward-only, no bounded `total_ms`) and keep the Sprint-5 "streamed" badge.

## Caveats (do not chase ghosts / do not break)

- **NEVER lock the RT mutex from control (D22a, `rt_engine.rs:4-9`).** The Sprint-5 engine made the audio
  callback own `MixerState` lock-free; the control thread reads the published atomics, it does not reach into
  the mixer. Any code path where the HTTP handler locks the RT mutex is wrong by construction.
- **NEVER alloc or free on the audio callback — prove it with the alloc harness, not inspection.** The position
  atomic is constructed control-side and **moved in** with `AddSample` (`rt_engine.rs:22-31` move-in variant);
  the callback only `store`s into it. Boxing the atomic, constructing it on the audio thread, or dropping it
  there re-introduces exactly the RT allocation Sprint 5 removed.
- **OFF by default; cost ~nothing when off (DW3).** With telemetry off the callback does at most one relaxed
  `load` of the gate bool and skips the per-sample store; the handler keeps the `0` literals. Do **not** publish
  positions unconditionally and gate only the read — the partner's constraint is about the **RT** cost.
- **Pre-reserve the side-map; no RT-side realloc.** Size the `internal_id → atomic` map (and the `Arc<AtomicUsize>`
  allocations) to the engine's max concurrent samples at startup, so steady-state add/remove does not reallocate
  on a hot path.
- **Do not fake position.** When telemetry is off, or for `windowed`/streamed samples with no bounded total, the
  UI shows "unavailable" — it must not interpolate a fake playhead from wall-clock time. Sample position stays a
  real, daemon-sourced value (the W2 contract).
- **Label the streamed case correctly.** `windowed === true` means forward-only with no bounded `total_ms`;
  gate seek/speed/reverse off `windowed`, not off `total_frames === 0`, and keep the Sprint-5 "streamed" badge.
- **Subscriber gate, not just opt-in.** Telemetry runs only when opted in **and** ≥1 client is observing
  (DW3). Opting in with nobody watching must still be a no-op on the RT path.

## Tasks (ordered, TDD-first)

**Subagents:** the Rust engine work (F1–F4: gate, RT publish, side-map, windowed flag, plus the alloc-harness
and Rust-suite tests) and the frontend work (F5: progress bars, playhead, telemetry toggle, API-client fields)
parallelize cleanly — they meet only at the `/status/samples` JSON shape and the `POST /telemetry/enable`
contract, which tasks 1 and 5 pin down first. Each behavior task writes the failing test first, confirms it
fails, writes the minimum code to pass, then confirms green. Backend tests = the existing Rust suite + the
alloc-counting harness; frontend tests = Vitest + React Testing Library (+ Playwright for E2E).

1. **Telemetry gate (F1) — do this first; everything else reads it.** Write a failing Rust test asserting that,
   with telemetry **off** (default), `handle_samples` returns `position: 0` / `progress_percent: 0.0` on a
   populated snapshot, and that toggling the gate via the enable path flips a populated sample to a non-zero
   position. Add `telemetry_enabled: Arc<AtomicBool>` (+ subscriber counter) to `AppState` (`http/mod.rs`) and
   the `MixerState`, and the `POST /telemetry/enable` route (`routes.rs:78-96`). Confirm green. Record the
   chosen opt-in surface in `DECISIONS.md` if it deviates from the brief.

2. **RT position publish + alloc harness (F2).** Write a failing **alloc-harness** test that runs `mix_audio`
   over several blocks with telemetry on and asserts **0 alloc / 0 free** on the callback path while the
   per-sample atomic advances; add a unit test asserting the atomic equals `ActiveSample.position` after a
   block, and that with telemetry **off** the atomic is not written. Implement the gated `store(self.position,
   Relaxed)` at `mixer.rs:1112`, with the `Arc<AtomicUsize>` moved in via `AddSample`. Confirm the harness shows
   zero alloc/free and the position advances.

3. **`internal_id` side-map + handler read (F3).** Write a failing Rust test that populates the snapshot + the
   side-map for a known `internal_id`, sets the atomic to a frame count, turns telemetry on, and asserts
   `handle_samples` returns the matching real `position`, `position_ms` (guarding `sample_rate == 0`), and
   `progress_percent` (guarding `total_frames == 0`). Pre-reserve the map; insert at the sample-start sites
   (`main.rs:1241`/`:1313`), remove at finish reconciliation; read it in `handle_samples` (`handlers.rs:702-736`).
   Confirm green. Add a test that a reaped sample's entry is removed (no leak, no stale read).

4. **Is-windowed flag (F4).** Write a failing Rust test asserting a streamed-source play yields `windowed: true`
   on `/status/samples` and a full-load play yields `windowed: false`. Add `windowed: bool` to `SampleStatus`
   (`http/mod.rs:25-36`), set it at the streamed (`main.rs:1289`) and full-load construction sites, emit it in
   `handle_samples`. Confirm green.

5. **API client + telemetry hook (F5, Lane A).** Write a failing Vitest test that the typed client emits a
   `POST /telemetry/enable` and parses `windowed`/`position`/`position_ms`/`progress_percent` from a
   `/status/samples` fixture. Extend the client and the `useTelemetry` hook; confirm green.

6. **Now-playing progress bars (F5, Lane A).** Write a failing RTL test: given a fixture sample with telemetry
   on and a non-zero `progress_percent`, `NowPlayingBoard` renders a determinate `LinearProgress` at that value
   and the `position_ms`/`total_ms` text; with telemetry off it renders "live position unavailable" (the W2
   fallback) and **no** progress bar. Implement; confirm green.

7. **Live playhead on the scrubber (F5, Lane A).** Write a failing RTL test: with telemetry on, `SampleTransport`
   overlays a playhead at `position_ms / total_ms` on the Sprint-5 seek scrubber; with `windowed === true` the
   playhead and progress are suppressed and the "streamed" badge remains; with telemetry off it falls back to
   "unavailable". Implement; confirm green.

8. **Playwright headless E2E (F5, Lane A).** Against the mock backend, opt in, assert progress advances across
   polls for a normal sample, assert a windowed sample shows no playhead, and assert opting out returns the
   board to "unavailable". Confirm green.

9. **Lane B + Rust Lane B (real device) and docs.** Run the live-daemon and real-CoreAudio verifications below;
   update `API-CONTRACT.md §5/§7`, `CHANGELOG.md`, and `README.md` per DW3. Append any cross-browser/manual
   steps to `MANUAL-VERIFICATION.md`.

## Files to create / touch

**Create:**
- `webui/src/features/telemetry/useTelemetry.ts` — owns the opt-in toggle + position-availability state.
- `webui/src/features/telemetry/useTelemetry.test.ts` — Vitest unit test for the hook.
- `webui/src/features/dashboard/NowPlayingBoard.progress.test.tsx` — RTL progress-bar / fallback tests.
- `webui/src/features/transport/SampleTransport.playhead.test.tsx` — RTL playhead / windowed-suppression tests.
- `webui/tests/e2e/telemetry-position.spec.ts` — Playwright headless E2E (mock backend).

**Touch:**
- `src/http/mod.rs` — `telemetry_enabled`/subscriber counter + the `internal_id → Arc<AtomicUsize>` side-map on
  `AppState`; `windowed: bool` on `SampleStatus` (`:25-36`); thread the new fields through `start_server`
  (`:111-122`).
- `src/http/routes.rs` — register `POST /telemetry/enable` (`:78-96`).
- `src/http/handlers.rs` — `handle_samples` reads the side-map + telemetry gate and emits real
  `position`/`position_ms`/`progress_percent` + `windowed` (`:702-736`).
- `src/audio/mixer.rs` — the per-sample relaxed-atomic publish at the advance point (`:1112`), gated on the
  telemetry flag, mirroring `clip_count` (`:1167-1168`).
- `src/rt_engine.rs` — move the per-sample `Arc<AtomicUsize>` (and the gate `AtomicBool`) in via the `AddSample`
  payload / callback state (`:33-41`), preserving the move-in / no-RT-free contract.
- `src/main.rs` — populate/drop the side-map at sample start (`:1241`/`:1313`) and finish reconciliation; set
  `windowed` at the streamed (`:1289`) and full-load construction sites; wire the gate into the callback state
  (`:549`).
- `tests/` — the existing Rust suite (`http_api_test.rs` status-value tests) + the alloc-counting harness.
- `webui/src/api/` — the `POST /telemetry/enable` call + the new `/status/samples` fields.
- `webui/src/features/dashboard/NowPlayingBoard.tsx`, `webui/src/features/transport/SampleTransport.tsx`.
- `API-CONTRACT.md` (§5 `/status/samples` fields now real when telemetry on; §7 the position addition lands),
  `CHANGELOG.md`, `README.md`, `docs/webui/MANUAL-VERIFICATION.md`.

## Verification

### Lane A (CI / headless)
- `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) all pass with the new telemetry hook, API
  fields, and components.
- Vitest + RTL: the API client emits `POST /telemetry/enable` and parses `windowed`/`position`/`position_ms`/
  `progress_percent`; `NowPlayingBoard` renders a determinate `LinearProgress` at the fixture `progress_percent`
  with telemetry on and the "live position unavailable" fallback (no bar) with telemetry off; `SampleTransport`
  overlays the playhead at `position_ms / total_ms` on the scrubber, suppresses it for `windowed === true`, and
  keeps the "streamed" badge.
- Playwright headless (mock backend): opt in → progress advances across polls for a normal sample; a windowed
  sample shows no playhead; opt out → board returns to "unavailable".

### Lane B (real browser + live daemon)
- Behind the sidecar against a running daemon: opting in makes the now-playing progress bars and the scrubber
  playhead advance in step with audible playback for a normal sample **and** a looped sample (position wraps at
  the loop boundary, matching `advance_position`'s wrap at `mixer.rs:616-625`).
- A streamed/windowed play shows the "streamed" badge with no playhead (gated off `windowed`, not
  `total_frames`), and seek/speed/reverse stay disabled (Sprint W5 gating still holds).
- Opting out returns every sample to "unavailable" and the polled `/status/samples` shows `position: 0` again.

### Rust Lane A [RA] (Docker gate)
- `cargo build --release` with `-D warnings` is clean; `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` pass; the full Rust suite is green.
- **Alloc harness:** running `mix_audio` over multiple blocks with telemetry **on** shows **0 alloc / 0 free**
  on the callback path while the per-sample atomic advances — the publish is a pure relaxed `store` into a
  moved-in atomic, never a construction/drop on the audio thread.
- Rust unit/integration tests: with telemetry **off** (default), `handle_samples` returns `position: 0` /
  `position_ms: 0` / `progress_percent: 0.0` and the RT path performs no store; with telemetry **on**, a
  populated side-map yields real `position`/`position_ms`/`progress_percent` (guards: `sample_rate == 0` and
  `total_frames == 0` leave the derived fields `0`); a streamed play reports `windowed: true`, a full-load play
  `windowed: false`; a reaped sample's side-map entry is removed (no stale read, no leak). No test path locks
  the RT mutex from control (D22a).

### Rust Lane B [RB] (native macOS real CoreAudio device)
- On the real device, enabling telemetry yields advancing positions for normal and looped samples with **no
  audible regression** (no dropouts, no glitch attributable to the per-block store); the `xruns`/stream-error
  counter does not climb under telemetry-on playback.
- Disabling telemetry restores the no-op RT path (no per-block store) with no audible change; playback is
  identical to the pre-sprint binary with telemetry off.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 6)

- [ ] Telemetry is OFF by default and gated: it activates only when explicitly opted in AND ≥1 telemetry client is subscribed; with telemetry off, the RT path does no new work (DW3) `[RA]`
- [ ] Each active sample publishes its live frame position via a relaxed atomic written once per audio block (mirroring `clip_count`); `handle_samples` reads it; `/status/samples` returns real `position`/`position_ms`/`progress_percent` when telemetry is on `[RA]`
- [ ] The mechanism respects D20/D22a — control never locks the RT mutex; the alloc-counting harness shows 0 alloc / 0 free on the callback path `[RA]`
- [ ] On the real CoreAudio device, enabling telemetry yields advancing positions with no audible regression; disabling restores the no-op path `[RB]`
- [ ] The UI renders real progress bars + a live playhead on the scrubber when telemetry is on, and falls back to "unavailable" when off `[A]`
- [ ] Against a real daemon, progress tracks audibly-correct playback for normal and looped samples `[B]`

## Behavior-change / changelog notes

This sprint changes the daemon's observable surface, so it is changelog-worthy per DW3:

- **New telemetry opt-in control + endpoint.** `POST /telemetry/enable` (the chosen opt-in surface) turns
  telemetry on; it is **OFF by default** and subscriber-gated, so the RT path stays free until someone is
  watching (DW3). Record in `CHANGELOG.md`; document the route and gating in `README.md`.
- **`/status/samples` `position`/`position_ms`/`progress_percent` become real when telemetry is on**
  (additive — they remain `0` when telemetry is off, exactly as today, so no existing client breaks). Update
  `API-CONTRACT.md §5` (these are no longer hard-coded `0`) and §7 (the planned "Live sample position" addition
  lands here, gated per DW3/DW12).
- **New `windowed` field on `/status/samples`** (additive) carrying the resolved load mode, so the UI gates
  seek/speed/reverse for streamed voices from the payload (closes Sprint W5 F3). Document in `API-CONTRACT.md §5`.
- If the opt-in surface deviates from `POST /telemetry/enable` + the poll-subscriber model, record the deviation
  and its evidence in `DECISIONS.md` (per its override rule) and reflect it in `API-CONTRACT.md`.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless) ·
Lane B green (live daemon: progress + playhead track audible normal and looped playback; streamed shows no
playhead; opt-out restores "unavailable") · Rust Lane A green (`cargo build --release` `-D warnings` · clippy ·
fmt · full suite · **alloc harness shows 0 alloc / 0 free on the callback path** · telemetry-off returns `0` and
does no RT work · telemetry-on returns real position/ms/percent + `windowed`) · Rust Lane B green (real
CoreAudio: advancing positions, no audible regression, opt-out restores the no-op path) · control **never**
locks the RT mutex (D20/D22a) · new frontend + Rust tests added (no coverage reduction) · `API-CONTRACT.md`,
`CHANGELOG.md`, and `README.md` updated for the DW3 telemetry opt-in and the now-real `/status/samples` fields ·
any deviation recorded in `DECISIONS.md` · out-of-scope discoveries logged in `docs/bugs.md` (tagged `Sprint
W6 F#`) · committed on the branch with a clear message.
