# Sprint 3 — Command Test-Bench & Cue Launcher

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A, B |
| Subagents | Optional (per-command forms / raw editor / cue launcher parallelize) |

## Goal

Build a forms-driven command console that lets an operator exercise **every** runtime command the daemon
accepts — `play`, `stop`, `stopall`, `volume`, `seek`, `speed`, the `voice_*` family, the `input_*` family,
the `cache_*` family, and `precache` — without hand-crafting JSON or speaking MQTT. Each form emits the exact
JSON shape the daemon expects (through the typed API client from Sprint W0, or the raw `/command` path per
DW10), with client-side validation that **mirrors the daemon's own ranges and enum casing** but never silently
rewrites the operator's input. On top of the per-command forms, this sprint adds two higher-level surfaces: a
**raw `/command` JSON editor** that exposes the full play surface the typed `/play` endpoint drops
(`channel_map`, `mode`, `window_ms`, `prebuffer_ms`, `freshness`, `cacheable`, `crossfade_ms`), and a **cue
launcher** that composes a file plus options into a play with a live JSON preview reflecting macro-merge and
explicit-field precedence.

"Done and correct" means: a representative command from each family, issued from the console, lands on a real
daemon and shows up on the Sprint W2 dashboard; the selector model's pure-OR semantics and its **silent
empty-selector no-op** are faithfully encoded and loudly warned about; and the UI never claims a command was
"valid" merely because `/command` returned `200`. This sprint builds on the typed API client and
`DaemonConnection` transport from Sprint W0 and the dashboard from Sprint W2 (for confirming effects); it must
not introduce a second, divergent way to reach the daemon, and it must not bypass the `DaemonConnection` seam
(DW2). It sends only commands that already exist — **no daemon change**.

## Why

Today the only way to drive the daemon's full command surface is to hand-author MQTT/HTTP JSON and read the
code to know which keys are accepted, which are case-sensitive, and which are silently dropped. That is exactly
the "experiment without hand-crafting MQTT" gap this surface closes, and it is the prerequisite for the matrix
mixer (Sprint W4) and the transport panels (Sprint W5), both of which build on the `/command` path and the
selector model established here.

Two pieces of daemon behavior have **no coverage anywhere in the program** until this sprint encodes them, and
both are silent footguns. First, the selector model is **pure OR with check order `internal_id → id → file →
voice`** and an **empty selector matches nothing and is a silent no-op** (`commands.rs:114-119`) — a `stop`
with no criteria set returns `200` and does nothing, with no error. Second, the typed `/play` endpoint
**silently drops** `channel_map`, `mode`, `window_ms`, `prebuffer_ms`, `freshness`, and `cacheable`
(`handlers.rs:181-198`), so the routing/streaming surface is unreachable through it; only the raw `/command`
path carries those fields (DW10). A console that did not encode the OR-logic, the empty-selector warning, and
the `/command`-vs-typed split would quietly mislead the operator. This sprint makes those behaviors visible and
safe.

## Scope

**In scope**

- **A form per runtime command** (`play`, `stop`, `stopall`, `volume`, `seek`, `speed`, `voice_volume`,
  `voice_fade_out`, `voice_stop`, `input_volume`, `input_mute`, `cache_clear`, `cache_invalidate`,
  `cache_reload`, `precache`) that emits correct JSON, with client-side validation mirroring the code: `volume`
  in `0..1`, `speed` in `-100..100` (no pitch) or `0.05..8.0` (pitch, no reverse), lowercase `mode`/`freshness`
  enums, and `internal_id` restricted to digits.
- **A selector widget** (`internal_id` / `id` / `file` / `voice`, pure-OR) shared by `stop` / `volume` /
  `seek` / `speed`, with an explicit, loud warning when no criterion is set (silent no-op).
- **A raw `/command` JSON editor** exposing the full play surface — `channel_map`, `mode`, `window_ms`,
  `prebuffer_ms`, `freshness`, `cacheable`, `crossfade_ms` — that typed `/play` omits (DW10), with field hints.
- **A cue launcher** composing `file` + options into a play, with a live JSON preview reflecting macro-merge
  and explicit-field precedence (top-level command params win over macro params; earlier macros win over later).
