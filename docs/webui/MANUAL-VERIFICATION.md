# Manual Verification — cross-browser, accessibility & human visual QA

Automated validation covers **Lane A** (CI/headless: build, typecheck, lint, unit/component, Playwright
headless) and **Lane B** (the dev's own real browser against a live daemon behind the sidecar). What it **cannot**
judge — rendering parity across browser engines, keyboard/screen-reader experience, and whether live telemetry
*looks* smooth to a human — lives here as **Lane C**. Sprints **append** concrete, copy-pasteable steps and
expected results as they implement features. The whole document is run once, after the final sprint, and results
are recorded inline.

> Sprints must not invent QA steps that Lane A or B already proves. Only add steps that genuinely require a
> different browser engine, assistive technology, or human judgment.

## How to run

1. Build and serve the production SPA through the reverse-proxy sidecar (see `docs/webui/README.md`), pointed at
   a running `mqttaudio` daemon with the HTTP server enabled.
2. For each check, exercise it in **Chrome, Safari, Firefox, and Edge** (and one mobile browser where noted), and
   record **PASS/FAIL + browser + notes + date** in the Result line.

---

## V-1 — Cross-browser rendering & interaction parity  _(seed; expanded by Sprints 2–5, 9)_

**Why manual:** layout, MUI theming, WebSocket behavior, and slider/scrubber interactions can differ across
engines in ways a single headless Chromium run cannot reveal.

**Steps:**
1. Load the app and connect to a daemon. Confirm the dashboard, command test-bench, matrix mixer, and transport
   controls render without layout breakage in each browser.
2. Drive a play, a stop, a voice-volume change, and a channel-mapped play from the UI; confirm the dashboard
   updates and the audio responds.
3. Resize from desktop to tablet width; confirm the responsive layout holds.

**Expected:** visual + interaction parity across all four engines; no console errors; no broken controls.

**Result:** **PASS across all three engine families (automated) + Chrome live-daemon PASS; real-app/mobile
visual spot-check PENDING (human).** Two complementary passes, 2026-06-05:

- **Cross-engine automated parity** — `pnpm test:e2e:crossbrowser` (config `playwright.crossbrowser.config.ts`)
  runs the fixture-backed e2e specs across **Chromium (Blink — also Edge), WebKit (Safari's engine), and
  Firefox (Gecko)**: connect + daemon-identity render, the `/ws` log-console (welcome + log frames, i.e.
  WebSocket behavior), and **axe a11y on both the connect screen and the connected dashboard** — **12/12 green
  on all three engines**. This covers the rendering / interaction / WebSocket / a11y parity that a single
  Chromium run can't, using real WebKit and Gecko engines (not emulation).
- **Chrome against the live daemon** — drove the SPA in real Chrome behind the Vite proxy: the full dashboard
  and all five tabs (Monitor / Mixer / Console / Matrix / Config) render without breakage; a real looping play
  + the Telemetry toggle drove live now-playing / meters / voice / cache updates; **dark and light themes**
  both render with good contrast; the layout **collapses cleanly to a single column at tablet width (834 px)**;
  **no console errors** (only benign Vite HMR + the React DevTools notice).

Remaining for a human (small): an eyes-on spot-check in the actual **Safari / Edge desktop apps** and a real
**mobile** browser — the engine families are now covered automatically, so this is a visual-polish confirmation,
not unverified surface.

---

## V-2 — Accessibility: keyboard & screen-reader walk-through  _(seed; expanded by Sprint 9)_

**Why manual:** automated axe checks catch structural issues but not the lived keyboard/screen-reader
experience.

**Steps:**
1. With the mouse unplugged/untouched, tab through the dashboard, command forms, matrix mixer, and transport
   controls; confirm focus order is sensible and every control is reachable and operable by keyboard.
2. With a screen reader (VoiceOver on macOS / Narrator on Windows), confirm controls announce meaningful labels
   and live regions (e.g. a clip/stream-error badge, a "ducked" indicator) are announced when they change.

**Expected:** full keyboard operability; meaningful announcements; no focus traps.

**Result:** **PARTIAL — keyboard + axe PASS (cross-engine); screen-reader PENDING (human).** 2026-06-05:
the **axe** check (`e2e/a11y.spec.ts`) reports no serious/critical violations on the connect screen **and** the
connected dashboard, now run across **Chromium, WebKit, and Firefox** via `pnpm test:e2e:crossbrowser` (not just
Chromium CI). In real Chrome, Tab traversal reaches the header controls in a sensible order and the **Telemetry
switch surfaces its descriptive tooltip on keyboard focus** (not hover-only) — the control is keyboard-focusable
and self-describing. Remaining for a human: an actual screen-reader (VoiceOver / NVDA) walk-through to confirm
spoken labels and that live regions (clip / stream-error badge, "ducked" indicator) announce on change — a
sensory judgment no automation can stand in for.

---

## V-3 — Live telemetry looks smooth (human judgment)  _(seed; populated by Sprints 6–7)_

**Why manual:** "do the meters and progress bars move smoothly, without jank or lag, and does the audio sound
unaffected with telemetry on?" is a judgment a human must make. Lane B proves it *works*; this confirms it
*feels* right and that opting in does not audibly degrade playback.

**Steps:**
1. Enable telemetry in the UI against a live daemon. Start several overlapping plays (including a loop and a
   variable-speed sample).
2. Watch the progress bars / playhead and the output + per-input meters for ~2 minutes.
3. Toggle telemetry off and back on.

**Expected:** progress and meters update smoothly at ~15–20 Hz with no visible stutter; audio is unaffected
whether telemetry is on or off; toggling off cleanly stops the live updates and the UI falls back to polling.

**Result:** **PARTIAL — Chrome visual PASS; "audio unaffected" listening judgment PENDING (human).** Chrome
2026-06-05 against a live daemon with a real looping play: enabling Telemetry made the **output meters (ch 0 /
ch 1) move with the tone** over `/ws/state`, and the now-playing progress bar + the Mixer seek playhead
**advanced live** through the loop; updates looked smooth with no visible stutter. Toggling Telemetry off
stopped the live updates (the off-state "Enable Telemetry…" fallback returns). Remaining for a human: a
sustained ~2-minute watch with several overlapping/variable-speed plays to judge sub-perceptual jank, and an
**ears-on confirmation that opting in does not audibly degrade playback** (the alloc harness already proves the
RT path stays 0-alloc/0-free, but "sounds identical" is a human call).

---

_Additional checks are appended by sprints as they land. Keep IDs stable (V-4, V-5, …)._
