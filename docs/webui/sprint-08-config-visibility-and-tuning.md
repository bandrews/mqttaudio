# Sprint 8 — Config Visibility & Tuning Panels

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0, 2 |
| Effort | L |
| Lanes | A, B, RA |
| Subagents | Optional (the `GET /config` endpoint and the frontend panels parallelize) |

## Goal

"Done and correct" is a daemon that can be *read* — a read-only `GET /config` returning the running
configuration with every secret redacted — and a set of UI panels that **display** those running values and
**emit** validated, restart-required config-JSON snippets the operator applies by hand and reboots the daemon.
The endpoint serializes the live `Config` with `mqtt.password` (`config.rs:23`) and `http.auth_token`
(`config.rs:620`) omitted or masked, and is gated exactly like the other status routes — open by default,
covered by `require_auth` when set (DW11). On top of it sit panels for ducking rules, the bass/LFE crossover,
channel aliases, per-channel calibration, the limiter ceiling + master gain, macros, and input definitions;
each validates its edits against a schema mirrored from `config.rs` and produces a JSON snippet flagged
"restart required" (DW8). The two genuinely-live knobs — a running input's `volume` and `mute`
(`main.rs:2020-2040`) — are wired to apply *immediately* and are visually distinct from the restart-required
editors. Ducking is visualized live: the `/metrics` `ducking` map and `/status/voices` `ducking_multiplier`
drive a "firing" indicator on the voice strips, with the mic-/voice-activity-trigger limitation surfaced in
the UI so nobody mistakes it for signal-gated ducking.

This builds on Sprint 0's typed client + `DaemonConnection` and Sprint 2's poll-based dashboard (the voice and
input racks, the `/metrics` poll loop). It must not break either: the new `/config` read joins the existing
poll model (DW6) without changing the status payloads Sprint 2 renders, and the daemon's RT audio path is not
touched at all — `GET /config` reads the control-side `Config`, never the mixer. The cardinal rule the whole
sprint defends: **the UI must never imply a config edit applies live except per-input volume/mute** (DW8).

## Why

The partner's ask — "tune gain/ducking/channel map settings" — is impossible to honor honestly today because
**there is no way to read the running config**: no `GET /config` route or handler exists in `src/http/`
(verified — grep for `/config`/`handle_config` returns nothing), and the daemon reads its config exactly once
at startup with no hot-reload, no SIGHUP, and no runtime config-mutation command (DW8, `main.rs:410-421`,
`:584`). A tuning UI that cannot read the current values, or that pretends an edit took effect when the daemon
will not see it until a restart, is worse than no UI — it lies to the operator. This sprint closes that gap on
both ends: it adds the read endpoint (DW11) and it builds editors that are explicit about being
display-plus-emit-snippet-plus-restart.

It also closes coverage that no prior sprint owns. Sprint 2 shows `ducking_multiplier` per voice but does not
relate it to the *rules* that produced it; this sprint adds the rules view and the live "firing" visualization
from the `/metrics` `ducking` map. Sprint 4's matrix mixer has a documented manual-label dependency (its
destination columns need human-typed labels because nothing fed it `audio.channel_aliases`); the
`channel_aliases` this sprint reads from `/config` close that dependency. And the per-input volume/mute that
Sprint 5 wires as transport controls gets its config-panel home here, clearly separated from everything that
needs a restart. Now, because the read endpoint and the panels are independent, the two can be built in
parallel (the `GET /config` shape is the only contract between them).

## Scope

**In scope**

- **A read-only `GET /config`** returning the running config with `auth_token` and `mqtt_password` redacted
  (DW11), gated like the other status routes (open by default; covered by `require_auth` when set). A Rust test
  asserts the secrets are absent from the response.
- **Config panels** that display current values from `/config` and emit validated config-JSON snippets flagged
  "restart required" for: ducking rules, bass/LFE crossover, channel aliases, per-channel calibration, the
  limiter ceiling + master gain, macros, and input definitions (DW8).
- **Live ducking visualization** — rules + resolved multipliers firing live on the voice strips, from the
  `/metrics` `ducking` map and `/status/voices` `ducking_multiplier`, surfacing the mic-/voice-activity-trigger
  (not signal-gated) limitation.