- **Honest response handling:** the console surfaces "enqueued", never "valid", and points the operator at the
  dashboard to confirm an effect.

**Out of scope**

- **The `channel_map` matrix grid UI** — owned by **Sprint W4** (channel-map matrix mixer), coordinate, do not
  duplicate. This sprint exposes `channel_map` **only as raw JSON** inside the raw `/command` editor; the
  src×dest grid, per-route gain cells, and clip-risk badges are W4's.
- **Transport on already-playing samples** (seek scrubber over `total_ms`, speed control with a 1.0 detent and
  pitch toggle, voice/input strips on live cards) — owned by **Sprint W5** (transport, speed & windowed
  gating), coordinate, do not duplicate. This sprint provides only the bare `seek` / `speed` / `voice_*` /
  `input_*` **forms**, not the live per-sample transport UI.
- **Config editing** (ducking rules, crossover, aliases, calibration, macros as config, input definitions) —
  owned by **Sprint W8** (config visibility & tuning), coordinate, do not duplicate. The cue launcher's macro
  preview is a *client-side composition* aid, not a config editor.

## Work items

### F1 — Command form framework + validation mirroring the daemon

- **Statement:** Build a reusable command-form framework and one form per runtime command, each emitting the
  exact JSON the daemon accepts and validating inputs against the daemon's documented ranges and enum casing —
  validating and **warning**, never silently clamping or rewriting.
- **Where:** `webui/src/features/console/CommandForm.tsx` (the per-command form shell + field renderers);
  `webui/src/features/console/validation.ts` (the validators). Emits via the Sprint W0 typed client / raw
  `/command`.
- **Priority:** High — this is the spine of the console; F2–F5 hang off it.
- **Evidence:** The parser accepts out-of-range `volume`/`speed` and clamps **downstream** in the mixer, not at
  parse time (API-CONTRACT §2). `volume` is clamped `0..1` downstream; `speed` is clamped at `mixer.rs:369-379`
  — with pitch correction the range is `0.05..8.0` and negative speed is rejected (no reverse,
  `mixer.rs:362-368`), and without pitch correction the range is `-100..100` where negative means reverse.
  `mode` and `freshness` are lowercase enums; `internal_id` is sent as a **string** but matched by parsing to
  `u64`, so a non-numeric value silently never matches (`commands.rs:80-86`). Command **names** are
  case-sensitive exact matches (`commands.rs:450`), and the legacy aliases exist for exactly three commands
  (`play`⇄`soundPlay`, `stopall`⇄`soundStopAll`, `precache`⇄`soundPrecache`).
- **Approach:** Define a declarative field spec per command (name, type, required/optional, default, validator)
  and render it through `CommandForm`. In `validation.ts`, mirror the daemon: warn (do not block submit, do not
  rewrite) when `volume` is outside `0..1`; gate `speed` on the pitch-correction toggle, warning outside
  `-100..100` (no pitch) or `0.05..8.0` (pitch) and warning that reverse is unavailable with pitch; constrain
  `mode`/`freshness` to lowercase enum values via select inputs so bad casing is unrepresentable; restrict
  `internal_id` to a digits-only input and warn that a non-numeric value can never match. Emit `play` /
  `stopall` / `precache` under their canonical names (the API client owns alias handling, Sprint W0). Forms
  send through the typed endpoints where those carry every field, and through `/command` where they do not (F3,
  DW10).

### F2 — Selector widget with empty-selector warning

- **Statement:** A single selector widget, shared by `stop` / `volume` / `seek` / `speed`, exposing the four
  OR-logic criteria and warning loudly and explicitly when none is set, because an empty selector is a silent
  no-op on the daemon.
- **Where:** `webui/src/features/console/SelectorField.tsx`, consumed by the `stop` / `volume` / `seek` /
  `speed` forms (F1).
- **Priority:** High — without the warning, the most common operator mistake (firing a selector command with no
  criteria) silently does nothing and looks like a daemon bug.
