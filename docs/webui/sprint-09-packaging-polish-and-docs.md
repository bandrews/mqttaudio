# Sprint 9 — Packaging, Polish, Cross-Browser, A11y & Docs

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0–8 |
| Effort | M |
| Lanes | A, B, C |
| Subagents | Optional (packaging / theming / a11y / docs parallelize) |

## Goal

"Done and correct" means the app is **installable, accessible, documented, and verified** the way a single
headless Chromium run can never be. This sprint produces a production SPA build and packages it with the
reverse-proxy sidecar into a Dockerized image an operator can run against a daemon (DW1, DW13); gives the UI a
light/dark MUI theme and a responsive layout that holds down to tablet width; makes the whole surface
keyboard-operable and screen-reader-labelled with an automated `axe` check wired into CI; writes a
`docs/webui/` README + getting-started (run the sidecar, point it at a daemon, optional token) linked from the
root `README.md`; verifies the Electron-repackage seam by proving only the `DaemonConnection` implementation
swaps (no UI module changes, DW2/DW13); and runs the accumulated cross-browser + a11y/visual QA pass across
Safari/Firefox/Edge with a screen reader, recorded in `MANUAL-VERIFICATION.md`.

It builds on everything Sprints 0–8 shipped — the `DaemonConnection` transport and typed client (W0), the
sidecar config (W1), the dashboard/test-bench/matrix/transport/config UIs (W2–W5, W8), and the telemetry
client (W6–W7) — and it must not break any of them. This is the program's **close-out** sprint: it adds no
daemon behavior of its own, only packaging, polish, accessibility, and truthful documentation. When it is
done, the program's tracker is honest and complete — every `[A]`/`[B]`/`[RA]`/`[RB]` box across all sprints is
genuinely green, and the `[C]` cross-browser/manual box is the last one ticked, only after the manual pass is
actually run.

## Why

Up to now the app has run from a dev server in headless Chromium against a mock backend and, in Lane B, from
the dev's own browser. None of that is a thing an operator can install, none of it covers the browser engines
the program promised (Safari/Firefox/Edge), none of it proves a keyboard-only or screen-reader user can drive
the UI, and there is no user-facing documentation telling anyone how to run it. Those gaps have **no coverage
yet**: theming, responsive layout, accessibility, the Docker image, the getting-started docs, the Electron
seam, and the cross-browser/manual pass are each first delivered here.

Now is the only honest time to do it: the feature set is complete (W2–W8), so packaging it freezes a real
surface rather than a moving target, and the cross-browser/a11y pass can exercise the *whole* app instead of a
half-built one. The Charter (SPRINT-TRACKER §Charter) and the Final gates both demand a truthful close-out —
the `[C]` box must not be checked until the manual pass genuinely runs, the docs must describe the app as it
actually is, and `CHANGELOG.md`/`README.md` must record the user-visible daemon additions other sprints made
(the DW3 telemetry opt-in from W6/W7 and the DW11 `GET /config` from W8) so the published artifact's docs are
not lying about the daemon it talks to.

## Scope

**In scope**

- **A production build + a Dockerized reverse-proxy sidecar image** (DW1, DW13): build the SPA `dist`, package
  it with the Caddy/nginx sidecar from Sprint 1 into a runnable container image, and document running it
  against a daemon (point at a host:port, inject the optional Bearer token server-side).
- **Theming + responsive layout:** a light/dark MUI theme with a user-toggleable mode, and a layout that holds
  down to tablet width; a component test covers the theme switch.
- **Accessibility:** keyboard navigation across the full surface, screen-reader labels and live regions, and an
  automated `axe` a11y check wired into CI (Lane A) that fails the gate on violations; fix the issues it finds.
- **Docs:** a `docs/webui/README.md` + getting-started (run the sidecar, point it at a daemon, optional token),
  linked from the root `README.md`; update `CHANGELOG.md`/`README.md` for the web app and for the W6/W7 (DW3
  telemetry opt-in) and W8 (DW11 `GET /config`) daemon additions.
- **Electron seam verification:** a smoke that swaps the `DaemonConnection` implementation and confirms **only**
  the connection layer changes — no UI module imports change (DW2/DW13).
