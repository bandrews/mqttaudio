# Sprint 4 — Channel-Map Matrix Mixer

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0, 3 |
| Effort | M |
| Lanes | A, B |
| Subagents | Optional (grid model / gain controls / emit can parallelize) |

## Goal

Build the program's headline routing affordance: a src×dest matrix grid that lets an operator wire any decoded
source channel to any output channel, set a per-route `gain`, and fire the result as a real play. The grid is
sized from the daemon's live `output_channels` (`/status`), each active cell is a route `{src, dest, gain?}`,
destination columns are labeled by channel aliases, and the whole map emits through `POST /command` — the only
path that carries `channel_map` (the typed `/play` silently drops it, per DW10). "Done and correct" means an
operator can express one-to-many fan-out, many-to-one summing, and partial routing through the grid; sees a
clip-risk badge on every destination that sums more than one source; and is warned about the two footguns the
daemon documents — that per-route `gain` is ignored on streamed plays and that an unknown alias silently aborts
the play with no sound. The emitted JSON must be a faithful, code-accurate `channel_map` that the daemon mixes
exactly as the grid depicts.

This builds directly on Sprint 3's typed API client and raw `/command` path — the matrix is a structured editor
that composes the same `play` command the test-bench can emit by hand. It must not regress the command client,
the selector-warning behavior, or the JSON shapes Sprint 3 locked. It models silence (`channel_map: []`) as
distinct from a default 1:1 routing (`channel_map` omitted) and must never conflate the two, because the daemon
treats them as different plays (`main.rs:1264`).

## Why

"Channel map matrix mixing" is one of the marquee features the program exists to expose, and a matrix grid is
the correct UI for it: routing is inherently a two-axis (source × destination) relation, and a grid is the only
affordance that makes fan-out, summing, and partial maps legible at a glance. Nothing in the program reaches
this surface yet — Sprint 3's raw `/command` editor *can* carry a hand-typed `channel_map`, but it offers no
structure, no validation against known channels, and no visibility into the summing semantics. The daemon's
routing model has sharp edges that a freeform editor cannot surface: mixing is **additive** (`mixer.rs:1326`),
so two sources summed into one destination can clip; per-route `gain` is the documented mitigation but it is
**silently dropped on streamed plays** (`main.rs:1247-1278`); and an **unknown alias aborts the play with a
logged error and a silent early return** (`main.rs:1733-1736`) — the command still returns `200`, but no sound
plays. A matrix that models routes as first-class objects, badges summed destinations, and validates aliases
before emit is the right place to make all of that visible. And because `channel_map` only travels on
`/command` (DW10), this sprint is the only way the routing surface becomes reachable through a usable UI.

## Scope

**In scope**

- **F1 — A src×dest matrix grid** sized from `output_channels` (read from `/status`), with source rows and
  destination columns; toggling a cell creates or removes a route `{src, dest, gain?}`. The grid models the
  three distinct map states: a populated map, an empty map `[]` (explicit silence), and *no map* (omitted →
  daemon default 1:1).
- **F2 — Per-route `gain` controls**, default unity (`1.0`), validated/clamped to `0.0..=8.0` mirroring the
  daemon (`main.rs:1744-1750`); a cell left at unity emits **no** `gain` key.
- **F3 — A summing clip-risk badge** on any destination column that has more than one active source, showing
  the source count so the operator can attenuate per-route before clipping.
- **F4 — Emit `play.channel_map` via `POST /command`** (DW10, never the typed `/play`), surfacing the
  documented caveats (streamed-gain ignored, unknown-alias silent abort, out-of-range silently skipped) as
  inline warnings.
- **F5 — Alias-labeled destination columns** from a manually-entered channel-alias map this sprint; the column
  labels feed alias validation before emit.

**Out of scope** (owned by other sprints — coordinate, do not duplicate)

- **Reading channel aliases from the daemon** (`GET /config`) — owned by **Sprint W8**, coordinate, do not
  duplicate. That endpoint does not exist yet (API-CONTRACT §7); until then this sprint accepts a
  manually-entered alias map and feeds it to both labels and validation. Design the alias-map input so Sprint
  W8 can swap its source from manual entry to `/config` with no UI rework.