- **Wiring the genuinely-live settings** (per-input `volume`/`mute`) to apply immediately, visually distinct
  from the restart-required editors.

**Out of scope**

- **Applying any *other* config live.** Impossible without a restart — the daemon reads config once at startup
  and has no runtime config-mutation command (DW8). Only per-input volume/mute change live; everything else is
  display + emit-snippet + restart.
- **Hot-reload / file-watch / SIGHUP.** YAGNI and explicitly out of scope (DW8). Do not build it.
- **The voice and input *status* racks themselves** (the read-only `/status/voices` + `/status/inputs`
  rendering) — owned by Sprint W2, coordinate, do not duplicate. This sprint adds the ducking-rules overlay and
  the live per-input volume/mute *controls*, not the racks.
- **The channel-map *matrix* grid** — owned by Sprint W4, coordinate, do not duplicate. This sprint only feeds
  it the alias labels via `/config`; it does not build or move the matrix.
- **The command-emitting forms for input_volume/input_mute as transport controls** — owned by Sprint W5,
  coordinate, do not duplicate. This sprint surfaces the same two live knobs inside the config panel and reuses
  the W3/W5 client methods rather than re-implementing the command JSON.
- **Telemetry atomics / state-event WebSocket** (live position, meters) — owned by Sprints W6/W7, coordinate,
  do not duplicate. Ducking visualization here uses the *already-live* `/metrics` + `/status/voices` poll
  surface, not new telemetry.

## Work items

### F1 — Read-only `GET /config`, secrets redacted (Rust)

- **Statement:** Add a read-only `GET /config` route + handler that serializes the running `Config` with
  `auth_token` and `mqtt_password` (and any future secret) omitted or masked, gated like the existing status
  routes. There is no such endpoint today.
- **Where:** `src/http/routes.rs:101-108` (status-route group — add `/config` here so it inherits the
  `require_auth` gating at `:124-140`); `src/http/handlers.rs` (new `handle_config`, alongside
  `handle_metrics` at `:82`); `src/http/mod.rs:60-90` (`AppState` — add a handle to the running config) and
  `:112-135` (`start_server` signature — thread the running `Config` in); `src/config.rs` (a redacting
  serialize view if the cleanest path is a dedicated struct rather than `#[serde(skip_serializing)]` on the
  secret fields).
- **Severity:** observability / required-for-sprint (the panels cannot read current values without it; DW11).
- **Evidence:** `start_server` (`src/http/mod.rs:112-121`) currently takes only `&HttpConfig`, not the full
  `Config`, so the running config is not reachable from the HTTP layer; it must be threaded in. The secrets to
  redact are `MqttConfig.password` (`config.rs:23`) and `HttpConfig.auth_token` (`config.rs:620`). The status
  routes already share one auth posture: when `require_auth` is set they get the auth middleware
  (`routes.rs:124-129`), otherwise they are open (`:140`) — `/config` must join that exact group, not the
  command group.
- **Fix:** Capture the running `Config` (or an `Arc<Config>`) at daemon start and thread it through
  `start_server` into `AppState`. Add `handle_config` returning the config as JSON with the two secret fields
  redacted — prefer a deliberate redacting view (a small serializable projection, or `#[serde(serialize_with)]`
  masking that emits e.g. `null`/`"***"`) over deleting the fields, so the *shape* the panels parse stays
  stable and a future secret is caught by the same mechanism. Register `/config` in the status-route group so
  it inherits the open-by-default / `require_auth`-gated behavior. **Do not** put it in the command group
  (those always carry the command auth middleware). A Rust test (`tests/http_api_test.rs`) populates a config
  with a non-empty `auth_token` and `mqtt.password`, hits `/config`, and asserts both secret *values* are
  absent from the serialized body (and that the rest of the config is present).

### F2 — Config read + display (frontend); feed alias labels to the matrix

- **Statement:** A config feature reads `GET /config` through the typed client and displays the running values;
  its `audio.channel_aliases` feed the Sprint W4 matrix's destination-column labels, closing W4's manual-label
  dependency.