- **Cross-browser + manual a11y/visual pass:** run the accumulated `V-#` checks across Safari/Firefox/Edge (+ a
  mobile browser where noted) with a screen reader, recorded in `MANUAL-VERIFICATION.md` (V-1, V-2, V-3).

**Out of scope** (named owner — coordinate, do not duplicate)

- **Building a full Electron desktop app** — DW13 scopes this to **seam verification only**; the Electron build
  is a deliberate future, not this program. Verify the seam (only `connection.*.ts` swaps); do not build,
  bundle, or ship an Electron binary.
- **New features** of any kind (new commands, new panels, new telemetry) — owned by **Sprints W2–W8**,
  coordinate, do not duplicate. This sprint packages and polishes the existing surface; it adds no behavior.
- **The reverse-proxy sidecar *config*** itself (the Caddyfile/nginx.conf + dev proxy) — authored in **Sprint
  W1** (DW1). This sprint *packages* that config into a Docker image; it does not re-author the proxy rules.
- **Daemon-side telemetry / `GET /config` implementation** — owned by **Sprints W6/W7** (DW3/DW12) and **W8**
  (DW11). This sprint only *documents* those additions in `CHANGELOG.md`/`README.md`; it changes no Rust.

## Work items

### F1 — Production build + Dockerized reverse-proxy sidecar image (DW1, DW13)

- **Statement:** Produce a production SPA build and package it with the Sprint-1 reverse-proxy sidecar into a
  runnable Docker image, documented for pointing at a daemon (host:port + optional server-side Bearer token).
- **Where:** `webui/Dockerfile` (multi-stage: SPA build → static assets baked behind the sidecar), `webui/deploy/`
  (the Sprint-1 `Caddyfile`/`nginx.conf` consumed by the image; optionally `webui/deploy/compose.yaml` for a
  one-command run), `webui/deploy/README.md` for the run instructions. The image serves `webui/dist` and
  reverse-proxies `/api/*` + `/ws` to the daemon, injecting `Authorization: Bearer` server-side (DW1, DW9). The
  daemon surface it proxies is unchanged (`routes.rs:76` `create_router`); the `?token=` leak it avoids is
  `routes.rs:62`.
- **Priority:** P0 — this is the program's shippable artifact; DW13 makes it the v1 deliverable. Without it,
  "installable" is unproven.
- **Rationale:** DW13 locks v1 to "the SPA build + a Dockerized reverse-proxy sidecar, documented." The sidecar
  is load-bearing per DW1/DW9 (same-origin keeps CORS off — `cors_permissive` default false at `routes.rs:147`;
  server-side header injection is the only way the browser `WebSocket` authenticates without leaking the token
  in `?token=`). Packaging the Sprint-1 config into an image is what turns "a config you could run" into "an
  artifact an operator runs." The daemon stays single-purpose — no Rust serving code (DW1).
- **Approach:** Author a multi-stage `Dockerfile`: stage one runs `pnpm install --frozen-lockfile` + `pnpm
  build` to produce `dist`; stage two is the sidecar base (Caddy or nginx) with `dist` copied in and the
  Sprint-1 proxy config baked in, reading the daemon target and the Bearer token from environment variables
  (e.g. `DAEMON_TARGET`, `DAEMON_TOKEN`) so neither is compiled into the image. Expose the HTTP(S) port; keep
  TLS behind the same opt-in the Sprint-1 config used (`wss://` via Caddy automatic-HTTPS or an explicit `tls`
  directive). Document in `webui/deploy/README.md`: build the image, run it with the daemon target + optional
  token env vars, and how the injected header must match the daemon's own `auth_token`. The image must be
  buildable in Lane A (CI builds it) and runnable against a live daemon in Lane B. Do **not** re-author the
  proxy rules — consume Sprint 1's config; this item is the build + packaging, not the proxy logic.

### F2 — Theming (light/dark) + responsive layout

- **Statement:** Give the UI a light/dark MUI theme with a user-toggleable mode and a responsive layout that
  holds down to tablet width; cover the theme switch with a component test.
- **Where:** `webui/src/theme/` (the MUI theme definitions — `theme.ts` light/dark palettes, a
  `ThemeModeProvider.tsx` exposing the mode + toggle), the app shell layout (`webui/src/app/AppShell.tsx` or
  equivalent) for the responsive breakpoints, and a theme-mode control in the app header. Tests:
  `webui/src/theme/ThemeModeProvider.test.tsx`.