- **Per-channel calibration (`channel_volumes`) and bass/LFE crossover config** — owned by **Sprint W8**,
  coordinate, do not duplicate. The matrix routes channels; it does not tune their levels or configure the sub.
- **Transport on the resulting playback** (seek/speed/loop-crossfade over the routed play) — owned by **Sprint
  W5**, coordinate, do not duplicate. This sprint fires the play; controlling it afterward is W5's surface.
- **Live sample position / progress on the routed play** — owned by **Sprint W6** (telemetry); positions are
  hard-coded `0` until then (API-CONTRACT §5). Do not fake progress for routed plays.

## Work items

### F1 — Matrix grid model + sizing

- **Statement:** A src×dest matrix grid, with source rows and destination columns, sized from the daemon's live
  `output_channels`. Each active cell is a route `{src, dest, gain?}`; toggling a cell adds or removes that
  route. The model distinguishes an **empty map `[]`** (explicit silence — no routes) from an **omitted map**
  (the daemon's default 1:1 over decoded channels), because the daemon plays them differently.
- **Where:** new `webui/src/features/matrix/MatrixMixer.tsx` (the grid component) and
  `webui/src/features/matrix/routeModel.ts` (the route/grid model + serializer). Destination count derives from
  `output_channels` on `/status` (API-CONTRACT §5 `/status` row). Source-row count is operator-chosen (decoded
  source channel count is not on `/status`; default to `output_channels` and let the operator add/remove source
  rows).
- **Severity:** core feature — without it the routing surface has no structured UI.
- **Rationale:** Routing is a two-axis relation; the grid is the only affordance that makes fan-out, summing,
  and partial maps legible. The empty-vs-omitted distinction is load-bearing: the daemon resolves `Some(map)`
  through alias lookup and a `None` map to `(0..channels)` 1:1 (`main.rs:1247-1265`); an empty `Some([])`
  resolves to zero routes — silence. Conflating them would emit the wrong play.
- **Approach:** Model the grid as a set of routes keyed by `(src, dest)`; the serializer emits
  `channel_map: [{src, dest, gain?}, …]` ordered deterministically (sort by `dest`, then `src`) so the emitted
  JSON is stable and diff-friendly. Expose three explicit map states in the model — `Default` (omit
  `channel_map` from the command entirely), `Silence` (emit `channel_map: []`), and `Routed` (emit the route
  list) — with a visible control to pick among them; never infer "omitted" from "the grid happens to be empty,"
  since an empty grid is `Silence`, not `Default`. Read `output_channels` via the Sprint 0 client / TanStack
  Query `/status` hook; re-size the destination axis if the daemon reports a different channel count.

### F2 — Per-route gain

- **Statement:** Each active route carries a `gain`, default unity (`1.0`), validated and clamped to the daemon's
  `0.0..=8.0` range. A route left at unity emits no `gain` key (so plain 1:1 maps stay byte-identical to a map
  with no gains).
- **Where:** new `webui/src/features/matrix/MatrixCell.tsx` (per-cell toggle + gain input); clamp/validation
  logic in `routeModel.ts`. Mirrors the daemon clamp at `main.rs:1744-1750`
  (`Some(g) if g.is_finite() => g.clamp(0.0, 8.0), _ => 1.0`).
- **Severity:** core feature — gain is the only mitigation for summing clip (F3).
- **Rationale:** The daemon clamps each route's gain to `[0.0, 8.0]` and treats any non-finite or absent value
  as `1.0` (`main.rs:1744-1750`); a route with no gain leaves the level unchanged. Emitting `gain: 1.0` on every
  cell would be noise and would diverge from the daemon's "no gain key → unity" path, which only calls
  `set_channel_route_gains` when at least one route differs from unity (`main.rs:1763-1764`).
- **Approach:** Default each cell's gain to `1.0`; on input, clamp to `[0.0, 8.0]` and reject non-finite values
  (fall back to `1.0`) exactly as the daemon does. In the serializer, **omit** the `gain` key for any route at
  exactly `1.0`; emit it only for routes the operator changed. Surface the clamp in the UI (e.g. a bounded
  slider/number field) so the operator cannot type a value the daemon would silently reshape.

### F3 — Summing clip-risk indicator

- **Statement:** Any destination column fed by more than one active source carries a clip-risk badge that shows
  the source count, so the operator knows where additive summing can overflow and can attenuate per-route.
- **Where:** `MatrixMixer.tsx` (column header / badge), derived from the route model in `routeModel.ts`.
- **Severity:** core feature — the summing semantics are invisible without it and silently produce clipping.
- **Rationale:** Mixing is additive: the daemon does `output[dest_idx] += blended_val * final_volume *
  route_gain` (`mixer.rs:1326`), so N sources summed into one destination can exceed full scale and clip. The
  daemon counts clips (`clip_count`) but does not refuse the route; the per-route `gain` (F2) is the only
  pre-emptive mitigation. Showing the summed-source count tells the operator both *where* and *how badly* to
  attenuate.
- **Approach:** For each destination, count active source routes; badge any destination with a count `> 1`, and
  render the count in/near the badge. Keep this a *risk* indicator, not a measured value — it cannot know actual
  levels (that needs the W7 meters); label it as a structural clip-risk warning, not a "clipping" assertion. Do
  not block the play; the operator may intend the summing.

### F4 — Emit via `/command` + caveats

- **Statement:** The matrix emits the assembled play through `POST /command` (raw command JSON, DW10), never the
  typed `/play`, and surfaces the daemon's documented routing caveats inline before/at emit.
- **Where:** `MatrixMixer.tsx` → the Sprint 0 client's `command()` method (`webui/src/api/client.ts` from
  Sprint 0). The three caveats trace to daemon code: per-route gain ignored on `mode:stream`
  (`main.rs:1247-1278` — the streamed path builds `resolved_map` but never calls `set_channel_route_gains`);
  unknown alias silently aborts the play (`main.rs:1733-1736`, and the streamed equivalent `main.rs:1256-1259`);
  out-of-range `src`/`dest` routes silently skipped at mix time (`mixer.rs:1263-1265`).
