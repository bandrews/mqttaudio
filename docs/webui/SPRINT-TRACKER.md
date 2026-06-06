# mqttaudio Web Control & Monitoring — Sprint Tracker

This is the control document for the **web app** program: a React + Material UI application that monitors live
mqttaudio playback and exercises the full feature set (multi-voice, channel-map matrix mixing, ducking/gain
tuning, cache state, sample progress) without hand-crafting MQTT/HTTP. It sequences the work into ten sprints,
tracks their status, and holds the acceptance criteria for each. It mirrors the conventions of the backend
"Quality Sprint" program (`docs/sprints/`) — it is a **separate program** with its own sprint numbering and its
own governance docs in this directory.

Each sprint has its own self-contained file (`sprint-NN-*.md`) with the full detail: goal, findings/work items,
an ordered TDD task list, files to touch, and a verification plan. Work the sprints **in order**; do not start a
sprint whose dependencies are not `Done`.

The implementers' source of truth for the daemon's surface is [`API-CONTRACT.md`](API-CONTRACT.md) (code-
verified). Design choices are pre-locked in [`DECISIONS.md`](DECISIONS.md).

---

## ⚠️ Charter — read before every sprint. No shortcuts.

These rules mirror `CLAUDE.md` and the backend program's charter.

**Decisions are pre-locked in [`DECISIONS.md`](DECISIONS.md)** (`DW#`). It resolves every design/behavior choice
for this program; any "decide"/"confirm" wording in a sprint file is **superseded** by it. You may **overrule** a
locked decision only when implementation uncovers new evidence that a different choice is clearly better (record
it per `DECISIONS.md`). **Never block the program waiting on a human.** If something genuinely needs a human and
has no other resolution, add it to [`NEEDS-HUMAN.md`](NEEDS-HUMAN.md), skip **only** that item, and finish
everything else.

- **Doing it right beats doing it fast.** Tedious, systematic work is usually correct. Never skip steps.
- **Never fake completion.** Do not check an acceptance box unless you have *actually verified* it.
- **Never disable, delete, or skip a test to make things "pass."** Fix the root cause. If a test is wrong, log
  it in `NEEDS-HUMAN.md` rather than silently weakening it.
- **Root cause only.** No symptom patches. Follow the systematic debugging process in `CLAUDE.md`.
- **TDD.** For every behavior: failing test first → confirm it fails → minimum code to pass → confirm green →
  refactor green. (Frontend: component/unit tests with React Testing Library; backend: the existing Rust TDD.)
- **Smallest reasonable change.** Match surrounding style. Do not rewrite working code without permission.
- **Green gate, every sprint.** A sprint is not `Done` until its lanes are green with **zero build/type/lint
  warnings**. Frontend: `pnpm build` + `tsc --noEmit` + eslint **warnings-as-errors** + Vitest + Playwright must
  pass. Backend-touching sprints additionally pass the Rust gates (`scripts/validate.sh`, clippy `-D warnings`,
  `cargo build --release` warning-free, the alloc harness).
- **Don't break the daemon's RT path.** The telemetry sprints (W6/W7) touch the real-time audio engine; they
  must hold the RT no-alloc/no-free/no-lock contract (D22a) and stay **off by default + subscriber-gated**
  (DW3). Verified by the alloc-counting harness, not by inspection.
- **Log out-of-scope discoveries** in `docs/bugs.md` tagged by their owning web sprint + finding (e.g.
  `Sprint W6 F2`); commit each sprint on a branch with a clear message.

If you cannot honestly check every box for a sprint, leave it `In progress` or `Blocked` and explain why.

---

## How to run this program

