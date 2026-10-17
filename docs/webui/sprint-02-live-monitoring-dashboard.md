# Sprint 2 — Live Monitoring Dashboard (Poll-Based)

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0, 1 |
| Effort | M |
| Lanes | A, B |
| Subagents | Optional (header / now-playing / racks / cache parallelize) |

## Goal

Build the monitoring core of the web app entirely within **today's** read endpoints: a persistent health
header, a now-playing board, voices and inputs racks, and a cache table — all poll-driven per DW6, with honest
labels and **no faked progress**. "Done and correct" means an operator opens the SPA against a live daemon and
sees the real, current state of playback (active counts, output channels, uptime, clip/stream-error activity,
cache memory budget, the active samples and their static metadata, every voice's volume and ducking state,
every input's volume and inferred mute state) refresh on a poll cadence — and sees an *explicit* "live position
unavailable" placeholder where the daemon does not yet expose progress, not an invented bar. This is most of the
"see what's happening" experience, and it ships **without any daemon change**.

It builds on Sprint 0's typed `DaemonConnection` + API client (the only way these components reach the daemon,
per DW2) and Sprint 1's reverse-proxy sidecar + same-origin transport (Lane B runs the SPA through that proxy
against a real daemon). It must not break either: components consume the Sprint 0 client and the TanStack Query
layer (DW4); none import `fetch`/`WebSocket` directly. Nothing here touches the Rust tree, so there are no Rust
lanes and no behavior-change notes — this is a read-only view over existing endpoints.

## Why

The daemon already exposes real, current state through `/status`, `/status/samples`, `/status/voices`,
`/status/inputs`, `/status/cache`, and `/metrics` (`routes.rs:101-108`). Sprints 0 and 1 made that surface
reachable and typed; nothing yet *renders* it. This sprint closes that gap: it is the first sprint that shows a
human what the daemon is doing, and it is achievable purely against the live-pollable read model (DW6) — no
telemetry opt-in, no RT changes, no config endpoint.

The reason this needs care rather than a naive table dump is **honesty**. Three traps in the current daemon
surface will produce a lying UI if rendered literally:

- `/status/samples` **hard-codes `position`, `position_ms`, and `progress_percent` to `0`**
  (`handlers.rs:721-722`, `:730`) until telemetry lands in Sprint 6. A progress bar bound to those fields would
  animate a fake "0% forever" or, worse, be misread as "stuck at the start." The board must say *live position
  unavailable* and point at Sprint 6 — never render a bar.
- `clips` and `xruns` from `/metrics`/`/status` are **cumulative counters**, not instantaneous values
  (`API-CONTRACT.md` §5 label hygiene). Showing the raw counter as a "current rate" is a lie; rates must be
  diffed client-side across successive polls (DW6 label hygiene).
- `xruns` counts cpal **fatal stream-error/rebuild** callbacks (`engine.rs:437-442`), **not** per-buffer
  underruns. Labeling the badge "buffer xruns" or "underruns" misrepresents what fired. It must read "stream
  errors / rebuilds."

A fourth, smaller trap: `/status/inputs` reports `muted` as a **derived** value (`muted = volume == 0.0`,
`handlers.rs:793`), not a stored mute flag. That is fine to show, but the UI must surface the caveat so an
operator does not mistake "volume turned to 0" for an explicit mute toggle.

## Scope

**In scope**

- **A persistent health header** (always visible) showing active samples/voices/inputs, `output_channels`, and
  uptime, plus a clip badge (cumulative clips) and a stream-error badge — built from `/status` + `/metrics`.
- **A cache memory-budget gauge**: resident `memory_bytes` against `memory_cap_bytes` with `memory_headroom_bytes`
  (a `null` cap renders as "unlimited"), with `disk_bytes` shown alongside — from `/metrics` `cache.*`.
- **A now-playing board** from `/status/samples`: per-sample static metadata, with progress rendered as an
  explicit placeholder pending Sprint 6 (no faked bar).
- **A voices rack** from `/status/voices`: `volume` and `ducking_multiplier` with a "ducked" indicator when the
  multiplier is below `1.0`.
- **An inputs rack** from `/status/inputs`: `volume` and `muted` (with the inferred-mute caveat surfaced).
- **A cache table** from `/status/cache` (and `/metrics`): memory/disk entries and sizes.
- **Client-side rate computation**: diff successive `/metrics` polls for clip and stream-error rates, never
  presenting a cumulative counter as instantaneous.

**Out of scope** (named owner — coordinate, do not duplicate)