- **Severity:** core feature — and the only correct transport for `channel_map`.
- **Rationale:** The typed `/play` endpoint **silently ignores** `channel_map` (`handlers.rs:181-198`, DW10), so
  routing is unreachable through it. The raw `POST /command` always returns `200 {success:true}` on enqueue and
  does **not** validate the command (API-CONTRACT §1, `handlers.rs:154-175`), so the UI cannot rely on the HTTP
  status to confirm the routing took — making client-side validation and caveat-surfacing the only feedback the
  operator gets before the sound (or silence) happens.
- **Approach:** Build the command as `{"command": "play", "file": …, "channel_map": [...], …}` and POST it to
  `/command` via the client (never `/play`). Before emit: (1) if `mode` is `stream`, warn that per-route `gain`
  is **ignored** — the gains in the grid will not be applied; (2) validate every `src`/`dest` against the known
  alias map (F5) and the numeric index range `0..output_channels`, and **warn on any unknown alias** ("this will
  silently abort the play — no sound") rather than emitting a play the daemon will drop; (3) note that
  out-of-range numeric routes are silently skipped (so a partial map is fine, but a typo'd index just vanishes).
  Do not treat a `200` as success confirmation — surface the caveats and let Lane B confirm audible routing.

### F5 — Alias labels

- **Statement:** Destination columns are labeled by channel aliases drawn from a channel-alias map. This sprint
  takes the map by manual operator entry; Sprint W8 will auto-populate it from `GET /config`.
- **Where:** new alias-map input in `webui/src/features/matrix/` (e.g. `AliasMapInput.tsx`), feeding both the
  column labels in `MatrixMixer.tsx` and the alias validation in F4. The daemon resolves aliases against
  `audio.channel_aliases` (API-CONTRACT §4, `config.rs:99-191`); a `ChannelRef` is a number, a numeric string,
  or an alias string.
- **Severity:** core feature for legibility — and the validation surface that catches the silent-abort footgun.
- **Rationale:** Operators think in named destinations ("left surround", "sub"), not bare indices, and the
  daemon's alias resolution is exactly where the silent-abort happens — an alias the daemon doesn't know aborts
  the play (`main.rs:1733-1736`). A labeled, validated alias map turns that invisible failure into a
  pre-emit warning. Reading aliases from the daemon is W8's job (`GET /config` does not exist yet,
  API-CONTRACT §7), so manual entry is the correct bridge.
- **Approach:** Provide an alias-map editor: `index → alias` entries the operator fills in (or leaves blank to
  fall back to the bare index as the label). Use the map for column headers and for F4's validation (a `dest`
  alias not present in the map, and not a valid numeric index, is "unknown" → warn). Encapsulate the map's
  *source* behind a small interface so Sprint W8 can replace manual entry with a `/config`-derived map with no
  change to labels or validation. Do not fetch `/config` here, and do not stub it — it is W8's endpoint.