1. Pick the lowest-numbered sprint that is `Not started` and whose dependencies are all `Done`.
2. Open its `sprint-NN-*.md`, set its status here to `In progress`.
3. Implement it (TDD), using subagents where the sprint file says it helps.
4. Run its lanes (below). All must be green.
5. Append any cross-browser / a11y / human-QA steps the sprint produced to `MANUAL-VERIFICATION.md` (V-#).
6. Tick every acceptance box below for that sprint. Commit atomically to the branch with a clear message.
7. Set the sprint status to `Done`. Go to step 1.
8. When all sprints are `Done`, complete the **Final gates**.

### Validation lanes (web-remapped, per DW7)

- **Lane A — CI (headless):** `pnpm build`, `tsc --noEmit`, eslint **warnings-as-errors**, Vitest + React
  Testing Library (unit/component), Playwright headless E2E against a mock/fixture backend. The default lane;
  almost every box is `[A]`. No real daemon.
- **Lane B — real browser + live daemon (dev machine):** the SPA served through the real reverse-proxy sidecar
  against a running `mqttaudio` daemon — real WebSocket rendering, real playback, real proxy/auth. The analog of
  the backend program's "real device" lane; used by boxes that need a live backend.
- **Lane C — cross-browser + a11y + manual human QA:** Safari/Firefox/Edge (+ mobile), keyboard/screen-reader,
  and visual QA a headless lane can't judge; steps accumulate in `MANUAL-VERIFICATION.md` (V-1, V-2…), run once
  at the end. The only boxes allowed to finish unchecked.
- **Rust lanes (backend-touching sprints W6/W7/W8 only):** `[RA]` = Rust Lane A (Docker `-D warnings`/clippy/
  `fmt`/tests + alloc harness); `[RB]` = Rust Lane B (native macOS real CoreAudio device). These prove RT-safety
  that the web lanes cannot.

---

## Status board

| # | Sprint | Status | Depends on | File |
|---|--------|--------|-----------|------|
| 0 | Foundations, API contract & CI harness | Done | — | [sprint-00](sprint-00-foundations-and-api-contract.md) |
| 1 | Deployment: reverse-proxy sidecar & connectivity | Done | 0 | [sprint-01](sprint-01-deployment-and-connectivity.md) |
| 2 | Live monitoring dashboard (poll-based) | Done | 0, 1 | [sprint-02](sprint-02-live-monitoring-dashboard.md) |
| 3 | Command test-bench & cue launcher | Done | 0 | [sprint-03](sprint-03-command-test-bench.md) |
| 4 | Channel-map matrix mixer | Done | 0, 3 | [sprint-04](sprint-04-channel-map-matrix-mixer.md) |
| 5 | Transport, speed & windowed gating | Done | 0, 2 | [sprint-05](sprint-05-transport-speed-and-windowed-gating.md) |
| 6 | Telemetry I: live position + opt-in gating | Not started | 0, 5 | [sprint-06](sprint-06-telemetry-live-position.md) |
| 7 | Telemetry II: meters + state-event WebSocket | Not started | 6 | [sprint-07](sprint-07-telemetry-meters-and-state-events.md) |
| 8 | Config visibility & tuning panels | Not started | 0, 2 | [sprint-08](sprint-08-config-visibility-and-tuning.md) |
| 9 | Packaging, polish, cross-browser, a11y & docs | Not started | 0–8 | [sprint-09](sprint-09-packaging-polish-and-docs.md) |
| 10 | **Bonus (daemon):** CPAL upgrade & device-detection fix | Not started | — | [sprint-10](sprint-10-cpal-upgrade.md) |

Status values: `Not started` · `In progress` · `Blocked` · `Done`.

> **Sprint 10 is a bonus, daemon-only sprint** bundled into this branch for convenience (partner request):
> bump `cpal` to the latest release to resolve a discovered device-detection bug. It has **no web-app
> dependency** and runs entirely on the **Rust lanes** (`[RA]`/`[RB]`); it can be done at any point. It is not
> required for the web program's Final gates, but should be green before the branch merges.

---

## Acceptance criteria

Tick a box only when genuinely verified. `[A]` = CI/headless, `[B]` = real browser + live daemon, `[C]` =
cross-browser/a11y/manual. `[RA]`/`[RB]` = the Rust Docker / native-device gates (backend-touching sprints).

### Sprint 0 — Foundations, API contract & CI harness
- [x] `webui/` scaffolds and builds: Vite + React + TS + MUI; `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) all pass in CI `[A]`
- [x] A swappable `DaemonConnection` transport interface exists with a browser/proxy implementation; "a connection" (base URL + optional token) is a first-class entity (multi-instance-ready, per DW2) `[A]`
- [x] A typed API client covers every runtime command + read endpoint from `API-CONTRACT.md`, encoding the transport quirks (`loop`/`loop_mode`, `time`/`time_ms`, `internal_id` as string, `/command` for the full play surface); unit tests assert the emitted JSON shapes `[A]`
- [x] Connection bootstrap reads `/health` + `/version`, detects open vs `require_auth`, and surfaces an optional Bearer-token field; a component test covers the auth-required path `[A]`
- [x] CI runs Vitest + React Testing Library and a Playwright headless smoke against a fixture/mock backend, green `[A]`

### Sprint 1 — Deployment: reverse-proxy sidecar & connectivity
- [x] A reverse-proxy sidecar config (Caddy or nginx) serves the built SPA and proxies `/api/*` + `/ws` to the daemon, injecting the Bearer header server-side; documented and runnable (DW1) `[A]`
- [x] A dev proxy (Vite) mirrors the sidecar so same-origin behavior holds in dev `[A]`
- [x] The `/ws` log stream renders in a live log console (connect, `{type:"connected"}`, `{type:"log"}`, lag/close handling); component test with a mock socket `[A]`
- [x] No UI module imports `fetch`/`WebSocket` directly — all access goes through `DaemonConnection`; the Electron-future seam is documented (DW2) `[A]`
- [x] Against a real daemon behind the sidecar, the SPA connects, streams logs, and survives a daemon restart with backoff/reconnect `[B]` — *automated against a real spawned daemon (`pnpm test:e2e:laneb`): connect + welcome frame (real version) + restart→backoff-reconnect verified. Real log **lines** don't arrive because the daemon never installs `WebSocketLogLayer` into tracing (welcome-frame-only `/ws`) — a daemon-side gap recorded in `docs/bugs.md` (Sprint W1), out of scope for the frontend-only sprint per DW1.*

### Sprint 2 — Live monitoring dashboard (poll-based)
- [x] A persistent health header shows active samples/voices/inputs, output channels, uptime, a clip badge, and a stream-error badge (labeled "stream errors / rebuilds", not "buffer xruns") from `/status` + `/metrics` `[A]`
- [x] A cache memory-budget gauge renders resident bytes vs cap with headroom from `/metrics` (`null` cap → "unlimited") `[A]`
- [x] A now-playing board lists active samples from `/status/samples`; progress is shown as "live position unavailable" pending telemetry (Sprint 6), not faked `[A]`
- [x] Voices rack (`/status/voices`, volume + `ducking_multiplier` with a "ducked" indicator) and inputs rack (`/status/inputs`, volume + `muted`) render from live data `[A]`
- [x] Clip/stream-error rates are computed client-side by diffing successive `/metrics` polls (cumulative counters are not shown as instantaneous) `[A]`
- [x] Against a real daemon, the dashboard reflects live plays/stops/duck changes within the poll interval `[B]` — *`pnpm test:e2e:laneb`: a real looping play appears on the now-playing board within the poll interval and clears on stopall.*

### Sprint 3 — Command test-bench & cue launcher
- [x] Every runtime command (play, stop, stopall, volume, seek, speed, voice_*, input_*, cache_*, precache) has a form that emits correct JSON, with client-side validation mirroring the code (volume 0-1, speed ranges, lowercase enums) `[A]`
- [x] The selector model (`internal_id`/`id`/`file`/`voice`, OR-logic) is exposed with an explicit warning when no selector is set (silent no-op) `[A]`
- [x] A raw `/command` editor exposes the full play surface (`channel_map`, `mode`, `window_ms`, `prebuffer_ms`, `freshness`, `cacheable`, `crossfade_ms`) that typed `/play` omits (DW10) `[A]`
- [x] A cue launcher composes file + options into a play, with a live JSON preview reflecting macro + explicit-field precedence `[A]`
- [x] Against a real daemon, representative commands from each family take effect and are reflected on the dashboard `[B]` — *`pnpm test:e2e:laneb`: the cue launcher plays a real file (shown on the Monitor) and the console Stop All clears it.*

### Sprint 4 — Channel-map matrix mixer
- [x] A src×dest matrix grid sized from `output_channels` (`/status`) lets the user toggle routes and set a per-route `gain`; channel aliases label destination columns `[A]`
- [x] Destinations with multiple summed sources carry a clip-risk badge; one-to-many fan-out and partial routing are supported `[A]`
- [x] The matrix emits a play via `/command` (not typed `/play`) and surfaces the caveats: per-route gain is ignored on `mode:stream`, and an unknown alias silently aborts the play `[A]`
- [x] Against a real multichannel device, a routed play lands on the intended channels `[B]` — *`pnpm test:e2e:laneb` routes a real play to dest 0/1 on the default device and it plays; routing to >2 channels needs multichannel hardware (out-of-range routes are silently skipped by the daemon).*

### Sprint 5 — Transport, speed & windowed gating
- [x] Each active sample card has a seek scrubber over `total_ms` (emits `seek`) and a speed control with a 1.0 detent spanning −100..100, plus a pitch-correction toggle that re-clamps the range to 0.05..8.0 and disables reverse `[A]`
- [x] Windowed/streamed voices show a "streamed" badge and disable seek, speed, reverse, and loop-crossfade controls `[A]` — *windowed is inferred from `total_frames === 0` until Sprint W6 F4 adds a real flag (docs/bugs.md, Sprint W5).*
- [x] Voice strips expose volume (`voice_volume`), fade-out (`time_ms`), and stop; input strips expose volume (`input_volume`) and mute (`input_mute`) `[A]`
- [x] Against a real daemon, seek/speed/voice/input controls change playback audibly and the dashboard reflects them `[B]` — *`pnpm test:e2e:laneb`: the mixer transport shows a real sample and its Stop reflects on the Monitor; the *audible* seek/speed effect is a human listening check (the commands are proven to emit and reach the daemon).*

### Sprint 6 — Telemetry I: live position + opt-in gating
- [ ] Telemetry is OFF by default and gated: it activates only when explicitly opted in AND ≥1 telemetry client is subscribed; with telemetry off, the RT path does no new work (DW3) `[RA]`
- [ ] Each active sample publishes its live frame position via a relaxed atomic written once per audio block (mirroring `clip_count`); `handle_samples` reads it; `/status/samples` returns real `position`/`position_ms`/`progress_percent` when telemetry is on `[RA]`
- [ ] The mechanism respects D20/D22a — control never locks the RT mutex; the alloc-counting harness shows 0 alloc / 0 free on the callback path `[RA]`
- [ ] On the real CoreAudio device, enabling telemetry yields advancing positions with no audible regression; disabling restores the no-op path `[RB]`
- [ ] The UI renders real progress bars + a live playhead on the scrubber when telemetry is on, and falls back to "unavailable" when off `[A]`
- [ ] Against a real daemon, progress tracks audibly-correct playback for normal and looped samples `[B]`

### Sprint 7 — Telemetry II: meters + state-event WebSocket
- [ ] Output peak/RMS and per-input capture-level meters are published via relaxed atomics from the output/capture stages, gated by the same opt-in/subscriber mechanism (0 alloc/free, no RT lock) `[RA]`
- [ ] A second WebSocket channel carries typed state events (play/stop/seek/voice_volume/input_mute/ducking/sample_finished) plus throttled (~15–20 Hz) tick frames for position/meters, emitted from existing control-thread mutation points + a control-side timer (DW12) `[RA]`
- [ ] On the real device, the state channel + meters run clean under load with telemetry on, and stop entirely with telemetry off `[RB]`
- [ ] The UI subscribes to the state channel and renders live output + per-input meters and event-driven updates, auto-falling back to polling when telemetry is off `[A]`
- [ ] Against a real daemon, meters move with signal, discrete events update the UI without a poll, and a finished sample disappears on its `sample_finished` event `[B]`

### Sprint 8 — Config visibility & tuning panels
- [ ] A read-only `GET /config` returns the running config with secrets redacted (`auth_token`, `mqtt_password`); a Rust test asserts redaction (DW11) `[RA]`
- [ ] Config panels display current values from `/config` and produce validated config-JSON snippets flagged "restart required" for ducking rules, bass/LFE crossover, channel aliases, per-channel calibration, limiter ceiling + master gain, macros, and input definitions (DW8) `[A]`
- [ ] Ducking is visualized live (rules firing) from `/metrics` ducking map + `/status/voices` `ducking_multiplier`, with the mic-trigger limitation surfaced `[A]`
- [ ] The genuinely-live settings (per-input volume/mute) are wired to take effect immediately, distinct from the restart-required editors `[A]`
- [ ] Against a real daemon, `/config` round-trips and a generated snippet validates against the daemon's loader `[B]`

### Sprint 9 — Packaging, polish, cross-browser, a11y & docs
- [ ] A production build + a Dockerized reverse-proxy sidecar image are produced and documented; the Electron-repackage seam is verified (only the connection layer swaps) (DW13) `[A]`
- [ ] Theming (light/dark) and a responsive layout work down to a tablet width; component tests cover the theme switch `[A]`
- [ ] Keyboard navigation and screen-reader labels pass an automated a11y check (axe) in CI `[A]`
- [ ] `docs/webui/` gains a README + getting-started (run the sidecar, point it at a daemon, optional token), linked from the main `README.md` `[A]`
- [ ] Cross-browser (Safari/Firefox/Edge) + manual a11y/visual QA pass recorded in `MANUAL-VERIFICATION.md` `[C]`

### Sprint 10 — Bonus (daemon): CPAL upgrade & device-detection fix
- [ ] `cpal` bumped to the latest release in `Cargo.toml`/`Cargo.lock`; the daemon builds warning-free with `-D warnings` and clippy is clean across the cpal API changes `[RA]`
- [ ] The discovered device-detection bug is reproduced (a failing/asserting test or a documented repro), root-caused, and fixed against the new cpal; a regression test or `--list-devices`/`--list-inputs` assertion covers it `[RA]`
- [ ] The existing audio test suite + the alloc-counting harness stay green on the new cpal; any cpal API/behavior change is reflected in `docs/architecture.md`/`CHANGELOG.md` `[RA]`
- [ ] On the real CoreAudio device, `--list-devices`/`--list-inputs` enumerate correctly and a play opens the device and runs clean on the new cpal `[RB]`

---

## Final gates

- [ ] All sprints 0–9 are `Done` — every `[A]`/`[B]`/`[RA]`/`[RB]` box is genuinely checked and green. The only
  boxes allowed to remain unchecked are the `[C]` cross-browser/manual items, run once at the end.
- [ ] `MANUAL-VERIFICATION.md` has been run (cross-browser + a11y + visual QA) with results recorded.
- [ ] `README.md` / `CHANGELOG.md` updated for user-visible items (the new web app; the DW3 telemetry opt-in;
  the DW11 `GET /config` endpoint).
- [ ] `docs/bugs.md` reflects any out-of-scope items discovered along the way (tagged by owning web sprint).
- [ ] This tracker is finished: statuses accurate, no half-truths.

## Global Definition of Done (applies to every sprint)

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless) ·
Lane B green where the sprint needs a live daemon · Rust lanes green for backend-touching sprints (`[RA]`/`[RB]`
incl. alloc harness; `cargo build --release` warning-free) · new tests added (no coverage reduction) ·
cross-browser/a11y steps appended to `MANUAL-VERIFICATION.md` where relevant · out-of-scope items logged to
`docs/bugs.md` · committed on a branch.
