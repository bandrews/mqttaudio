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

**Result:** _(record PASS/FAIL + browser + notes + date)_

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

**Result:** _(record PASS/FAIL + AT used + notes + date)_

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

**Result:** _(record PASS/FAIL + notes + date)_

---

_Additional checks are appended by sprints as they land. Keep IDs stable (V-4, V-5, …)._