- **Where:** `webui/src/features/config/` (a `useConfig` TanStack Query hook over the Sprint W0 client; a
  `ConfigOverview` panel; a shared `channelAliases` selector that W4's matrix consumes).
- **Priority:** P1 (the panels and the W4 label fix both depend on it).
- **Rationale:** Sprint W4's matrix has a documented "manual-label dependency" — its destination columns had no
  source of human-readable names because nothing fed it `audio.channel_aliases`. `GET /config` (F1) now
  provides them, so the matrix can label columns by alias instead of by raw index. All daemon access goes
  through `DaemonConnection`; no module imports `fetch` directly (DW2).
- **Approach:** Add a `getConfig()` method to the typed client (Sprint W0) and a `useConfig` query hook (cached;
  `/config` is read-once-ish, so a long stale time is fine — config does not change without a restart, DW8).
  Render a read-only overview of the running config. Expose a memoized `channelAliases` selector (alias → index
  map) that the W4 matrix imports for its column headers; coordinate the shape with W4 rather than duplicating
  its grid. Component tests assert the panel renders values from a `/config` fixture and that the alias selector
  yields the expected label map.

### F3 — Restart-required editors emitting validated snippets

- **Statement:** Editors for ducking rules, bass/LFE crossover, channel aliases, per-channel calibration,
  `output_ceiling_db` + `master_gain`, macros, and input definitions. Each pre-fills from `/config`, validates
  edits against a schema mirrored from `config.rs`, and emits a config-JSON snippet flagged **"restart
  required"** (DW8). They must never imply live application.
- **Where:** `webui/src/features/config/*` (one panel per group: `DuckingRulesEditor`, `BassCrossoverEditor`,
  `ChannelAliasesEditor`, `ChannelCalibrationEditor`, `LimiterGainEditor`, `MacrosEditor`, `InputsEditor`); a
  `configSchema.ts` mirroring the validated ranges from `src/config.rs`.
- **Priority:** P1 (the core deliverable).
- **Rationale:** The daemon validates these fields at load (`Config::validate`, `src/config.rs:970`) — e.g.
  `output_ceiling_db` must be finite ∈ `[-60.0, 0.0]` (`config.rs:1017-1019`) and `master_gain` finite ∈
  `[0.0, 8.0]` (`config.rs:1025-1026`). An editor that emits a snippet the daemon will *reject* at startup is a
  trap; mirroring the validation client-side gives the operator the error before the restart, not after. Per
  DW8 the daemon reads config once at startup, so these are display-plus-emit-snippet-plus-restart, never live.
- **Approach:** Mirror the validated ranges and required shapes from `config.rs` into `configSchema.ts` (at
  minimum: ceiling `[-60, 0]`, master gain `[0, 8]`, LFE crossover frequency, alias map shape,
  `channel_volumes` map, ducking-rule shape, input-definition shape). Each editor pre-fills from the `useConfig`
  data, validates on change against the schema (surfacing the same message text the loader would), and renders a
  copyable JSON snippet that is a *valid fragment* of the config file (so the operator can paste it). Every
  editor carries a persistent, unmissable **"restart required — the daemon reads config once at startup"**
  banner (DW8); none shows a "saved/applied" affordance. Vitest/RTL tests assert: an out-of-range ceiling is
  flagged before emit; a valid edit emits a snippet whose shape round-trips the schema; the restart-required
  banner is present on every editor.

### F4 — Live ducking visualization

- **Statement:** Show the configured ducking rules and the resolved multipliers *firing live* on the voice
  strips, driven by the `/metrics` `ducking` map and `/status/voices` `ducking_multiplier`; surface the
  documented mic-/voice-activity-trigger (not signal-gated) limitation.
- **Where:** `webui/src/features/config/DuckingPanel.tsx`; data from the existing `/metrics` `ducking` map
  (`handlers.rs:122-128`) and `/status/voices` `ducking_multiplier`.
- **Priority:** P2 (visualization; the read + editors are the gating deliverables).
- **Rationale:** Sprint W2 shows a per-voice `ducking_multiplier` but does not relate it to the *rules* that
  caused it. The control-side ducking snapshot the handlers read is updated by `notify_voice_activity`
  (`src/main.rs:920-957`): a voice becomes a ducking *trigger* when it goes active (a sample starts, or a
  configured input voice is marked active — `main.rs:522-529`), **not** when its audio signal crosses a
  threshold. So a configured mic input ducks the moment its voice is active, regardless of whether anyone is
  actually speaking; the UI must say so plainly rather than implying VAD/threshold gating. This uses only the
  already-live poll surface (DW6) — no new telemetry (W6/W7).
- **Approach:** Cross-reference the configured rules (from `/config`, F2) with the live `ducking` map (from
  `/metrics`) and the per-voice `ducking_multiplier` (from `/status/voices`): mark each rule "firing" when its
  trigger voice is present in the live `ducking` map, and show the resolved multiplier on the affected voice
  strip. Include a clear annotation that ducking is **voice-activity-triggered, not signal-gated** (a mic input
  ducks while its voice is active, not while it has signal). Component tests: a fixture with an active duck
  shows the rule "firing" and the multiplier on the target strip; an empty `ducking` map shows nothing firing;
  the limitation annotation is present.

### F5 — Live per-input volume/mute, wired distinctly

- **Statement:** The only config-derived state changeable at runtime — a running input's `volume` and `mute`
  — applies immediately, and is rendered as visibly distinct from the restart-required editors.
- **Where:** `webui/src/features/config/InputsPanel.tsx`; emits `input_volume`/`input_mute` via the Sprint
  W3/W5 client methods. Daemon path: `src/main.rs:2020-2040` (`InputVolume` → `SetInputVolume`, `InputMute` →
  `SetInputMute`).
- **Priority:** P1 (the one live knob; must be unambiguous next to the restart-required editors).
- **Rationale:** Per DW8 and the API contract §8, the *only* config-derived state the daemon mutates at runtime
  is per-input volume/mute — the audio thread owns the live input and applies the change immediately
  (`main.rs:2024-2040`; mute saves-then-zeroes and unmute restores, D34). Everything else in the config panel
  needs a restart. If the two are visually intermixed, the operator will assume the restart-required editors
  also apply live — the exact failure DW8 forbids.
- **Approach:** In `InputsPanel`, render the input *definitions* (device, channels, route) as restart-required
  display/edit (part of F3's input-definitions editor), but render per-input `volume` (slider) and `mute`
  (toggle) as **live** controls that emit `input_volume`/`input_mute` through the existing client methods and
  reflect optimistically, then reconcile against the next `/status/inputs` poll. Mark the live controls with a
  distinct affordance (e.g. a "live" badge / section divider) and keep them out of the "restart required"
  region. Do not re-implement the command JSON — reuse Sprint W3/W5's client. Tests: toggling mute emits the
  correct `input_mute` JSON and shows live; the live section is visually/semantically separated from the
  restart-required editors (assert the absence of a restart banner on the live controls and its presence on the
  definition editor).

## Caveats (do not chase ghosts / do not break)

- **Never imply a config edit applies live except per-input volume/mute (DW8).** Every editor in F3 — ducking
  rules, crossover, aliases, calibration, ceiling/gain, macros, input *definitions* — is display + emit-snippet
  + restart. The daemon reads config once at startup (`main.rs:410-421`, `:584`); there is no
  config-mutation command and no hot-reload. The only live knobs are `input_volume`/`input_mute`
  (`main.rs:2020-2040`). Do not add a "Save"/"Apply" button to any restart-required editor; do not poll a
  fictional "config applied" state.
- **Redact secrets in `GET /config` (DW11).** `mqtt.password` (`config.rs:23`) and `http.auth_token`
  (`config.rs:620`) must be omitted or masked — never serialized verbatim. Prefer a redacting *view* so a
  future secret field is caught by the same mechanism rather than leaking. The Rust test asserts the values are
  absent; do not weaken it to a structural check.
- **The emitted snippet must round-trip the daemon's loader.** A snippet that fails `Config::from_file` /
  `Config::validate` (`config.rs:683`, `:970`) at startup is a trap. Mirror the *actual* validated ranges
  (ceiling `[-60, 0]`, master gain `[0, 8]`) — do not invent looser or stricter bounds. The Lane B box proves
  the round-trip against the real loader.
- **Do not fake unavailable data.** `/status/samples` `position`/`position_ms`/`progress_percent` are still
  hard-coded `0` until Sprint W6 — this sprint must not surface a fabricated position anywhere in the config
  panels. Ducking visualization uses only the genuinely-live `ducking` map + `ducking_multiplier`.
- **Do not touch the RT audio path.** `GET /config` reads the control-side `Config`; it must never lock the RT
  mutex (D22a, `rt_engine.rs:302-323`) nor allocate/free on the audio callback. This sprint adds no atomics and
  no telemetry — it is a control-thread read plus frontend. The `[RA]` gate exists to *prove* the daemon change
  (F1) stays clean (`-D warnings`/clippy/`fmt`/tests + the alloc harness), not because new RT code is added.
- **Label ducking honestly.** It is voice-activity-triggered, not signal-gated (`notify_voice_activity`,
  `main.rs:920-957`). Do not present it as VAD/threshold ducking; surface the limitation in the panel.
- **Do not duplicate other sprints' surfaces.** The matrix grid is W4 (feed it aliases, don't rebuild it); the
  status racks are W2; the input_volume/input_mute command JSON is W3/W5 (reuse, don't re-encode); live
  telemetry is W6/W7 (don't add atomics here).

## Tasks (ordered, TDD-first)

Each behavior task writes the failing test first, confirms it fails, writes the minimum code to pass, and
confirms green. **Subagents (optional):** the backend `GET /config` (Task 1) and the frontend panels (Tasks
2–6) parallelize cleanly — the only shared contract is the `/config` JSON shape, so freeze that shape first and
let the two tracks run; reconcile at Task 7. Backend uses the existing Rust suite + the alloc-counting harness;
frontend uses Vitest + RTL (+ Playwright for E2E against the mock backend).

1. **`GET /config`, secrets redacted (F1) — backend, do first; it fixes the `/config` contract.** In
   `tests/http_api_test.rs`, add a failing test that builds an `AppState` whose config has a non-empty
   `http.auth_token` and `mqtt.password`, `GET`s `/config`, and asserts (a) `200`, (b) neither secret *value*
   appears anywhere in the serialized body, and (c) a non-secret field (e.g. `audio.output_ceiling_db`) is
   present. Confirm it fails (no route). Thread the running `Config` through `start_server`
   (`src/http/mod.rs:112`) into `AppState` (`:60`); add `handle_config` in `handlers.rs` with a redacting view;
   register `/config` in the status-route group (`routes.rs:101-108`). Confirm green. Add a second test that
   under `require_auth` an unauthenticated `GET /config` is rejected like the other status routes.

2. **Config read + display + alias selector (F2) — frontend.** Add a failing Vitest/RTL test: given a `/config`
   fixture, `ConfigOverview` renders the running values and the `channelAliases` selector returns the expected
   alias→index map. Confirm it fails. Add `getConfig()` to the typed client, the `useConfig` hook, the overview
   panel, and the alias selector. Confirm green. (Coordinate the alias-selector shape with W4 so its matrix can
   consume it.)

3. **Restart-required editors + schema mirror (F3) — frontend.** Add failing tests: an out-of-range
   `output_ceiling_db` (outside `[-60, 0]`) is flagged before emit; a valid ducking-rule / crossover / alias /
   calibration / ceiling+gain / macro / input-definition edit emits a JSON snippet matching the schema; every
   editor shows the "restart required" banner. Confirm they fail. Build `configSchema.ts` (mirroring the
   `config.rs:970` ranges) and the editors. Confirm green.

4. **Live ducking visualization (F4) — frontend.** Add a failing test: given `/metrics` with a non-empty
   `ducking` map and `/status/voices` with a sub-1.0 `ducking_multiplier`, `DuckingPanel` marks the matching
   rule "firing" and shows the multiplier on the target strip, and renders the voice-activity-trigger
   limitation note; an empty `ducking` map shows nothing firing. Confirm it fails. Implement the cross-reference
   against the live poll surface. Confirm green.

5. **Live per-input volume/mute, wired distinctly (F5) — frontend.** Add a failing test: in `InputsPanel`,
   moving the volume slider / toggling mute emits the correct `input_volume`/`input_mute` JSON via the W3/W5
   client and reflects optimistically; the live controls are in a section *without* a restart-required banner
   while the input-definitions editor *has* one. Confirm it fails. Wire the live controls (reusing the existing
   client methods) and the visual separation. Confirm green.

6. **Playwright headless E2E (Lane A).** Against the mock backend, drive: open the config panel → values render
   from `/config` → edit a ceiling out of range → see the validation error → enter a valid value → snippet
   appears with the restart banner → toggle a live input mute → see it reflect live. Headless, green.

7. **Reconcile + docs.** Confirm the frontend's parsed `/config` shape matches F1's redacting view (subagent
   reconciliation point). Document the new endpoint in `docs/webui/API-CONTRACT.md` §5/§7 and `docs/http-api.md`
   (its "REST Endpoints" + status sections), and changelog the `GET /config` addition per DW11. Run the full
   Lane A + `[RA]` gates green.

## Files to create / touch

**Create:**
- `webui/src/features/config/ConfigOverview.tsx` — read-only display of the running config from `/config`.
- `webui/src/features/config/DuckingRulesEditor.tsx`, `BassCrossoverEditor.tsx`, `ChannelAliasesEditor.tsx`,
  `ChannelCalibrationEditor.tsx`, `LimiterGainEditor.tsx`, `MacrosEditor.tsx`, `InputsEditor.tsx` — the
  restart-required snippet editors (F3).
- `webui/src/features/config/DuckingPanel.tsx` — live ducking visualization (F4).
- `webui/src/features/config/InputsPanel.tsx` — live per-input volume/mute controls (F5).
- `webui/src/features/config/configSchema.ts` — the schema mirror of `config.rs` validated ranges/shapes.
- `webui/src/features/config/useConfig.ts` — the TanStack Query hook + `channelAliases` selector.
- `webui/src/features/config/__tests__/` — Vitest/RTL tests for the above.
- `webui/tests/e2e/config.spec.ts` — the Playwright headless flow.

**Touch:**
- `src/http/handlers.rs` — new `handle_config` (alongside `handle_metrics`, `:82`).
- `src/http/routes.rs` — register `/config` in the status-route group (`:101-108`).
- `src/http/mod.rs` — `AppState` gains a handle to the running config (`:60`); `start_server` threads it in
  (`:112`).
- `src/config.rs` — a redacting serialize view for `GET /config` (mask `mqtt.password` `:23`, `http.auth_token`
  `:620`) if a dedicated view is the cleanest path.
- `src/main.rs` — pass the running `Config` (or `Arc<Config>`) into `start_server` at `:693`.
- `tests/http_api_test.rs` — the `/config` redaction + `require_auth` tests.
- `webui/src/api/client.ts` (+ types) — add `getConfig()`.
- `webui/src/features/matrix/*` — consume the `channelAliases` selector for destination labels (coordinate with
  W4; minimal touch).
- `docs/webui/API-CONTRACT.md` (§5/§7), `docs/http-api.md`, `CHANGELOG.md` — document the new endpoint (DW11).

## Verification

### Lane A (CI / headless)
- `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) all pass; Vitest + RTL green.
- **F2:** `ConfigOverview` renders the running values from a `/config` fixture; the `channelAliases` selector
  yields the expected alias→index label map (the W4 matrix can consume it).
- **F3:** an out-of-range `output_ceiling_db`/`master_gain` is flagged client-side *before* emit against the
  mirrored `[-60,0]`/`[0,8]` ranges; a valid edit in each group (ducking, crossover, aliases, calibration,
  ceiling+gain, macros, inputs) emits a JSON snippet matching the schema; **every** editor shows the
  "restart required" banner and **no** editor shows a "saved/applied" affordance.
- **F4:** a non-empty `/metrics` `ducking` map + sub-1.0 `/status/voices` `ducking_multiplier` marks the
  matching rule "firing" and shows the multiplier on the target strip; an empty map shows nothing firing; the
  voice-activity-trigger (not signal-gated) limitation note is present.
- **F5:** the live volume/mute controls emit the correct `input_volume`/`input_mute` JSON and reflect
  optimistically; they live in a section *without* a restart-required banner, while the input-definitions
  editor *has* one.
- **Playwright headless** drives the full config flow against the mock backend, green.

### Lane B (real browser + live daemon)
- Against a real daemon behind the sidecar, `GET /config` returns the running config and **round-trips**: the
  values shown match the on-disk config, with `auth_token`/`mqtt_password` absent from the response.
- A snippet generated by an F3 editor, pasted into the daemon's config file, **validates against the daemon's
  loader** (`Config::from_file`/`validate`, `config.rs:683`/`:970`) and the daemon starts with the change in
  effect after a restart.
- A live per-input volume/mute change applies immediately (audible / reflected in `/status/inputs`), confirming
  the live-vs-restart split is real.
- Ducking firing renders live as a configured trigger voice goes active.

### Rust Lane A [RA]
- `cargo build --release` with `-D warnings` is clean; `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` pass.
- The new `tests/http_api_test.rs` cases pass: `GET /config` returns `200` with `auth_token`/`mqtt_password`
  values **absent** from the body and non-secret fields present; under `require_auth`, an unauthenticated
  `GET /config` is rejected exactly like the other status routes.
- The full existing Rust suite stays green (no regression to status/command routes).
- The alloc-counting harness shows the audio callback path is unchanged — `GET /config` is a control-thread
  read and adds **0** new allocations/frees/locks on the RT path.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 8)

- [ ] A read-only `GET /config` returns the running config with secrets redacted (`auth_token`, `mqtt_password`); a Rust test asserts redaction (DW11) `[RA]`
- [ ] Config panels display current values from `/config` and produce validated config-JSON snippets flagged "restart required" for ducking rules, bass/LFE crossover, channel aliases, per-channel calibration, limiter ceiling + master gain, macros, and input definitions (DW8) `[A]`
- [ ] Ducking is visualized live (rules firing) from `/metrics` ducking map + `/status/voices` `ducking_multiplier`, with the mic-trigger limitation surfaced `[A]`
- [ ] The genuinely-live settings (per-input volume/mute) are wired to take effect immediately, distinct from the restart-required editors `[A]`
- [ ] Against a real daemon, `/config` round-trips and a generated snippet validates against the daemon's loader `[B]`

## Behavior-change / changelog notes

- **New read-only `GET /config` endpoint (secrets redacted)** — the daemon gains a status-gated route returning
  the running config with `auth_token` and `mqtt_password` omitted/masked (DW11). This is an observable daemon
  change: record it in `CHANGELOG.md`, and document it in `docs/webui/API-CONTRACT.md` §5 (REST endpoints) /
  §7 (it moves from "planned" to "present") and in `docs/http-api.md`'s endpoint list. No other daemon behavior
  changes — there is no config-mutation command, no hot-reload, and no RT-path change.
- Everything else in this sprint is **pure frontend** (the panels, editors, ducking visualization, and the
  live-vs-restart split) and changes no daemon behavior; the per-input volume/mute "live" wiring reuses the
  existing `input_volume`/`input_mute` commands and adds nothing new to the daemon.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless
against the mock backend) · Lane B green (real daemon: `/config` round-trips with secrets absent, a generated
snippet validates against the real loader, a live input volume/mute change applies immediately) · Rust Lane A
green (`cargo build --release` `-D warnings` warning-free · clippy · fmt · full suite incl. the new `/config`
redaction + `require_auth` tests · alloc harness shows 0 new alloc/free/lock on the RT path) · every editor is
display-plus-emit-snippet-plus-restart with no "applied live" affordance except per-input volume/mute (DW8) ·
secrets redacted in `GET /config` (DW11) with a test that asserts it · ducking labeled voice-activity-triggered
not signal-gated · new tests added with no coverage reduction · `CHANGELOG.md` + `docs/webui/API-CONTRACT.md` +
`docs/http-api.md` updated for the new endpoint (DW11) · committed atomically on the branch.