## Caveats (do not chase ghosts / do not break)

- **Use `/command`, never the typed `/play`.** The typed `/play` silently drops `channel_map`
  (`handlers.rs:181-198`, DW10). Emitting routing through `/play` produces a 1:1 play with no error — the worst
  kind of silent failure. The matrix must POST raw command JSON to `/command`.
- **Per-route `gain` is ignored on streamed plays — warn, do not silently emit it as if it works.** Only the
  full-load path calls `set_channel_route_gains` (`main.rs:1763-1764`); the streamed path builds the route map
  but drops the gains (`main.rs:1247-1278`). If the play's `mode` is `stream`, the grid's gains will not apply;
  the UI must say so.
- **Unknown alias = silent no-sound, not an error.** An alias the daemon can't resolve logs an error and returns
  early (`main.rs:1733-1736`); the command still returns `200`. Validate `src`/`dest` against the alias map and
  the index range, and warn — never present a `200` as "it played."
- **Empty `channel_map: []` (silence) is NOT the same as omitting `channel_map` (default 1:1).** The daemon
  resolves `None` to 1:1 and `Some([])` to zero routes (`main.rs:1247-1265`). Model and emit these as distinct
  states; do not infer "omitted" from an empty grid.
- **Out-of-range numeric routes are silently skipped, not errors.** `if src_ch >= buffer_channels || dest_ch >=
  output_channels { continue; }` (`mixer.rs:1263-1265`). A partial map is legitimate; a typo'd index just
  disappears. Warn on indices outside `0..output_channels`, but do not block — the operator may be partially
  routing on purpose.