- **Priority:** P1 — it does not block the live data path, but a single fixed theme and a desktop-only layout
  fail the program's tablet-width and visual-QA promises (V-1).
- **Rationale:** DW4 fixes the stack as Material UI; MUI's `createTheme` + `ThemeProvider` + `useMediaQuery`
  give light/dark and responsive layout idiomatically, so this is configuration of the chosen stack, not new
  machinery. The cross-browser pass (V-1, step 3) explicitly resizes desktop→tablet and expects the layout to
  hold, so the responsive work must be real before that pass runs.
- **Approach:** Define light and dark MUI palettes in `webui/src/theme/`; wrap the app in a `ThemeModeProvider`
  that holds the mode, defaults sensibly (honor `prefers-color-scheme` via `useMediaQuery('(prefers-color-scheme:
  dark)')`), persists the user's explicit choice, and exposes a toggle rendered in the app header. Make the
  layout responsive with MUI's `Grid`/`Stack` + breakpoints so the dashboard, test-bench, matrix mixer, and
  transport controls reflow cleanly at tablet width (no horizontal scroll, no clipped controls; the matrix grid
  in particular must remain usable). The component test mounts the provider, asserts the default mode, toggles
  it, and asserts the applied MUI palette mode flips (e.g. `theme.palette.mode` and a themed element's
  resolved color). Keep view logic theme-agnostic — components consume theme tokens, they do not hard-code
  colors.

### F3 — Accessibility: keyboard nav, screen-reader labels, automated `axe` in CI

- **Statement:** Make the full surface keyboard-operable and screen-reader-labelled, and wire an automated
  `axe` a11y check into CI (Lane A) that fails the gate on violations; fix the issues it surfaces.
- **Where:** the `axe` integration in the test setup (`@axe-core/react` in dev and/or `jest-axe`/`axe-core` in
  Vitest + RTL component tests, and `@axe-core/playwright` in the headless E2E), plus targeted fixes across
  components — accessible names/`aria-label`s on icon-only controls (theme toggle, mute, stop), `aria-live`
  regions for status that changes asynchronously (the clip / stream-error badge, the "ducked" indicator,
  connection state, telemetry on/off), correct labelling of the sliders/scrubbers (W5) and the matrix grid
  (W4), and a sensible focus order across the dashboard, forms, matrix, and transport.
- **Priority:** P0 — an automated a11y gate is an explicit acceptance box, and the V-2 manual pass depends on
  the structural issues being fixed first so the human pass tests *experience*, not low-hanging structural bugs.
- **Rationale:** `axe` catches structural a11y issues (missing names, bad contrast, missing roles) cheaply and
  deterministically in CI; the V-2 manual pass (keyboard + screen reader) then judges what `axe` cannot (focus
  order *quality*, whether `aria-live` announcements actually read meaningfully). The two are complementary —
  `axe` is the floor, V-2 is the ceiling. Live regions matter here specifically because much of this UI updates
  asynchronously (poll-driven badges, telemetry ticks), and a screen-reader user gets nothing from a silently
  re-rendered badge.
- **Approach:** Add `axe` assertions to the Vitest + RTL component suite for the key views (dashboard header,
  command forms, matrix mixer, transport strip) and an `@axe-core/playwright` scan to the headless E2E that
  asserts zero serious/critical violations on the loaded app; run both as warnings-as-errors so a violation
  fails Lane A. Then fix what they flag: give every icon-only control an accessible name, mark async-updating
  status with `aria-live="polite"` (and label it per the API-CONTRACT's hygiene — the stream-error badge is
  "stream errors / rebuilds", not "buffer xruns", from `engine.rs:437-442`), ensure sliders expose
  `aria-valuetext` (seek as a time, speed as a multiplier), label matrix cells by src→dest, and verify a
  logical tab order. Do not paper over a real issue by suppressing the rule — fix the markup. The keyboard and
  screen-reader *experience* is verified by V-2 in Lane C, not here.

### F4 — Docs: `docs/webui/` README + getting-started, linked from root, changelog the daemon additions

- **Statement:** Write a `docs/webui/README.md` + getting-started covering run-the-sidecar, point-it-at-a-daemon,
  and the optional token; link it from the root `README.md`; and update `CHANGELOG.md`/`README.md` for the web
  app and the daemon additions other sprints made (DW3 telemetry opt-in, DW11 `GET /config`).