- **Per-sample transport (seek/speed) + voice/input control strips** — owned by **Sprint W5**, coordinate, do
  not duplicate. This sprint *displays* voices/inputs/samples; it does not emit `seek`/`speed`/`voice_*`/`input_*`
  commands.
- **Editing config** (ducking rules, crossover, aliases, calibration, macros, input definitions) — owned by
  **Sprint W8**, coordinate, do not duplicate. No `GET /config` consumption here (it does not exist yet,
  `API-CONTRACT.md` §7).
- **Real live position + meters** — owned by **Sprints W6/W7**, coordinate, do not duplicate. The progress
  placeholder this sprint renders is replaced by W6's real atomics; meters arrive in W7. Do not stub either.

## Work items

### F1 — Persistent health header

- **Statement:** A health header is rendered at all times (independent of which view is active) showing the
  active sample/voice/input counts, `output_channels`, uptime, a clip badge, and a stream-error badge.
- **Where:** `webui/src/features/dashboard/HealthHeader.tsx`; data from `/status` (`handle_status`,
  `handlers.rs:660`) and `/metrics` (`handle_metrics`, `handlers.rs:82`) via the Sprint 0 typed client + a
  TanStack Query hook polling at 1–2 s (DW6).
- **Priority:** core — this is the always-on "is it alive and what is it doing" strip.
- **Rationale:** `/status` carries `active_samples/active_inputs/active_voices` and `output_channels`
  (`handlers.rs:683`); `/metrics` carries `uptime_seconds` plus the cumulative `clips`/`xruns`. There is no
  single endpoint with all of it, so the header composes both. Always-visible because connection health and clip
  activity are the operator's first signal something is wrong.
- **Approach:** Compose one header component fed by both queries. Counts and `output_channels` come from
  whichever query resolves; prefer `/metrics` where fields overlap (it is the richest surface,
  `API-CONTRACT.md` §5). Render uptime humanized from `uptime_seconds`. The **clip badge** shows the cumulative
  `clips` count and, alongside it, the diffed clip *rate* from F7 (clearly labeled as a rate). The **stream-error
  badge** shows cumulative `xruns` and its diffed rate, and **must be labeled "stream errors / rebuilds"**, never
  "buffer xruns" / "underruns" (`engine.rs:437-442`). When a poll fails (daemon unreachable), the header surfaces
  a connection-degraded state rather than stale numbers presented as live.

### F2 — Cache memory-budget gauge

- **Statement:** A gauge renders resident cache memory against its cap with remaining headroom; a `null` cap
  renders as "unlimited"; disk usage is shown alongside.
- **Where:** `webui/src/features/dashboard/CacheBudgetGauge.tsx`; from `/metrics` `cache.*`
  (`handlers.rs:138-142`).
- **Priority:** core observability — the memory backstop is the load-bearing safety property of the streaming
  redesign, and an operator must see how close it is running to the cap.
- **Rationale:** `/metrics` `cache` exposes `memory_bytes`, `memory_entries`, `memory_headroom_bytes`,
  `memory_cap_bytes` (`null` = unlimited), and `disk_bytes` (`handlers.rs:138-142`,
  `API-CONTRACT.md` §5). This is the only place the cap + headroom are published.
- **Approach:** An MUI `LinearProgress` (determinate) when `memory_cap_bytes` is a number:
  `value = memory_bytes / memory_cap_bytes * 100`, with `memory_headroom_bytes` shown as the remaining figure
  and human-readable byte formatting throughout. When `memory_cap_bytes` is `null`, render an **indeterminate /
  "unlimited"** treatment (no fill-to-cap math) and show resident bytes + entry count without a percentage. Show
  `disk_bytes` as a secondary figure beside the gauge. Do **not** infer a cap when one is absent.

### F3 — Now-playing board (no faked progress)

- **Statement:** A board lists every active sample with its static metadata; progress is rendered as an explicit
  "live position unavailable" placeholder pointing at Sprint 6 — **not** a progress bar.
- **Where:** `webui/src/features/dashboard/NowPlayingBoard.tsx`; from `/status/samples` (`handle_samples`,
  `handlers.rs:702`).
- **Priority:** core — this is the literal "what is playing right now" view.
- **Rationale:** `/status/samples` returns `internal_id, id, voice, file, total_frames, total_ms, sample_rate,
  volume, voice_volume, speed, loop_mode` per active sample (`API-CONTRACT.md` §5). But `position`, `position_ms`,
  and `progress_percent` are **hard-coded `0`** (`handlers.rs:721-722`, `:730`) until telemetry lands. Binding a
  bar to those fields would render a permanent fake. (Note `internal_id` is already stringified by the daemon,
  `API-CONTRACT.md` §3 — keep it a string.)