- **`output_channels` is on `/status`; decoded *source* channel count is not.** Size the destination axis from
  `/status`; do not invent a source-channel count from the daemon (it isn't exposed) — let the operator size the
  source axis, defaulting to `output_channels`.
- **Do not read or stub `GET /config`.** It does not exist yet (API-CONTRACT §7); aliases come from manual entry
  this sprint. Faking a `/config` response would be a lie the W8 sprint then has to unwind.
- **This sprint touches no Rust.** It composes an existing `play` command. Do not modify the daemon; if you find
  a routing bug in the daemon, log it in `docs/bugs.md` tagged `Sprint W4 F#` rather than fixing it here.

## Tasks (ordered, TDD-first)

Subagents may parallelize along the three natural seams — the grid/route model (F1), the per-route gain controls
(F2), and the emit-plus-caveats path (F4/F5) — but the route model (Task 1–2) must land first because the gain
controls and the serializer both depend on its shape. Frontend tests are Vitest + React Testing Library for
components and the model; the live-routing check is a Playwright/manual Lane B step. Each behavior task writes
the failing test first, confirms it fails, writes the minimum code to pass, and confirms green.

1. **Route model + serializer (F1).** Write failing Vitest tests on `routeModel.ts`: toggling a cell adds/removes
   a `{src, dest}` route; the serializer emits a deterministically-ordered `channel_map`; the three map states
   serialize distinctly — `Default` omits `channel_map` entirely, `Silence` emits `channel_map: []`, `Routed`
   emits the route list. Confirm they fail (no model yet), implement the minimum model + serializer, confirm
   green.

2. **Grid sizing from `output_channels` (F1).** Write a failing RTL test that mounts `MatrixMixer` with a mocked
   `/status` returning a given `output_channels` and asserts the destination-column count matches (and re-sizes
   when the mock changes). Confirm it fails, wire the `/status` hook + sizing, confirm green.

3. **Per-route gain clamp + omit-at-unity (F2).** Write failing tests: a cell's gain defaults to `1.0`; an input
   of `9` clamps to `8.0` and `-1` clamps to `0.0`; a non-finite input falls back to `1.0`; a route at exactly
   `1.0` emits **no** `gain` key while a route at `2.0` emits `gain: 2.0`. Confirm they fail, implement
   `MatrixCell` + the serializer's omit-at-unity rule, confirm green.

4. **Summing clip-risk badge (F3).** Write a failing RTL test: routing two sources into one destination renders
   a clip-risk badge on that destination showing source count `2`; a single-source destination shows no badge.
   Confirm it fails, implement the per-destination count + badge, confirm green.

5. **Emit via `/command`, not `/play` (F4).** Write a failing test asserting the matrix calls the client's
   `command()` (POST `/command`) with `{"command":"play", …, "channel_map":[…]}` and **never** calls the typed
   `play()`/`/play` path. Confirm it fails, wire the emit to `client.command()`, confirm green.

6. **Streamed-gain caveat (F4).** Write a failing test: with `mode: "stream"` selected and at least one
   non-unity route gain, the UI shows the "per-route gain is ignored on streamed plays" warning; with `mode`
   unset/`auto` it does not. Confirm it fails, implement the conditional warning, confirm green.

7. **Alias map + unknown-alias validation (F5 + F4).** Write failing tests: destination columns render the
   operator-entered aliases (blank → bare index); a `dest` alias not in the map and not a valid numeric index
   triggers the "unknown alias — silently aborts the play, no sound" warning before emit; a valid alias or a
   valid index does not. Confirm they fail, implement `AliasMapInput` + the validation, confirm green.

8. **Out-of-range + empty-vs-omitted guardrails (F1 + F4).** Write failing tests: a numeric route index `>=
   output_channels` raises the "silently skipped" warning (non-blocking); switching to `Silence` emits
   `channel_map: []` while `Default` omits it. Confirm they fail, implement the guard + state control, confirm
   green.

9. **Playwright headless emit assertion (Lane A E2E).** Add a Playwright test against the mock backend: build a
   fan-out + a summed destination in the grid, click emit, and assert the captured request hit `/command` (not
   `/play`) with the exact expected `channel_map` JSON (including omitted gains at unity). Confirm green.

10. **Lane B live-routing check.** Against a real multichannel device behind the sidecar, emit a routed play and
    confirm by ear/scope that audio lands on the intended channels (and that a deliberately-unknown alias plays
    nothing, matching the warning). Record the steps in `MANUAL-VERIFICATION.md` if any are not automatable.

## Files to create / touch

- **Create:**
  - `webui/src/features/matrix/MatrixMixer.tsx` — the grid component (sizing, clip-risk badges, emit).
  - `webui/src/features/matrix/MatrixCell.tsx` — per-cell route toggle + gain control.
  - `webui/src/features/matrix/routeModel.ts` — route/grid model, clamp logic, and the `channel_map` serializer.
  - `webui/src/features/matrix/AliasMapInput.tsx` — manual alias-map editor (W8 swaps its source later).
  - `webui/src/features/matrix/__tests__/routeModel.test.ts`,
    `webui/src/features/matrix/__tests__/MatrixMixer.test.tsx`,
    `webui/src/features/matrix/__tests__/MatrixCell.test.tsx` — Vitest + RTL.
  - `webui/e2e/matrix-mixer.spec.ts` — Playwright headless emit assertion against the mock backend.
- **Touch:**
  - `webui/src/App.tsx` (or the route/nav registry) — mount the matrix view.
  - `webui/src/api/client.ts` — only if the Sprint 0 `command()` signature needs a thin convenience for `play`;
    do **not** add a typed channel-map endpoint (DW10).
  - `webui/src/api/status.ts` (the Sprint 2 `/status` hook) — read `output_channels` for grid sizing.
  - `MANUAL-VERIFICATION.md` — append the Lane B multichannel-routing step (V-#) if not fully automatable.

## Verification

### Lane A (CI / headless)
- `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) pass clean.
- **F1:** `routeModel` unit tests prove toggle add/remove, deterministic serialization order, and the three
  distinct map states (`Default` omits `channel_map`; `Silence` → `[]`; `Routed` → route list). The grid
  destination-column count tracks the mocked `output_channels` and re-sizes on change.
- **F2:** gain defaults to `1.0`, clamps to `[0.0, 8.0]`, rejects non-finite (→ `1.0`), and **omits** the
  `gain` key for any route at exactly unity; a changed route emits its `gain`.
- **F3:** a destination with ≥2 active sources renders the clip-risk badge with the correct source count;
  single-source and zero-source destinations render none.
- **F4:** emit calls the client's `command()` (POST `/command`) with a `{"command":"play", …,"channel_map":[…]}`
  body and never the typed `/play`; the `mode:stream` gain-ignored warning shows only under `mode:stream`; an
  out-of-range index raises a non-blocking "silently skipped" warning.
- **F5:** destination columns render operator-entered aliases (blank → index); an unknown `dest` alias (not in
  the map, not a valid index) raises the "silently aborts the play — no sound" warning before emit.
- **Playwright (headless, mock backend):** a built fan-out + summed-destination grid, on emit, produces exactly
  the expected `channel_map` JSON at `/command` (gains omitted at unity), asserted against the captured request.

### Lane B (real browser + live daemon)
- Against a **real multichannel device** behind the sidecar, a routed play **lands on the intended channels** —
  a fan-out routes one source to multiple speakers; a partial map leaves un-routed destinations silent; a summed
  destination is audible (and, if intentionally hot, clips as the badge warned). Verified by ear/scope, not by
  the HTTP `200`.
- A deliberately **unknown alias** produces **no sound** (matching the F4 warning), confirming the silent-abort
  caveat is real and the warning is honest.
- On a `mode:stream` routed play, the per-route gains are **not** applied (matching the F4 streamed-gain
  caveat) — confirm the warning fired and the audible level reflects unity gains.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 4)

- [ ] A src×dest matrix grid sized from `output_channels` (`/status`) lets the user toggle routes and set a per-route `gain`; channel aliases label destination columns `[A]`
- [ ] Destinations with multiple summed sources carry a clip-risk badge; one-to-many fan-out and partial routing are supported `[A]`
- [ ] The matrix emits a play via `/command` (not typed `/play`) and surfaces the caveats: per-route gain is ignored on `mode:stream`, and an unknown alias silently aborts the play `[A]`
- [ ] Against a real multichannel device, a routed play lands on the intended channels `[B]`

## Behavior-change / changelog notes

None. This sprint is pure frontend: it composes an existing `play` command through `POST /command` and changes
no daemon behavior, no endpoint, and no observable output of the daemon. No `CHANGELOG.md` entry and no DW#
behavior decision are required. (The caveats it surfaces — DW10's `/command`-only routing, the streamed-gain
drop, the unknown-alias abort — are existing daemon behaviors documented in API-CONTRACT §4, not changes this
sprint makes.)

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL for the route model,
grid, cell, gain clamp, clip-risk badge, emit path, and alias validation · Playwright headless asserting the
exact `channel_map` JSON hits `/command` not `/play`) · Lane B green on a real multichannel device (routed play
lands on the intended channels; unknown alias plays nothing; streamed-gain caveat confirmed) · the three
documented caveats surfaced in the UI (streamed-gain ignored, unknown-alias silent abort, out-of-range skipped)
· empty-vs-omitted modeled and emitted distinctly · new tests added with no coverage reduction · no daemon
change and therefore no changelog entry · any out-of-scope routing discovery logged in `docs/bugs.md` tagged
`Sprint W4 F#` · committed atomically on the sprint branch with a clear message.
