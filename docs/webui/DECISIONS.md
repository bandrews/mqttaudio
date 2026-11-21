# Locked Decisions — Web Control & Monitoring app

Every open design choice for the web-app program is **resolved here, upfront**, so no sprint stalls waiting
on a human. Treat each as the **default ruling**: build the locked choice and **do not stop to ask**. Wherever
a sprint file says "decide", "confirm with the partner", or similar, **this document supersedes it** — the
decision is already made.

**You may overrule a locked decision — but only when implementation uncovers new information** (a measurement,
a code reality, a browser limitation, a test result) that makes a different choice clearly better. When you do:
(1) override because the *evidence* demands it, not preference; (2) record what you found in `docs/bugs.md` (and
the changelog if user-visible) and update the affected sprint file; (3) keep it consistent with the program's
intent — RT-safety, open-by-default behavior, YAGNI, no shortcuts. Absent new evidence, follow the locked
choice and keep moving.

These decisions were taken with the partner up front (the two scope questions: backend-telemetry scope and
deployment model) plus the conventions inherited from the backend program. `DW#` numbers are **global and
monotonic** across this file (the web program's namespace; distinct from the backend `D1–D49`).

---

## Program-wide

- **DW1 · Deployment = reverse-proxy sidecar (v1).** Ship a lightweight static server (Caddy or nginx) that
  serves the built SPA **and** reverse-proxies `/api/*` and `/ws` to the daemon, injecting the `Authorization:
  Bearer` header server-side and terminating TLS (`wss://`). *Why:* it makes the SPA same-origin (CORS stays
  off — `cors_permissive` default false, `routes.rs:147`), and it fixes the load-bearing browser limitation
  that the native `WebSocket` API cannot set an `Authorization` header, so a direct-to-daemon app would leak
  the token in the `?token=` URL on a `require_auth` daemon (`routes.rs:62`). The daemon stays single-purpose
  (no new Rust serving code). *Partner decision.*

- **DW2 · Electron-ready architecture; no Electron build in v1.** All daemon access goes through a single
  swappable `DaemonConnection` transport interface; **"a connection" (base URL + optional token) is a
  first-class entity** and the data model is multi-instance-capable even though v1 connects to one daemon
  behind the proxy. No React UI module imports `fetch`/`WebSocket` directly. *Why:* the partner wants the
  option to repackage as an Electron desktop app later (native header-capable WebSocket, multi-instance mDNS
  discovery) with **no UI rewrite** — only a new connection implementation. *Partner decision.*

- **DW3 · Telemetry is opt-in AND subscriber-gated.** Every new real-time telemetry mechanism (live sample
  position atomics, output/input meters, the state-event WebSocket + throttled ticks) is **OFF by default**,
  costs ~nothing on the RT audio path when off, and activates **only when the user has explicitly opted in
  *and* at least one telemetry client is subscribed**. *Why:* the partner's explicit constraint — telemetry
  "may impact performance", so it must not run unless someone is actively watching. *Behavior change* (new
  control + endpoints) — changelog it. *Partner decision.* See DW12 for the mechanism.

  - **DW3 · W6 implementation note (landed).** The opt-in surface is `GET/POST /telemetry` (`{enabled: bool}`),
    a single shared `AtomicBool` flipped by the SPA's Telemetry switch — not the per-read "transient subscriber"
    TTL the W6 brief sketched. The RT gate is this flag: with telemetry off the callback does one relaxed load
    and skips the per-sample position store (0-alloc/0-free both ways, proven by `tests/alloc_harness.rs`). The
    **subscriber-count** half of the gate ("≥1 client subscribed") is only meaningful once the state-event
    WebSocket exists, so it is **deferred to Sprint W7** (per the W6 F1 note); for the poll-based W6 path the
    explicit opt-in flag is the gate. Recorded here per the override rule.

- **DW4 · Frontend stack.** Vite + React + TypeScript + Material UI (MUI). Server state via a query/cache layer
  (TanStack Query) over the typed API client; light view-local state via Zustand or React Context (no Redux).
  Tests: Vitest + React Testing Library (unit/component) and Playwright (E2E). Package manager: pnpm. *Why:*
  matches the partner's React + Material UI ask; a query/cache layer fits the poll-heavy read model; all
  conventional, low-bikeshed choices.

- **DW5 · Talk to the HTTP server exclusively, never MQTT.** The REST API mirrors every MQTT command, and a
  browser cannot speak MQTT. *Why:* one transport, and it is the one a browser/Electron app can use.

- **DW6 · Read model = poll by default, push when telemetry is on.** Default poll cadences: `/metrics` and
  `/status*` at **1–2 s**, `/status/cache` at **2–5 s**, `/version` once. When the telemetry WebSocket is
  connected (DW3), discrete state events + tick frames supersede polling for the data they carry; polling
  remains the always-available fallback (and the only source for cache/version). *Why:* this is exactly what is
  live-pollable today; it avoids over-polling and degrades gracefully when telemetry is off.

- **DW7 · Lane remap (web), with Rust lanes for backend-touching sprints.** The audio program's device-based
  lanes do not apply to a React app, so:
  `[A]` = **CI lane** — `pnpm build`, `tsc --noEmit`, eslint **warnings-as-errors**, Vitest + RTL, Playwright
  headless against a mock/fixture backend. The default, near-every-box lane.
  `[B]` = **real browser + live daemon** session on the dev machine — the SPA served through the real sidecar
  against a running `mqttaudio` daemon (real WS rendering, real playback, real proxy/auth). The analog of the
  old "real device" lane.
  `[C]` = **cross-browser + a11y + manual human QA** — Safari/Firefox/Edge, keyboard/screen-reader, visual QA,
  recorded in `MANUAL-VERIFICATION.md` (V-1, V-2…). The only boxes allowed to finish unchecked.
  Sprints that change the **daemon** (W6/W7/W8) additionally run the existing **Rust** gates, tagged `[RA]`
  (Rust Lane A — Docker `-D warnings`/clippy/`fmt`/tests + the alloc-counting harness) and `[RB]` (Rust Lane B
  — native macOS real CoreAudio device). *Why:* RT-touching changes still need the audio program's safety
  gates; the web `[A]`/`[B]` cannot prove RT-safety.

- **DW8 · Config editing = emit-snippet + restart-required; only per-input volume/mute are live.** The daemon
  reads config **once at startup** (no hot-reload, no SIGHUP, no runtime config-mutation command — verified:
  `resolve_memory_cap`/engine init at `main.rs:410-421`, `:584`). So config panels **display** the running
  config (via DW11's `GET /config`) and **emit a validated config-JSON snippet** the operator applies and
  restarts; they must clearly say "restart required". The **only** config-derived state changeable live is a
  running input's volume/mute (`SetInputVolume`/`SetInputMute`, `main.rs:2020-2040`); those editors apply
  immediately. *Why:* building hot-reload is out of scope and YAGNI; the UI must not imply live application it
  cannot deliver.

- **DW9 · Security & token handling.** The optional Bearer token is held in memory; persisting it (e.g.
  `localStorage`) is allowed only with a visible warning. **Behind the proxy (DW1) the SPA never puts the token
  in a URL** — the proxy injects the header. If a deployment ever bypasses the proxy, the browser `WebSocket`
  can only authenticate via `?token=` and the UI must show a "token visible in URL/logs" warning. The UI reads
  `require_auth`/open mode (via `/health` behavior) to decide whether to require a token. Constant-time compare
  already exists server-side (`ct_eq`, `routes.rs:20`). *Why:* the browser WS header limit + plaintext-token
  exposure are real; the proxy is what makes the secure path the default path.

- **DW10 · The full command surface goes through `/command`.** The command console and the matrix mixer POST
  **raw command JSON to `/command`**, because the typed `/play` silently drops `channel_map`, `mode`,
  `window_ms`, `prebuffer_ms`, `freshness`, `cacheable` (`handlers.rs:181-198`). Typed endpoints are used only
  where they carry every field the UI needs. *Why:* otherwise the routing/streaming surface is unreachable.

## Sprints 6–7 — Telemetry (the mechanism)

- **DW11 · Read-only `GET /config`, secrets redacted.** Add a read-only endpoint returning the running config
  with `auth_token` and `mqtt_password` (and any future secret) **omitted or masked**. Gate it like the other
  status routes (open by default; covered by `require_auth` when set). *Why:* the config panels (DW8) need to
  read current values; the daemon must never leak secrets to do it.

- **DW12 · Telemetry mechanism = lock-free RT atomics + a control-side throttled broadcaster.** Continuously-
  changing values (sample position, output/input meters) are published by the **RT thread** into pre-sized
  **relaxed atomics** (exactly the existing `clip_count`/`xruns` pattern — `mixer.rs:1167`, `engine.rs:439`),
  written once per audio block, **0 alloc / 0 free, no lock**. The **control thread never locks the RT mutex**
  (D22a invariant — `rt_engine.rs:302-323`); instead it reads the atomics. Discrete events (play/stop/seek/
  voice/ducking/sample_finished) are emitted from the existing control-thread mutation points (`main.rs`
  `start_sample`, finish reconciliation, `refresh_snapshot`, ducking notify). A control-side **~15–20 Hz
  timer** samples the atomics and broadcasts compact tick frames over a **second WebSocket broadcast channel**
  (mirroring `LogBroadcaster`); the per-block RT writes are throttled to the publish rate by that timer, not by
  the RT thread. All of it is behind the DW3 gate. *Why:* this is the only design consistent with the RT
  no-lock/no-free contract; anything that locks the mixer or allocates on the callback is forbidden.

## Sprint 9 — Packaging

- **DW13 · Sidecar shipped as a documented config + Docker image; Electron seam verified, not built.** v1
  delivers the SPA build + a Dockerized reverse-proxy sidecar and documents pointing it at a daemon. The
  Electron repackage is **verified as a seam** (swap the `DaemonConnection` implementation) but not built in
  this program. *Why:* matches DW1/DW2 — ship the lightweight server path; keep Electron a future option.

---

## How decisions cross-reference

Sprint files cite these inline by number ("per DW3", "gated per DW12"). `SPRINT-TRACKER.md`'s Charter points
the whole program here as the tie-breaker. Behavior-changing decisions (DW3, DW11) are flagged for
`CHANGELOG.md`. Unresolvable blockers go to `NEEDS-HUMAN.md`; out-of-scope discoveries go to `docs/bugs.md`
tagged by their owning web sprint + finding (`Sprint W6 F2`, etc.).