- **Approach:** One card/row per sample showing `file`, `voice`, `total_ms` (humanized), `volume`, `voice_volume`,
  `speed`, and `loop_mode`. In place of a progress bar, render a fixed informational element reading
  **"live position unavailable — enable telemetry (Sprint W6)"**. Do **not** read `position`/`progress_percent`
  from the payload to drive any bar, and do **not** synthesize progress from wall-clock + `total_ms` (that would
  be a fabricated value, and it ignores speed/loop/seek). The board updates on the 1–2 s poll; samples that
  disappear from the payload are removed.

### F4 — Voices rack

- **Statement:** A rack lists every voice with its id, sample count, volume, and ducking multiplier, flagging
  voices that are currently ducked.
- **Where:** `webui/src/features/dashboard/VoicesRack.tsx`; from `/status/voices` (`handle_voices`,
  `handlers.rs:738`).
- **Priority:** core — ducking visibility is a primary reason the partner wants this UI.
- **Rationale:** `/status/voices` returns per-voice `id, sample_count, volume, ducking_multiplier`
  (`handlers.rs:753`), where `1.0` means "not ducked" and a value below `1.0` means active attenuation
  (`API-CONTRACT.md` §5). Without a "ducked" indicator the multiplier is an opaque number.
- **Approach:** One row per voice: `id`, `sample_count`, `volume`, and `ducking_multiplier`. Render a **"ducked"
  indicator** (badge/chip) when `ducking_multiplier < 1.0`, and show the multiplier value so the operator sees
  *how much*. No control affordances here (that is Sprint W5) — display only. Updates on the 1–2 s poll.

### F5 — Inputs rack

- **Statement:** A rack lists every live input with its index, voice id, volume, channel count, and mute state,
  with the inferred-mute caveat surfaced.
- **Where:** `webui/src/features/dashboard/InputsRack.tsx`; from `/status/inputs` (`handle_inputs`,
  `handlers.rs:781`).
- **Priority:** core — inputs are part of the daemon's full feature set this app must exercise.
- **Rationale:** `/status/inputs` returns per-input `index, voice_id, volume, channels, muted`
  (`API-CONTRACT.md` §5). **`muted` is not a stored flag — it is derived as `volume == 0.0`**
  (`handlers.rs:793`). Presenting it as an explicit mute toggle would mislead; an operator who sets volume to 0
  is not the same as one who pressed mute, but the daemon cannot tell them apart.
- **Approach:** One row per input: `index`, `voice_id`, `volume`, `channels`, and a mute state derived from the
  `muted` field. Surface the caveat (a tooltip/help affordance, or a footnote on the rack) stating mute is
  inferred from `volume == 0.0` and is not a distinct stored state. Display only (control is Sprint W5). Updates
  on the 1–2 s poll.

### F6 — Cache table

- **Statement:** A table lists memory and disk cache entries with their sizes.
- **Where:** `webui/src/features/dashboard/CacheTable.tsx`; from `/status/cache` (and `/metrics` `cache.*`).
- **Priority:** observability — complements the F2 budget gauge with the entry-level breakdown.
- **Rationale:** `/status/cache` returns memory/disk `{entries, size_bytes, size_mb}`
  (`API-CONTRACT.md` §5), polled at 2–5 s (the slower cadence, DW6) since cache contents change less often than
  playback state.
- **Approach:** A two-section (memory / disk) table or summary showing `entries` and both `size_bytes` and
  `size_mb`, with consistent human-readable byte formatting matching F2. Poll at 2–5 s per DW6 (distinct from the
  1–2 s status cadence) to avoid over-polling a slow-moving surface. This is the entries view; the
  budget-vs-cap gauge is F2.

### F7 — Client-side rate computation

- **Statement:** Clip and stream-error *rates* are computed by diffing successive `/metrics` polls; cumulative
  counters are never presented as instantaneous values.