- **Where:** `docs/webui/README.md` (new — the web-app entry doc), the root `README.md` (a link to it + a brief
  "Web Control & Monitoring app" mention), `CHANGELOG.md` (the web app as a new artifact; the W6/W7 DW3
  telemetry opt-in; the W8 DW11 `GET /config` route). The getting-started references the F1 image and the
  Sprint-1 `webui/deploy/README.md` rather than duplicating the proxy config.
- **Priority:** P0 — "documented" is an acceptance box, and `MANUAL-VERIFICATION.md`'s "How to run" already
  points at `docs/webui/README.md` (`MANUAL-VERIFICATION.md` step 1), so the manual pass cannot run until this
  exists.
- **Rationale:** The program's Final gates require `README.md`/`CHANGELOG.md` to be truthful about user-visible
  items: the new web app, the DW3 telemetry opt-in, and the DW11 `GET /config` endpoint. DECISIONS.md flags DW3
  and DW11 as behavior-changing → changelog. The web app is a separate program (SPRINT-TRACKER intro), so its
  README lives in `docs/webui/` and the root README links to it rather than absorbing it.
- **Approach:** Write `docs/webui/README.md` as the web-app entry point: what it is (monitor + drive a running
  daemon), how to run it (build/run the F1 Docker image, or run the Sprint-1 sidecar directly), how to point it
  at a daemon (the daemon target env var), the optional Bearer token (set it in the sidecar's environment — DW9
  — never in a URL), and the telemetry opt-in (off by default, DW3). Link it from the root `README.md` under a
  short "Web Control & Monitoring app" heading. Update `CHANGELOG.md`: add the web app as a new artifact, and
  record the daemon-surface additions the program made — the DW3 opt-in telemetry mechanism (W6/W7) and the
  DW11 `GET /config` endpoint (W8) — attributing them to their sprints. Keep the prose evergreen and accurate;
  do not document features that do not exist (no Electron app, no hot-reload — config is read once at startup,
  DW8). Cross-link `API-CONTRACT.md` and `DECISIONS.md` for implementers.

### F5 — Electron seam verification (DW2/DW13)

- **Statement:** Verify the DW2 seam by swapping the `DaemonConnection` implementation behind a smoke and
  confirming **only** the connection layer changes — no UI module import changes — proving an Electron repackage
  needs no UI rewrite (DW13).
- **Where:** a seam smoke under `webui/src/api/` (e.g. `connection.seam.test.ts`) that constructs the app
  against a second, stub `DaemonConnection` implementation and asserts the UI renders/operates identically;
  reinforced by the Sprint-1 ESLint seam rule (no `fetch`/`WebSocket` outside `webui/src/api/`) which already
  guarantees no component reaches around the interface. A short seam note in `docs/webui/README.md` (or
  `webui/deploy/README.md`).
- **Priority:** P1 — it does not gate the live path, but it is the concrete proof DW13 asks for and the thing
  that keeps DW2's "no UI rewrite" promise honest.
- **Rationale:** DW2 promises an Electron repackage that swaps only the connection implementation. DW13 asks
  this program to **verify that seam, not build Electron**. The proof is structural: if the app can be driven
  through a different `DaemonConnection` implementation with zero UI-module changes, the seam holds. The
  Sprint-1 lint rule already prevents components from importing `fetch`/`WebSocket` directly, so the only thing
  left is to demonstrate a second implementation slots in cleanly.
- **Approach:** Provide a minimal alternate `DaemonConnection` implementation (an in-memory/stub transport, the
  same one the tests already use as a mock) and a smoke that boots the app wired to it, asserting representative
  views render and a representative command round-trips — with the only changed file being the connection
  implementation/wiring, not any component. Document that an Electron implementation would be exactly this swap:
  a native header-capable `WebSocket` + direct daemon URLs in a new `connection.*.ts`, with the UI untouched
  (DW2/DW13). Make explicit in the doc and the test name that this is **seam verification**, not an Electron
  build. Do not add Electron dependencies, a main process, or a packaging step — that is out of scope (DW13).

### F6 — Cross-browser + manual a11y/visual QA pass (V-1, V-2, V-3)