- **Evidence:** The four criteria — `internal_id`, `id`, `file`, `voice` — are flattened directly into the
  message (not a nested `selector` object), and matching is **pure OR with check order `internal_id → id →
  file → voice`** (`commands.rs:72-111`): a sample matches if **any** specified criterion matches. An **empty
  selector (all four absent) matches nothing and is a silent no-op** (`commands.rs:114-119`); the daemon returns
  `200` and does nothing. `file` is exact string equality (no path normalization) and `voice` is an exact
  `voice_id`, so each targets *all* samples of that file/voice (API-CONTRACT §3).
- **Approach:** Render four optional fields and flatten any that are set into the command body (never nest them
  under a `selector` key). Surface the check order in helper text so the operator understands that
  `internal_id` is consulted first. When all four are empty, render a prominent inline warning ("No selector set
  — this command will match nothing and silently do nothing") and visibly flag the submit; do **not** disable
  submit (the daemon does accept it — the UI's job is to warn, not to forbid). Restrict `internal_id` to digits
  (shared with F1) and note that a non-numeric `internal_id` never matches.

### F3 — Raw `/command` JSON editor (full play surface)

- **Statement:** A raw `/command` JSON editor that exposes the full play surface — `channel_map`, `mode`,
  `window_ms`, `prebuffer_ms`, `freshness`, `cacheable`, `crossfade_ms` — that the typed `/play` endpoint
  silently drops, with field hints so the operator knows what each key does.
- **Where:** `webui/src/features/console/RawCommandEditor.tsx`, posting through the `DaemonConnection` raw
  `/command` path (Sprint W0).
- **Priority:** High — these fields are otherwise **unreachable** from the UI; the routing and streaming surface
  depends on them (DW10).
- **Evidence:** `POST /play` accepts **only** `file`, `id`, `volume`, `voice`, `fade_in`, `start_position_ms`,
  `loop`/`loop_mode`, `crossfade_ms`, and **silently ignores** `channel_map`, `mode`, `window_ms`,
  `prebuffer_ms`, `freshness`, `cacheable` (`handlers.rs:181-198`). The raw `/command` endpoint forwards
  arbitrary JSON verbatim (API-CONTRACT §1). A command is `{"command":"<name>", …params}` with params flattened
  at the top level **or** nested under a `message` object, and **nested wins wholesale** — if `message` is
  present, top-level keys are ignored entirely, not merged (`commands.rs:25-33`).
- **Approach:** Provide a JSON editor pre-seeded with a `play` skeleton including the full play surface, with
  inline hints for each field (e.g. `mode`: lowercase enum; `channel_map`: array of `{src,dest,gain?}`;
  `freshness`: lowercase enum, default `trusting`; `cacheable`: bool, default `true`). Validate that the body is
  syntactically valid JSON and carries a `command` key before enabling submit; **warn** (do not block) if both a
  nested `message` and flattened top-level params are present, because the flattened keys would be ignored
  wholesale (`commands.rs:25-33`). Post to `/command` verbatim. Do **not** build the `channel_map` matrix grid
  here — that is Sprint W4; here `channel_map` is raw JSON only.

### F4 — Cue launcher + macro-merge preview

- **Statement:** A cue launcher that composes a `file` plus options into a play, with a live JSON preview that
  reflects macro-merge and explicit-field precedence, so the operator sees the exact body that will be sent.
- **Where:** `webui/src/features/console/CueLauncher.tsx` (the composition UI), `webui/src/features/console/macroMerge.ts`
  (the precedence logic). Sends via the raw `/command` path (it composes the full play surface).
- **Priority:** Medium — a convenience surface for the common case (fire a file with a few options); valuable
  but built on F1/F3.
- **Evidence:** Macro merge is **forward order, earlier macros take precedence**, and the command's own
  top-level params are merged **last (highest priority)** (`commands.rs:418-437`): top-level `>` `macro[0]` `>`
  `macro[1]` `>` … . **Macros use the singular key `macro`, flattened form only**, and are **inert unless the
  daemon wires `expand_macros`** into the command path (API-CONTRACT §1 note) — i.e. composing a `macro`
  reference here may have no effect on the running daemon today.
- **Approach:** Let the operator pick a `file`, set common options (volume, voice, loop, fade, mode, etc.), and
  optionally reference one or more macros by name. Compute the merged body **client-side** in `macroMerge.ts`
  with the exact precedence (top-level explicit fields win; among macros, earlier wins) and render it as a live
  JSON preview that updates as fields change. Label the macro reference clearly: singular `macro` key, flattened
  form only, and **possibly inert** on the daemon (link the operator to the dashboard to confirm an effect).
  Send the previewed body verbatim via `/command`. The macro composition is a client-side aid only — config-side
  macro definitions are Sprint W8.

### F5 — Response-handling honesty

- **Statement:** The console's result UI must report that a command was **enqueued**, never that it was
  **valid**, and must direct the operator to the dashboard to confirm the effect — because `/command` returns
  `200 {success:true}` on enqueue regardless of whether the command is valid or will do anything.
- **Where:** `webui/src/features/console/` result UI (the per-form result/toast component shared across F1–F4).
- **Priority:** High — a UI that conflated `200` with "valid" would actively mislead, and several daemon
  footguns (empty selector, non-numeric `internal_id`, unknown alias, dropped typed-`/play` fields) all return
  `200` and silently do nothing.
- **Evidence:** The raw `POST /command` always returns `200 {success:true}` on **enqueue** — it does **not**
  validate the command or report parse errors to the caller (`handlers.rs:154-175`); the only non-`200`
  responses are `400` on un-serializable JSON and `500` on a send-channel failure. `MissingMessage` is a coarse
  gate that only checks *something* was sent, not that required fields are present (`commands.rs:36-38`). The UI
  cannot use the HTTP status to confirm a command was valid (API-CONTRACT §1).
- **Approach:** On a `200`, render "enqueued — confirm on the dashboard", not "success" or "valid". Surface
  `400`/`500` distinctly as request-level failures (malformed JSON / daemon unreachable). Keep the wording
  evergreen and honest; pair it with a link/affordance to the Sprint W2 dashboard so the operator verifies the
  actual effect (a new sample, a volume change, a stop) rather than trusting the status code.

## Caveats (do not chase ghosts / do not break)

- **HTTP 200 is not "valid".** `/command` returns `200 {success:true}` on **enqueue** regardless of validity
  (`handlers.rs:154-175`). Never render "valid" or "success" off a `200`; render "enqueued" and point at the
  dashboard. This is F5's whole reason to exist — do not weaken it for nicer-looking copy.
- **Validate and warn — never silently clamp.** The parser does **not** clamp; it accepts out-of-range
  `volume`/`speed` and clamps downstream (`mixer.rs:369-379`, API-CONTRACT §2). Mirror the ranges as **warnings
  that still let the operator submit** their exact value. Do not auto-correct, auto-round, or block on
  out-of-range numerics — the operator may be deliberately probing the clamp.
- **Empty selector is a real, silent no-op.** Do not "helpfully" default a selector to "all" or block submit;
  the daemon's empty selector matches nothing (`commands.rs:114-119`). Warn loudly and let it through.
- **Nested `message{}` shadows flat keys.** If a raw body has both a `message` object and flattened top-level
  params, the flattened keys are **ignored wholesale, not merged** (`commands.rs:25-33`). Do not mix the two
  forms; warn if both are present.
- **Macros: singular `macro`, flattened only, possibly inert.** Use the singular key `macro`, flattened form;
  do not assume a macro reference takes effect — `expand_macros` may not be wired into the daemon's command path
  (API-CONTRACT §1 note). Present the merged preview honestly and confirm effects on the dashboard.
- **`internal_id` is a string of digits.** Send it as a string; a non-numeric value silently never matches
  (`commands.rs:80-86`). Restrict the input to digits and warn otherwise.
- **Do not build the channel_map matrix grid or live transport here.** The matrix grid is Sprint W4; the live
  per-sample seek/speed/voice/input transport UI is Sprint W5. This sprint exposes `channel_map` only as raw
  JSON and provides only the bare command forms.
- **Do not fake unavailable data.** `/status/samples` reports `position`/`position_ms`/`progress_percent` as
  hard-coded `0` until Sprint W6; the console must not invent progress or imply a live position it cannot read.

## Tasks (ordered, TDD-first)

Each behavior task writes the failing Vitest + React Testing Library test first, confirms it fails, writes the
minimum component/logic code to pass, then confirms green. **Subagents:** the per-command forms + validation
(F1/F2/F5), the raw editor (F3), and the cue launcher + macro-merge (F4) are independent enough to parallelize
across subagents once the `CommandForm` field-spec shape and the shared result component are agreed; merge on a
single console feature module. This is a **pure-frontend** sprint (no daemon change), so no Rust lanes apply.

1. **Validation unit tests + validators (F1).** Write failing unit tests in `validation.ts`'s test file
   asserting: `volume` outside `0..1` yields a warning (not a block, not a rewrite); `speed` warns outside
   `-100..100` with pitch off and outside `0.05..8.0` with pitch on, and flags reverse as unavailable with
   pitch on; `mode`/`freshness` reject non-lowercase values; `internal_id` rejects non-digits. Confirm they
   fail, implement the pure validators, confirm green.
2. **Per-command form rendering + JSON emission (F1).** Write failing RTL tests that render each command's form
   and assert the emitted JSON shape (correct `command` name, correct keys, optional fields omitted when blank,
   `internal_id` as a string). Confirm failing, implement `CommandForm` + the field specs, confirm green.
3. **Selector widget + empty-selector warning (F2).** Write a failing RTL test asserting that with no criterion
   set the selector renders the explicit no-op warning and flags submit, and that setting any one criterion
   flattens it into the body (not nested) and clears the warning. Confirm failing, implement `SelectorField`,
   confirm green.
4. **Response-handling honesty (F5).** Write a failing RTL test asserting that a `200 {success:true}` renders
   "enqueued" (not "valid"/"success") with a dashboard pointer, and that `400`/`500` render distinct
   request-failure messages. Confirm failing, implement the shared result component, confirm green.
5. **Raw `/command` editor (F3).** Write failing tests asserting: the editor seeds a `play` skeleton with the
   full play surface; submit is blocked on invalid JSON or a missing `command` key; a body with both `message`
   and flattened top-level params raises the shadowing warning; a valid body posts verbatim to `/command`.
   Confirm failing, implement `RawCommandEditor`, confirm green.
6. **Macro-merge precedence (F4).** Write failing unit tests in `macroMerge.ts`'s test file asserting the
   precedence (top-level explicit `>` `macro[0]` `>` `macro[1]`), singular `macro` key, flattened form. Confirm
   failing, implement the pure merge, confirm green.
7. **Cue launcher + live preview (F4).** Write a failing RTL test asserting the preview JSON updates live as
   fields/macros change and equals the `macroMerge` output, and that submit posts the previewed body to
   `/command`. Confirm failing, implement `CueLauncher`, confirm green.
8. **Lane A gate.** Run `pnpm build`, `tsc --noEmit`, eslint warnings-as-errors, the full Vitest + RTL suite,
   and the Playwright headless console smoke against the mock backend. All green, zero warnings.
9. **Lane B verification.** Against a real daemon behind the sidecar, fire one representative command from each
   family and confirm each is reflected on the dashboard (record steps for the box below).

## Files to create / touch

- **Create:**
  - `webui/src/features/console/CommandForm.tsx` — per-command form shell + field renderers.
  - `webui/src/features/console/validation.ts` — daemon-mirroring validators (volume/speed/enum/internal_id).
  - `webui/src/features/console/SelectorField.tsx` — shared OR-logic selector with empty-selector warning.
  - `webui/src/features/console/RawCommandEditor.tsx` — raw `/command` JSON editor (full play surface).
  - `webui/src/features/console/CueLauncher.tsx` — cue composition UI + live JSON preview.
  - `webui/src/features/console/macroMerge.ts` — macro-merge + explicit-field precedence logic.
  - `webui/src/features/console/CommandResult.tsx` — shared "enqueued, not valid" result UI (F5).
  - Co-located test files (`*.test.ts`/`*.test.tsx`) for each of the above; a Playwright spec
    `webui/e2e/console.spec.ts` for the headless smoke.
- **Touch:**
  - `webui/src/features/console/index.ts` (or the console route registration) to mount the console surface.
  - The app's route/navigation wiring to add the console entry alongside the dashboard.
  - The Sprint W0 typed API client only if a command's field spec reveals a missing/mis-encoded key (encode it
    there, with a client unit test — do not duplicate emission logic in the form).

## Verification

### Lane A (CI / headless)

- `pnpm build`, `tsc --noEmit`, and eslint **warnings-as-errors** pass with zero warnings.
- **F1:** unit tests prove every command's form emits the correct `command` name and key set, optional fields
  omitted when blank, `internal_id` serialized as a string; validators warn (never block, never rewrite) on
  out-of-range `volume`/`speed`, gate `speed` on the pitch toggle (`-100..100` vs `0.05..8.0`, reverse blocked
  with pitch), and reject non-lowercase `mode`/`freshness` and non-digit `internal_id`.
- **F2:** RTL tests prove the empty selector renders the explicit no-op warning and flags (but does not disable)
  submit, and that any single criterion flattens into the body (not nested) and clears the warning.
- **F3:** tests prove the raw editor seeds the full play surface, blocks submit on invalid JSON or a missing
  `command` key, raises the `message`-vs-flattened shadowing warning, and posts verbatim to `/command`.
- **F4:** unit tests prove macro-merge precedence (top-level `>` `macro[0]` `>` `macro[1]`, singular `macro`,
  flattened); an RTL test proves the cue launcher's live preview equals the merge output and posts the previewed
  body.
- **F5:** an RTL test proves a `200 {success:true}` renders "enqueued" (never "valid"/"success") with a
  dashboard pointer, and `400`/`500` render distinct request-failure messages.
- Playwright headless console smoke against the mock backend drives one form end-to-end and asserts the posted
  body, green.

### Lane B (real browser + live daemon)

- With the SPA served through the real sidecar against a running `mqttaudio` daemon, fire one representative
  command from **each family** and confirm the effect on the Sprint W2 dashboard: `play` (a new active sample
  appears), `stop`/`stopall` (it disappears), `volume` (the sample/voice volume changes), `voice_volume` /
  `voice_stop` (voice rack reflects it), `input_volume` / `input_mute` (input rack reflects it), `precache` /
  `cache_*` (cache gauge/entries reflect it). A raw `/command` `play` carrying `channel_map`/`mode` is accepted
  (effect confirmed audibly/on-dashboard, full routing UI deferred to W4). Confirm an empty-selector `stop`
  returns `200` and does nothing, matching the UI's warning. Record the steps run.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 3)

- [ ] Every runtime command (play, stop, stopall, volume, seek, speed, voice_*, input_*, cache_*, precache) has a form that emits correct JSON, with client-side validation mirroring the code (volume 0-1, speed ranges, lowercase enums) `[A]`
- [ ] The selector model (`internal_id`/`id`/`file`/`voice`, OR-logic) is exposed with an explicit warning when no selector is set (silent no-op) `[A]`
- [ ] A raw `/command` editor exposes the full play surface (`channel_map`, `mode`, `window_ms`, `prebuffer_ms`, `freshness`, `cacheable`, `crossfade_ms`) that typed `/play` omits (DW10) `[A]`
- [ ] A cue launcher composes file + options into a play, with a live JSON preview reflecting macro + explicit-field precedence `[A]`
- [ ] Against a real daemon, representative commands from each family take effect and are reflected on the dashboard `[B]`

## Behavior-change / changelog notes

None — this is a **pure-frontend** sprint that sends existing commands through the existing HTTP surface; there
is **no daemon change** and nothing to record in `CHANGELOG.md`. The honest response-handling (F5) and the
empty-selector / `message`-shadowing / macro-inertness warnings surface **existing** daemon behavior; they do
not alter it.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless
against the mock backend) with zero build/type/lint warnings · every command family has a JSON-correct,
daemon-mirroring form (F1) · the shared selector warns on the empty no-op (F2) · the raw `/command` editor
exposes the full play surface and warns on `message`-vs-flattened shadowing (F3) · the cue launcher's live
preview matches the macro-merge precedence and posts verbatim (F4) · the result UI says "enqueued", never
"valid" (F5) · Lane B green (one command per family confirmed on the dashboard against a live daemon) · new
Vitest/RTL/Playwright tests added with no coverage reduction · no daemon change, so no `CHANGELOG.md` entry ·
any out-of-scope discoveries logged to `docs/bugs.md` tagged by owning web sprint · committed atomically on a
branch with a clear message.