- **Where:** `webui/src/state/rates.ts` (a small derivation module/hook consumed by F1's badges).
- **Priority:** correctness / label hygiene — this is the mechanism that keeps F1 honest.
- **Rationale:** `clips` and `xruns` are **cumulative** counters (`API-CONTRACT.md` §5 label hygiene; the daemon
  increments `xruns` with `fetch_add` at `engine.rs:439`). The only honest way to show "activity" is the delta
  between two timestamped samples divided by the elapsed interval — never the raw counter dressed up as a rate
  (DW6 label hygiene).
- **Approach:** Keep the previous `/metrics` sample (counter value + timestamp). On each new poll, compute
  `rate = max(0, current − previous) / elapsed_seconds` for `clips` and `xruns` independently. Clamp negatives to
  zero (guards a daemon restart resetting the counter). Expose both the **cumulative total** and the **derived
  rate** as distinct, distinctly-labeled values so the UI can show the total as a count and the rate as
  "per second / per minute" without conflating them. The first poll has no predecessor, so render the total and
  an explicit "rate pending" until a second sample exists. No timer of its own — it derives off the existing
  query cadence.

## Caveats (do not chase ghosts / do not break)

- **Do NOT fake position/progress.** `/status/samples` returns `position`/`position_ms`/`progress_percent`
  hard-coded to `0` (`handlers.rs:721-722`, `:730`). Render an explicit "live position unavailable (Sprint W6)"
  placeholder. Do not bind a bar to the zeroed fields, and do not synthesize progress from wall-clock — that is a
  fabricated number and it ignores speed/loop/seek.
- **Label `xruns` correctly.** They are **stream-error / rebuild** events (`engine.rs:437-442`), not per-buffer
  underruns. The badge text must say "stream errors / rebuilds." Do not call them "xruns," "buffer underruns," or
  "dropouts" in the UI copy.
- **`muted` is inferred** as `volume == 0.0` (`handlers.rs:793`), not a stored mute flag. Surface the caveat;
  do not present it as a definitive mute toggle distinct from volume.
- **Counters are cumulative** (`clips`, `xruns`). Diff successive polls for rates (F7); never show a raw
  cumulative counter labeled as an instantaneous/per-second value (DW6).
- **Display only, no control.** This sprint must not emit `seek`/`speed`/`voice_*`/`input_*`/`cache_*` commands —
  those control strips are Sprint W5. Adding even a "stop" button here duplicates W5 and breaks the scope split.
- **No daemon change.** Do not touch `src/`. If you find a real bug in a read handler, log it in `docs/bugs.md`
  tagged `Sprint W2 F#` per the Charter; do not fix it inline.
- **Go through the transport.** No component may import `fetch`/`WebSocket` directly (DW2) — everything routes
  through the Sprint 0 `DaemonConnection` + typed client. Do not add a second HTTP path.
- **Respect the poll cadences** (DW6): `/status*` and `/metrics` at 1–2 s, `/status/cache` at 2–5 s. Do not
  hammer the daemon faster "to feel live."

## Tasks (ordered, TDD-first)

**Subagents grouping (optional, per the metadata):** F1 (header) / F3 (now-playing) / F4+F5 (racks) /
F2+F6 (cache) can be built in parallel by separate subagents once Task 1 (the shared query hooks) and Task 2
(F7 rates) exist, since they consume disjoint components and endpoints. Keep one owner for `state/` to avoid
conflicting query-key conventions. Every behavior task below writes the failing Vitest/RTL test first, confirms
it fails, writes the minimum component code to pass, then confirms green. Frontend tests are Vitest + React
Testing Library; the live cross-check is Playwright headless (Lane A, mock backend) and a real-daemon session
(Lane B). No Rust tests — this sprint does not touch the daemon.

1. **Shared polling hooks (foundation).** Write failing RTL tests for thin TanStack Query hooks over the Sprint 0
   client: `useStatus`, `useMetrics`, `useSamples`, `useVoices`, `useInputs`, `useCache`, each asserting it calls
   the typed client method (not `fetch`) and exposes loading/error/data. Confirm fail, implement the hooks with
   the DW6 cadences (status/metrics 1–2 s; cache 2–5 s), confirm green. These are the only daemon access points
   for every component below.

2. **F7 rate derivation.** Write a failing unit test for `state/rates.ts`: feed two timestamped `/metrics`
   samples and assert the diffed clip/stream-error rates; feed a single sample and assert "rate pending"; feed a
   decreasing counter (restart) and assert the rate clamps to `0`, never negative. Confirm fail, implement the
   diff/clamp logic, confirm green.

3. **F1 health header.** Write failing RTL tests against a fixture `/status` + `/metrics` payload: counts,
   `output_channels`, humanized uptime, the clip badge (cumulative + rate from F7), and the stream-error badge —
   asserting its text is **"stream errors / rebuilds"** and is **never** "buffer xruns"/"underruns". Add a test
   for the connection-degraded state on a failed poll. Confirm fail, implement `HealthHeader.tsx`, confirm green.

4. **F3 now-playing board.** Write failing RTL tests: rows render `file`/`voice`/`total_ms`/`volume`/
   `voice_volume`/`speed`/`loop_mode` from a fixture `/status/samples`; **no element with a `progressbar` role
   exists**; the "live position unavailable (Sprint W6)" placeholder is present; a removed sample disappears on
   the next poll. Confirm fail, implement `NowPlayingBoard.tsx`, confirm green.

5. **F4 voices rack.** Write failing RTL tests: rows render `id`/`sample_count`/`volume`/`ducking_multiplier`;
   a voice with `ducking_multiplier < 1.0` shows the "ducked" indicator and a voice at `1.0` does not. Confirm
   fail, implement `VoicesRack.tsx`, confirm green.

6. **F5 inputs rack.** Write failing RTL tests: rows render `index`/`voice_id`/`volume`/`channels`; an input with
   `muted:true` shows muted and one with `muted:false` does not; the inferred-mute caveat is present and
   discoverable. Confirm fail, implement `InputsRack.tsx`, confirm green.

7. **F2 cache budget gauge.** Write failing RTL tests: with a numeric `memory_cap_bytes`, a determinate gauge
   shows the right percentage and `memory_headroom_bytes`; with `memory_cap_bytes: null`, the gauge renders
   "unlimited" with no percentage; `disk_bytes` is shown. Confirm fail, implement `CacheBudgetGauge.tsx`,
   confirm green.

8. **F6 cache table.** Write failing RTL tests: memory and disk sections render `entries`, `size_bytes`, and
   `size_mb` from a fixture `/status/cache`. Confirm fail, implement `CacheTable.tsx`, confirm green.

9. **Dashboard composition + Playwright headless (Lane A).** Compose F1–F6 into the dashboard view. Write a
   Playwright headless E2E against the mock/fixture backend asserting all panels render and that the stream-error
   badge copy and the "live position unavailable" placeholder are present (the two honesty invariants). Confirm
   green.

10. **Green gate + Lane B.** Run `pnpm build`, `tsc --noEmit`, eslint warnings-as-errors, Vitest + RTL, and the
    Playwright headless suite — all green, zero warnings. Then run Lane B (Sprint 1 sidecar + live daemon):
    drive real plays/stops/duck changes and confirm the dashboard reflects them within the poll interval. Record
    any cross-browser/visual notes for `MANUAL-VERIFICATION.md` if they arise (the dashboard itself stays `[A]`).

## Files to create / touch

**Create:**

- `webui/src/features/dashboard/HealthHeader.tsx` (F1)
- `webui/src/features/dashboard/CacheBudgetGauge.tsx` (F2)
- `webui/src/features/dashboard/NowPlayingBoard.tsx` (F3)
- `webui/src/features/dashboard/VoicesRack.tsx` (F4)
- `webui/src/features/dashboard/InputsRack.tsx` (F5)
- `webui/src/features/dashboard/CacheTable.tsx` (F6)
- `webui/src/features/dashboard/Dashboard.tsx` (composition view)
- `webui/src/state/rates.ts` (F7)
- `webui/src/state/queries.ts` (the shared `useStatus`/`useMetrics`/`useSamples`/`useVoices`/`useInputs`/`useCache` hooks)
- Co-located test files: `*.test.tsx` for each component, `rates.test.ts`, and a Playwright spec
  `webui/e2e/dashboard.spec.ts`
- Fixtures under `webui/src/test/fixtures/` (sample `/status`, `/metrics`, `/status/samples|voices|inputs|cache`
  payloads, including a `null`-cap and a ducked-voice case)

**Touch:**

- `webui/src/App.tsx` (mount the persistent `HealthHeader` + the `Dashboard` route)
- Sprint 0's typed client only if a read-endpoint method is missing (it should already cover all read endpoints
  per the Sprint 0 acceptance — if not, that is a Sprint 0 gap to log, not to silently patch here)