- **Statement:** Run the accumulated `MANUAL-VERIFICATION.md` checks (V-1 cross-browser parity, V-2
  keyboard/screen-reader, V-3 live-telemetry smoothness) across Safari/Firefox/Edge (+ a mobile browser where
  noted) with a screen reader, and record PASS/FAIL + browser/AT + notes + date inline.
- **Where:** `MANUAL-VERIFICATION.md` (V-1, V-2, V-3 — the seeds already exist; this sprint *runs* them and may
  append any genuinely-new manual step the final surface needs). Executed against the F1 production build served
  through the sidecar at a live daemon (per `MANUAL-VERIFICATION.md` "How to run").
- **Priority:** P0 for the program's close-out — this is the sole `[C]` box and the Final gate that proves the
  app works on the engines and with the assistive tech a headless run cannot cover.
- **Rationale:** Lane A (headless Chromium) and Lane B (the dev's own browser) cannot judge rendering parity
  across Safari/Firefox/Edge, the lived keyboard/screen-reader experience, or whether telemetry *looks* smooth
  — exactly what `MANUAL-VERIFICATION.md` exists for (DW7 Lane C). The seeds V-1/V-2/V-3 were written by earlier
  sprints to be run once, at the end, on the whole app; this sprint is that run.
- **Approach:** Build and serve the production SPA through the sidecar against a running daemon. For each of
  V-1/V-2/V-3, follow its steps in Chrome, Safari, Firefox, and Edge (and one mobile browser where the step
  says so), and with VoiceOver (macOS) / Narrator (Windows) for V-2: confirm rendering + interaction parity and
  the desktop→tablet resize (V-1), full keyboard operability + meaningful screen-reader announcements with no
  focus traps (V-2), and smooth ~15–20 Hz telemetry with audio unaffected by the opt-in toggle (V-3). Record
  the actual result on each `Result:` line (PASS/FAIL + browser/AT + notes + date). If a check reveals a real
  defect, fix it (theming/a11y/layout are in scope) and re-run; if it reveals an out-of-scope defect, log it in
  `docs/bugs.md` tagged `Sprint W9`. **Do not check the `[C]` acceptance box until this pass is genuinely run
  and recorded.**

## Caveats (do not chase ghosts / do not break)

- **Do not build a full Electron app.** DW13 scopes this to **seam verification only**. Adding Electron
  dependencies, a main process, or a desktop packaging step is out of scope; the deliverable is the smoke + the
  doc that proves only `connection.*.ts` swaps (F5). Verify the seam, do not build the product.
- **The `[C]` box must not be checked until the manual pass is actually run.** The Final gates and the Charter
  both forbid faking completion. `MANUAL-VERIFICATION.md` is run once, here, and the `[C]` box (F6) is the last
  box ticked — only after real PASS/FAIL results are recorded inline. An empty `Result:` line is not a pass.
- **Do not re-author the sidecar proxy rules.** The Caddyfile/nginx.conf + dev proxy are Sprint W1's (DW1).
  This sprint *packages* that config into a Docker image (F1); changing the proxy logic here duplicates W1 and
  risks diverging the two.
- **Do not add daemon code.** This sprint changes **no Rust**. The telemetry mechanism (DW3/DW12) and `GET
  /config` (DW11) are implemented by W6/W7/W8; F4 only *documents* them. There are no `[RA]`/`[RB]` lanes here.
  If the packaging or QA pass surfaces a real daemon defect, log it in `docs/bugs.md` tagged `Sprint W9` — do
  not patch the daemon under this sprint.
- **Do not fake unavailable data, and label things correctly.** The a11y labelling (F3) must keep the
  API-CONTRACT's label hygiene: the stream-error badge is "stream errors / rebuilds", not "buffer xruns"
  (`engine.rs:437-442`); cumulative `clips`/`xruns` are rates-by-diff, not instantaneous. If telemetry is off,
  progress/meters announce "unavailable" — `aria-live` must not announce a faked value. Sample position is real
  only when telemetry is on (W6); off, it is unavailable, not zero.
- **Do not suppress `axe` violations to make the gate pass.** F3's `axe` check fails the build on real issues;
  fix the markup (accessible names, live regions, contrast), never blanket-disable the rule. A suppressed
  violation is a faked completion.
- **Do not regress the secure-by-default token path (DW9).** The F1 image injects the Bearer header
  server-side; the SPA never puts the token in a URL. Do not add a `?token=` path to the packaged build.
- **Do not break the existing UI or its tests.** Theming/responsive/a11y changes touch shared components; the
  W2–W8 component and E2E suites must stay green. The smallest reasonable change wins (Charter).

## Tasks (ordered, TDD-first)

Subagents: packaging (F1), theming (F2), accessibility (F3), and docs/seam (F4/F5) are four largely-independent
tracks that can parallelize; the cross-browser/manual pass (F6) is sequenced last because it runs the whole,
finished surface. Each behavior-affecting task writes the failing test first, confirms it fails, writes the
minimum code to pass, and confirms green. Frontend tests are Vitest + React Testing Library (component, plus
`axe` assertions) and Playwright (E2E, headless, mock backend, plus `@axe-core/playwright`); there is **no
daemon change**, so no Rust lane runs.

1. **Theme switch + responsive layout (F2) — start here; it touches the shared shell other tracks build in.**
   Write a failing Vitest + RTL test mounting the `ThemeModeProvider`: assert the default mode (respecting
   `prefers-color-scheme`), toggle the mode, and assert the applied MUI `theme.palette.mode` flips and a themed
   element's resolved color changes. Confirm it fails, then implement the light/dark palettes, the provider +
   header toggle, and the responsive breakpoints in the app shell. Confirm green. (The desktop→tablet resize
   *experience* is V-1 in Lane C; the unit test proves the switch and palette wiring.)

2. **Automated `axe` a11y check + fixes (F3).** Add `axe` assertions to the component suite for the dashboard
   header, a command form, the matrix mixer, and the transport strip, and an `@axe-core/playwright` scan to the
   headless E2E asserting zero serious/critical violations. Run them; confirm they **fail** on the current
   markup (missing names / live regions / contrast). Fix the markup minimally — accessible names on icon-only
   controls, `aria-live="polite"` on async badges (labelled "stream errors / rebuilds"), `aria-valuetext` on
   sliders, labelled matrix cells, logical focus order — until the checks pass as warnings-as-errors. Confirm
   green. Do not suppress a real violation.

3. **Electron seam smoke (F5).** Write a failing seam smoke under `webui/src/api/` that boots the app against a
   stub `DaemonConnection` implementation and asserts representative views render and a representative command
   round-trips with **only** the connection implementation/wiring differing from the browser path. Confirm it
   fails (or drives out the wiring needed to make the swap clean), then make it pass. Confirm the Sprint-1 lint
   seam rule still holds (no `fetch`/`WebSocket` outside `webui/src/api/`). This proves DW2/DW13 structurally.

4. **Production build + Dockerized sidecar image (F1) — config/packaging, validated by the build + Lane B.**
   Author `webui/Dockerfile` (multi-stage: `pnpm build` → sidecar base with `dist` + the Sprint-1 proxy config
   baked in, daemon target + token from env) and, optionally, `webui/deploy/compose.yaml`. Make CI build the
   image (Lane A) and confirm `pnpm build` produces a clean `dist`. Document build/run in
   `webui/deploy/README.md`. Full proxy correctness against a real daemon (header injection, `/ws` upgrade, TLS)
   is verified in Lane B.

5. **Docs + changelog (F4).** Write `docs/webui/README.md` + getting-started (run the F1 image / Sprint-1
   sidecar, point at a daemon, optional token, telemetry opt-in off by default), link it from the root
   `README.md`, and add a short seam note (F5). Update `CHANGELOG.md`/`README.md` for the web app and the daemon
   additions — the DW3 telemetry opt-in (W6/W7) and the DW11 `GET /config` route (W8). Validated by a Lane A
   docs/link check; no daemon behavior changes here.

6. **Playwright headless a11y + theme E2E (Lane A).** Add a headless Playwright spec that loads the SPA, runs
   the `@axe-core/playwright` scan (zero serious/critical), toggles the theme and asserts the mode flips, and
   confirms the layout has no horizontal overflow at a tablet viewport. Confirm green in CI.

7. **Lane B live-daemon image session.** Build and run the F1 Docker image in front of a running `mqttaudio`
   daemon: confirm the SPA loads same-origin through the packaged sidecar, the dashboard/test-bench/matrix/
   transport work end-to-end, telemetry opt-in toggles cleanly, and the Bearer token never appears in any
   browser-issued URL (devtools network). Confirm a daemon restart is survived (backoff/reconnect from W1).

8. **Cross-browser + manual a11y/visual pass (F6) — last; runs the whole finished surface.** Serve the F1
   production build through the sidecar at a live daemon. Run V-1 (parity + desktop→tablet resize), V-2
   (keyboard + screen reader), and V-3 (telemetry smoothness) in Chrome/Safari/Firefox/Edge (+ mobile where
   noted) with VoiceOver/Narrator; record PASS/FAIL + browser/AT + notes + date on each `Result:` line. Fix
   in-scope defects and re-run; log out-of-scope defects in `docs/bugs.md` tagged `Sprint W9`. Only then check
   the `[C]` box.

## Files to create / touch

- **Create:** `webui/Dockerfile`, `webui/deploy/compose.yaml` (optional one-command run),
  `webui/src/theme/theme.ts`, `webui/src/theme/ThemeModeProvider.tsx`,
  `webui/src/theme/ThemeModeProvider.test.tsx`, `webui/src/api/connection.seam.test.ts` (Electron seam smoke),
  `webui/tests/e2e/a11y-theme.spec.ts` (Playwright `@axe-core/playwright` + theme/tablet check),
  `docs/webui/README.md` (web-app entry doc + getting-started), `axe` component-test helpers as needed.
- **Touch:** `webui/package.json` (dev-deps: `@axe-core/playwright` and the Vitest `axe` helper; any new
  build/lint/test scripts), the app shell (`webui/src/app/AppShell.tsx` or equivalent) + app header (theme
  toggle, responsive breakpoints), component files needing accessible names / `aria-live` / `aria-valuetext`
  (dashboard header, command forms, matrix mixer, transport/voice/input strips), `webui/deploy/README.md`
  (image build/run + seam note), the root `README.md` (link + web-app mention), `CHANGELOG.md` (web app; DW3
  telemetry opt-in; DW11 `GET /config`), `MANUAL-VERIFICATION.md` (V-1/V-2/V-3 results), `docs/bugs.md` (any
  out-of-scope discoveries tagged `Sprint W9`).

## Verification

### Lane A (CI / headless)

- `pnpm build` produces a clean `dist`; `tsc --noEmit` and eslint **warnings-as-errors** pass (including the
  Sprint-1 seam rule, which fails the build if any module outside `webui/src/api/` references `fetch`/
  `WebSocket`).
- The `webui/Dockerfile` builds in CI (multi-stage SPA build → sidecar image) (F1).
- Vitest + RTL: the theme-switch test (F2) proves the default mode and that toggling flips `theme.palette.mode`
  + a resolved color; the `axe` component assertions (F3) report zero serious/critical violations on the
  dashboard header, a command form, the matrix mixer, and the transport strip.
- The Electron seam smoke (F5) boots the app against a stub `DaemonConnection` and asserts representative views
  render + a command round-trips with only the connection layer differing — no UI-module change.
- Playwright headless against the mock backend: the `@axe-core/playwright` scan is clean (zero serious/critical),
  the theme toggle flips the mode, and there is no horizontal overflow at a tablet viewport (F2/F3).
- The docs/link check: `docs/webui/README.md` exists and is linked from the root `README.md`; `CHANGELOG.md`
  records the web app, the DW3 telemetry opt-in, and the DW11 `GET /config` route (F4).

### Lane B (real browser + live daemon)

- The F1 Docker image, run in front of a live daemon, serves the production SPA same-origin through the packaged
  sidecar; the dashboard, command test-bench, matrix mixer, and transport controls work end-to-end against real
  data.
- The telemetry opt-in toggles cleanly (real progress/meters when on, "unavailable" fallback when off — no faked
  values).
- The Bearer token is injected only by the sidecar: no browser-issued URL contains `?token=` (verified in
  devtools network), confirming DW9 holds in the packaged image.
- A daemon restart is survived via the Sprint-1 backoff/reconnect, served from the image without a page reload.

### Lane C (cross-browser / a11y / manual)

- **V-1 — cross-browser rendering & interaction parity:** the app renders and operates without layout breakage
  in Chrome, Safari, Firefox, and Edge; a play/stop/voice-volume/channel-mapped play drives the dashboard and
  audio; the desktop→tablet resize holds (F2). PASS/FAIL + browser + notes + date recorded.
- **V-2 — keyboard & screen-reader walk-through:** the full surface (dashboard, command forms, matrix, transport)
  is keyboard-operable with a sensible focus order and no focus traps; controls announce meaningful labels and
  `aria-live` regions (clip / "stream errors / rebuilds" badge, "ducked" indicator, telemetry/connection state)
  are announced on change, under VoiceOver/Narrator (F3). PASS/FAIL + AT + notes + date recorded.
- **V-3 — live telemetry smoothness:** with telemetry on against a live daemon, progress bars/playhead and
  output + per-input meters update smoothly at ~15–20 Hz with no visible stutter; audio is unaffected by the
  opt-in; toggling off cleanly stops the live updates and falls back to polling. PASS/FAIL + notes + date
  recorded.
- The `[C]` box is checked **only** after all three are genuinely run and their `Result:` lines filled.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 9)