## Verification

### Lane A (CI / headless)

- `pnpm build`, `tsc --noEmit`, and eslint **warnings-as-errors** pass with zero warnings.
- **F1:** RTL asserts counts/`output_channels`/uptime render; the clip badge shows cumulative + diffed rate; the
  stream-error badge text is exactly "stream errors / rebuilds" and a snapshot/text assertion proves it is
  **never** "buffer xruns"/"underruns"; a failed poll renders the degraded state.
- **F2:** RTL asserts the determinate gauge math and headroom for a numeric cap; the "unlimited" treatment for a
  `null` cap (no percentage); `disk_bytes` present.
- **F3:** RTL asserts all static fields render; **`queryByRole('progressbar')` returns null** (no fake bar); the
  "live position unavailable (Sprint W6)" placeholder is present; the payload's zeroed `position`/`progress_percent`
  are not bound to any bar.
- **F4:** RTL asserts a `ducking_multiplier < 1.0` row shows the "ducked" indicator and a `1.0` row does not.
- **F5:** RTL asserts mute state derives from `muted`, and the inferred-mute caveat is discoverable.
- **F6:** RTL asserts memory/disk `entries`/`size_bytes`/`size_mb` render.
- **F7:** unit tests assert correct diffed rates across two samples, "rate pending" on the first, and a
  non-negative clamp on a counter reset.