- [ ] A production build + a Dockerized reverse-proxy sidecar image are produced and documented; the Electron-repackage seam is verified (only the connection layer swaps) (DW13) `[A]`
- [ ] Theming (light/dark) and a responsive layout work down to a tablet width; component tests cover the theme switch `[A]`
- [ ] Keyboard navigation and screen-reader labels pass an automated a11y check (axe) in CI `[A]`
- [ ] `docs/webui/` gains a README + getting-started (run the sidecar, point it at a daemon, optional token), linked from the main `README.md` `[A]`
- [ ] Cross-browser (Safari/Firefox/Edge) + manual a11y/visual QA pass recorded in `MANUAL-VERIFICATION.md` `[C]`

## Behavior-change / changelog notes

This sprint changes **no daemon behavior of its own** — it is packaging, polish, accessibility, and
documentation; the Rust binary is untouched and there are no `[RA]`/`[RB]` lanes. Its changelog work is to make
the published docs truthful about the daemon additions earlier sprints made:

- **DW3 — telemetry is opt-in + subscriber-gated** (implemented in Sprints W6/W7): record in `CHANGELOG.md`
  that the daemon gained an opt-in, subscriber-gated real-time telemetry mechanism (live sample position,
  output/input meters, a state-event WebSocket channel), **off by default** with ~no RT cost when off. Flagged
  behavior-changing by DECISIONS.md.
- **DW11 — read-only `GET /config`, secrets redacted** (implemented in Sprint W8): record the new endpoint
  returning the running config with `auth_token`/`mqtt_password` redacted. Flagged behavior-changing by
  DECISIONS.md.
- **The web app itself** is a new user-visible artifact: record it in `CHANGELOG.md`/`README.md` with a link to
  `docs/webui/README.md`.

F1–F3 and F5–F6 (image, theming, a11y, seam smoke, manual pass) are frontend/packaging/docs — they introduce
**no** daemon-observable change. If the packaging or QA pass surfaces a real daemon defect, log it in
`docs/bugs.md` tagged `Sprint W9` rather than patching the daemon here.

## Definition of Done

Lane A green (`pnpm build` clean `dist` · `tsc --noEmit` · eslint **warnings-as-errors** incl. the Sprint-1 seam
rule · Vitest + RTL incl. the theme-switch test and the `axe` component assertions · the Electron seam smoke ·
Playwright headless incl. `@axe-core/playwright` clean + theme/tablet checks · the `webui/Dockerfile` builds in
CI) · Lane B green (the F1 image in front of a live daemon serves the SPA same-origin, the full feature set works
end-to-end, telemetry opt-in toggles cleanly with no faked values, the token never appears in a URL, a daemon
restart is survived) · Lane C run and recorded (V-1 cross-browser + tablet, V-2 keyboard + screen reader, V-3
telemetry smoothness across Safari/Firefox/Edge with a screen reader; PASS/FAIL + notes + date on every
`Result:` line; the `[C]` box checked only after) · `docs/webui/README.md` + getting-started written and linked
from the root `README.md` · `CHANGELOG.md`/`README.md` updated for the web app, the DW3 telemetry opt-in, and
the DW11 `GET /config` route · no daemon code changed (no `[RA]`/`[RB]` lanes) · new tests added with no coverage
reduction · out-of-scope discoveries logged in `docs/bugs.md` tagged `Sprint W9` · committed atomically on the
sprint branch with a clear message.