- **Playwright headless (mock backend):** the composed dashboard renders all panels; the stream-error label and
  the position-unavailable placeholder are present (the two honesty invariants); the suite is green.

### Lane B (real browser + live daemon)

- Served through the Sprint 1 sidecar against a running `mqttaudio` daemon: the dashboard reflects **live**
  plays, stops, and ducking changes within the poll interval (1–2 s for status/metrics). Concretely: start a
  play → it appears on the now-playing board and the active-sample count increments; stop it → it disappears;
  trigger a duck → the affected voice shows the "ducked" indicator and a `ducking_multiplier < 1.0`; precache a
  file → the cache table entry count and the F2 budget gauge move.
- The clip / stream-error badges show cumulative totals from the real daemon and a diffed rate that returns to a
  resting value when no new events occur (proving F7 is diffing, not echoing the counter).
- The cache budget gauge renders the daemon's real `memory_cap_bytes` (or "unlimited" if the running config sets
  no cap) with live `memory_bytes`/`memory_headroom_bytes`.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 2)

- [ ] A persistent health header shows active samples/voices/inputs, output channels, uptime, a clip badge, and a stream-error badge (labeled "stream errors / rebuilds", not "buffer xruns") from `/status` + `/metrics` `[A]`
- [ ] A cache memory-budget gauge renders resident bytes vs cap with headroom from `/metrics` (`null` cap → "unlimited") `[A]`
- [ ] A now-playing board lists active samples from `/status/samples`; progress is shown as "live position unavailable" pending telemetry (Sprint 6), not faked `[A]`
- [ ] Voices rack (`/status/voices`, volume + `ducking_multiplier` with a "ducked" indicator) and inputs rack (`/status/inputs`, volume + `muted`) render from live data `[A]`
- [ ] Clip/stream-error rates are computed client-side by diffing successive `/metrics` polls (cumulative counters are not shown as instantaneous) `[A]`
- [ ] Against a real daemon, the dashboard reflects live plays/stops/duck changes within the poll interval `[B]`

## Behavior-change / changelog notes

None. This sprint is a read-only dashboard composed over existing daemon endpoints (`/status`, `/status/samples`,
`/status/voices`, `/status/inputs`, `/status/cache`, `/metrics`); it adds no daemon code, no endpoint, and no
observable daemon behavior. There is nothing to record in `CHANGELOG.md` and no `DW#` behavior decision is
exercised beyond the read-model and label-hygiene rulings (DW6) that this sprint *consumes*. If implementation
uncovers a real bug in a read handler, log it in `docs/bugs.md` tagged `Sprint W2 F#` — do not fix the daemon
here.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless
against the mock backend) with **zero** build/type/lint warnings · every dashboard component (F1–F6) and the
rate derivation (F7) covered by new Vitest/RTL/unit tests, no coverage reduction · the two honesty invariants
proven by test (the now-playing board renders **no** `progressbar` and shows "live position unavailable"; the
stream-error badge reads "stream errors / rebuilds", never "buffer xruns") · all daemon access through the
Sprint 0 `DaemonConnection`/typed client (no direct `fetch`/`WebSocket`, DW2) · DW6 poll cadences honored
(status/metrics 1–2 s, cache 2–5 s) · Lane B green against a live daemon behind the Sprint 1 sidecar (plays/
stops/duck changes reflected within the poll interval) · no `src/` change and any read-handler bug logged to
`docs/bugs.md` (no `CHANGELOG.md` entry, none warranted) · committed on the branch with a clear message.
