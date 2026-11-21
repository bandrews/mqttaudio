# Sprint 1 — Deployment: Reverse-Proxy Sidecar & Connectivity

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A, B |
| Subagents | Optional (proxy config / log console / lifecycle can parallelize) |

## Goal

Make the web app reachable the secure default way (DW1): stand up a lightweight reverse-proxy sidecar that
serves the built SPA and proxies `/api/*` and `/ws` to a running `mqttaudio` daemon, injecting the
`Authorization: Bearer` header **server-side** and terminating TLS (`wss://`) so the browser is same-origin and
never carries the token. Mirror that topology in the Vite dev server so development behaves identically without
the production sidecar. On top of the proxy, render the daemon's one live feed that exists today — the `/ws` log
stream — in a live log console, and give the connection a real lifecycle: connect/disconnect, backoff/retry,
auth-failure and offline surfacing, and a `/health` re-probe on reconnect. Document the Electron seam so the
desktop repackage later swaps only the `DaemonConnection` implementation (DW2).

"Done and correct" is: the SPA loads through the sidecar against a live daemon, the log console streams real
tracing events, and a daemon restart is survived by backoff/reconnect without a page reload. It builds on
Sprint 0's `DaemonConnection` transport interface, the typed API client, and the CI harness — and it must not
break them: every byte of daemon access still flows through `DaemonConnection`, no React component reaches for
`fetch`/`WebSocket` directly, and the secure-by-default token handling (DW9) stays intact. This sprint adds **no
daemon code** (DW1 explicitly rejected daemon static-serving).

## Why

DW1 and DW9 make the proxy load-bearing, not a convenience. The browser's native `WebSocket` API cannot set an
`Authorization` header, so a direct-to-daemon app on a `require_auth` daemon has exactly one way to authenticate
the socket: putting the token in the URL as `?token=` — which the daemon does accept (`routes.rs:62`), but which
leaks the secret into URLs, proxy logs, and browser history (DW9). The sidecar is what removes that leak: it is
the single server-side place the `Bearer` header is injected, it keeps the SPA same-origin so CORS stays off
(`cors_permissive` defaults false), and it keeps the daemon single-purpose.

This sprint also stands up the **only live feed the daemon has today**. Everything else in the program polls
(`/status*`, `/metrics`) until telemetry lands in Sprints 6–7; the `/ws` log stream is the lone push channel
that exists now, and nothing renders it yet. Getting the connection lifecycle, backoff, and the log console
right here means later sprints inherit a proven transport rather than reinventing reconnection. The gap closed
is "the app cannot actually be deployed or connected" — without the sidecar there is no secure delivery path,
and without the lifecycle work there is no resilient connection to build the dashboard on.

## Scope

**In scope**

- **A reverse-proxy sidecar config** (Caddy recommended; nginx alternative) that serves the SPA `dist`, proxies
  `/api/*` (prefix stripped) to the daemon HTTP port, proxies `/ws` with the WebSocket `Upgrade`/`Connection`
  headers, injects `Authorization: Bearer <token>` server-side (DW9), and can terminate TLS for `wss://`.
- **A Vite dev proxy** mirroring the sidecar so `/api` and `/ws` are same-origin in development without the
  production server.
- **A live log console** consuming `/ws`: render the `{type:"connected"}` welcome frame, then `{type:"log",
  message}` lines, handle the broadcast `Lagged`/`Closed` outcomes, and reconnect with backoff.
- **Connection lifecycle + resilience:** connect/disconnect, exponential backoff/retry, surfacing `401`
  (auth) and offline states, and a `/health` re-probe on reconnect.
- **The Electron seam,** documented and guarded: confirm no module touches `fetch`/`WebSocket` outside the
  connection layer, and document that an Electron build swaps only that implementation (DW2).

**Out of scope** (named owner — coordinate, do not duplicate)

- **Dashboard widgets** (health header, now-playing board, gauges) — owned by **Sprint W2**, coordinate, do not
  duplicate. This sprint renders only the log console.
- **Command-sending forms** (play/stop/volume/etc.) — owned by **Sprint W3**, coordinate, do not duplicate.
- **A state-event WebSocket** carrying playback/position/meter events — owned by **Sprint W7**; the `/ws` here is
  **logs-only** and must not be presented as a state feed.
- **Daemon static-serving** (the rejected Option 2; DW1 chose the sidecar) — never added; do not put serving code
  in the Rust daemon.
- **Production Docker image of the sidecar + Electron repackage verification** — owned by **Sprint W9** (DW13);
  this sprint ships the runnable config, W9 ships the image.

## Work items

### F1 — Reverse-proxy sidecar config (DW1)

- **Statement:** Provide a runnable, documented reverse-proxy sidecar that serves the built SPA and proxies the
  daemon surface, injecting the `Bearer` token server-side so the browser never carries it (DW9), with optional
  TLS for `wss://`.
- **Where:** `webui/deploy/Caddyfile` (recommended) plus `webui/deploy/nginx.conf` (alternative);
  `webui/deploy/README.md` for the run/point-at-a-daemon instructions. Proxies to the daemon HTTP server
  (`routes.rs:76` `create_router`); the `?token=`-leak it eliminates is `routes.rs:62`.
- **Priority:** P0 — it is the program's delivery mechanism; nothing in Lane B works without it.
- **Rationale:** The browser `WebSocket` API cannot send an `Authorization` header, so a direct-to-daemon SPA
  leaks the token in `?token=` on a `require_auth` daemon (`routes.rs:62`). The sidecar is the only place that
  injects the header (DW9) and the only thing that keeps the SPA same-origin so CORS stays off (`cors_permissive`
  default false). The `/ws` upgrade block is essentially the nginx `Upgrade`/`Connection` snippet from
  `docs/http-api.md`.
- **Approach:** In the Caddyfile, serve `webui/dist` with SPA fallback (`try_files`/`file_server` to
  `index.html`); `handle_path /api/*` reverse-proxies to the daemon HTTP host:port with the prefix stripped;
  `handle /ws` reverse-proxies with the upgrade headers preserved. Inject `header_up Authorization "Bearer
  {$DAEMON_TOKEN}"` on both proxied routes so the token is read from an environment variable, never from the
  browser. Gate TLS behind Caddy's automatic-HTTPS or an explicit `tls` directive for `wss://`. Provide the nginx
  equivalent: a `location /api/` `proxy_pass` with `rewrite` to strip the prefix and `proxy_set_header
  Authorization "Bearer ..."`, and a `location /ws` with `proxy_http_version 1.1` + `proxy_set_header Upgrade
  $http_upgrade` + `proxy_set_header Connection "upgrade"`. Document that the token lives in the sidecar's
  environment, not in the SPA, and that the daemon's own `auth_token` is what the injected header must match.

### F2 — Vite dev proxy mirroring the sidecar

- **Statement:** Mirror the sidecar in the Vite dev server so `/api` and `/ws` are same-origin in development,
  giving dev the identical relative-URL behavior the production proxy provides.
- **Where:** `webui/vite.config.ts` (`server.proxy`).
- **Priority:** P0 — without it, dev code would have to special-case absolute URLs or CORS, diverging from
  production and from DW1's same-origin contract.
- **Rationale:** DW1 makes same-origin the default. If dev talked to an absolute daemon URL, the SPA would need
  branchy URL construction and would not exercise the relative `/api` + `/ws` paths the sidecar serves. A dev
  proxy keeps the one code path honest.
- **Approach:** Configure `server.proxy` to forward `/api` to the daemon HTTP base (with `rewrite` to strip the
  `/api` prefix, matching F1) and `/ws` with `ws: true` so the dev server upgrades the socket. Read the daemon
  target and any dev token from an env file (e.g. `VITE_DAEMON_TARGET`), documented alongside F1. The SPA's
  `DaemonConnection` browser implementation uses relative URLs in both dev and production; only the proxy target
  differs.

### F3 — Live log console over `/ws`

- **Statement:** Render the daemon's `/ws` log stream in a live console: subscribe through `DaemonConnection`,
  show the `{type:"connected"}` welcome frame and each `{type:"log",message}` line, and handle the
  `Lagged`/`Closed` broadcast outcomes with a backoff reconnect.
- **Where:** `webui/src/features/logs/LogConsole.tsx` (component) and `webui/src/features/logs/useLogStream.ts`
  (the hook that drives subscription state) — both built only on `DaemonConnection`, never on raw `WebSocket`.
  Daemon frame shapes: welcome `{type,message,version}` at `websocket.rs:62-66`, log frame
  `{type:"log",message}` at `websocket.rs:81-84`, `Lagged`/`Closed` handling at `websocket.rs:94-101`.
- **Priority:** P0 — the log stream is the only live feed the daemon exposes today and the only thing that
  proves the WebSocket transport end-to-end before telemetry exists.
- **Rationale:** The `/ws` handler emits exactly one welcome frame then one log frame per tracing event, and
  drops messages under load via `broadcast` `Lagged` rather than buffering unboundedly (`websocket.rs:94-101`).
  The console must reflect both: show streamed lines and make a `Lagged` gap visible (e.g. a "messages dropped"
  marker) rather than silently swallowing it, and treat `Closed` as a disconnect that triggers reconnect.
- **Approach:** `useLogStream` subscribes via `DaemonConnection`'s socket abstraction, maintaining a bounded
  ring of recent lines (cap the rendered buffer so a long-lived console does not grow without bound). On the
  welcome frame, mark the stream connected and record the reported daemon version. On each log frame, append the
  message. On a lag signal surfaced by the transport, insert a visible "stream lagged — messages dropped"
  marker. On close/error, drop to a disconnected state and let F4's backoff drive reconnection. `LogConsole.tsx`
  renders the lines with autoscroll and a connected/disconnected indicator. The component test drives a **mock
  socket** (Lane A): assert the welcome frame flips state to connected, log frames append in order, a lag signal
  renders the dropped-messages marker, and a close transitions to disconnected and schedules a reconnect. Do not
  present these lines as playback/state events — they are logs (Sprint W7 owns state events).

### F4 — Connection lifecycle + resilience

- **Statement:** Give the browser `DaemonConnection` a real lifecycle: retry with exponential backoff, surface
  `401` (auth-required/failed) distinctly from offline/unreachable, and re-probe `/health` on reconnect before
  declaring the connection live.
- **Where:** `webui/src/api/connection.browser.ts` (the browser/proxy `DaemonConnection` implementation from
  Sprint 0). Health/auth semantics: `/health` is liveness-only (API-CONTRACT §5); `401` comes from the auth
  middleware (`routes.rs:36-72`) when the proxy's injected token is wrong or `require_auth` is set with no valid
  token.
- **Priority:** P0 — the dashboard and every later sprint poll through this connection; a flaky or
  silently-dead connection corrupts all of them.
- **Rationale:** Behind the sidecar the SPA hits relative URLs, so a daemon restart manifests as socket close
  + failing fetches, not a CORS or URL error. The connection must distinguish three states the UI presents
  differently: **offline/unreachable** (network/daemon down → retry with backoff), **auth failure** (`401` →
  stop retrying blindly, surface that the proxy token is wrong or auth is required), and **live** (`/health`
  re-probe succeeds). Treating a `401` as a transient offline blip would spin forever; treating offline as auth
  would mislead the operator.
- **Approach:** Implement an exponential backoff with jitter and a sane ceiling for reconnect attempts (both the
  WebSocket and the poll/health channel), resetting the backoff on a successful `/health` probe. Map fetch/socket
  outcomes to an explicit connection-state enum (`connecting` / `live` / `offline` / `unauthorized`). On
  reconnect, re-probe `/health` first and only mark `live` once it returns 200; a `401` short-circuits to
  `unauthorized` and surfaces the auth-failure state to the UI rather than looping silently. Expose this state so
  the log console (F3) and future dashboard can render it. Unit-test the state machine and backoff schedule with
  fake timers (Lane A): assert backoff grows and caps, a `401` lands in `unauthorized` without further blind
  retries, and a recovered `/health` resets to `live` with backoff cleared.

### F5 — Electron seam guard + doc

- **Statement:** Enforce and document the DW2 seam: no UI module imports `fetch`/`WebSocket` directly — all
  daemon access goes through `DaemonConnection` — and document that an Electron repackage swaps **only** that
  implementation.
- **Where:** an ESLint rule (e.g. `no-restricted-globals`/`no-restricted-syntax` scoped to disallow `fetch` and
  `WebSocket` outside `webui/src/api/`) plus a guard test, and a seam note in `webui/deploy/README.md` (and/or
  `docs/webui/` connection docs).
- **Priority:** P1 — it does not block the live path, but it is the mechanism that keeps DW2 true over the life
  of the program; without it, later sprints will drift and the Electron seam silently rots.
- **Rationale:** DW2 promises an Electron repackage with **no UI rewrite** — only a new connection
  implementation. That only holds if no component ever reaches around `DaemonConnection`. A lint rule makes the
  violation a build failure (eslint warnings-as-errors per DW7) rather than a code-review hope.
- **Approach:** Add an ESLint restriction that flags `fetch(`/`new WebSocket(` usage in any file under
  `webui/src/` except the connection layer (`webui/src/api/`). Add a Lane A test asserting the rule fires on a
  fixture that imports `fetch` outside the connection layer and passes for compliant code (or rely on the eslint
  run itself failing the gate). Document the seam: the browser/proxy implementation talks relative `/api` + `/ws`
  through the sidecar; an Electron implementation would use a native header-capable WebSocket and direct daemon
  URLs, and only `connection.*.ts` changes (DW2/DW13). State clearly that this is a future seam, not built here.

## Caveats (do not chase ghosts / do not break)

- **Do not add static-serving to the daemon.** DW1 rejected Option 2; the sidecar serves the SPA. No new Rust
  serving code, no new daemon routes. Touch the daemon only to read its existing surface.
- **The proxy is the ONLY place the token is injected (DW9).** The SPA must never put the token in a URL behind
  the proxy. Do not add a `?token=` query param to any relative request the SPA makes; that path exists
  (`routes.rs:62`) only for the bypass-the-proxy fallback, and using it from the SPA defeats the entire point of
  DW1.
- **`/ws` is logs-only today.** Do not present it as a state/playback feed, do not parse log message text into
  structured events, and do not let the log console masquerade as a now-playing source. Structured state events
  are Sprint W7 (DW12); faking them here would mislead and would be torn out.
- **Do not fake unavailable data.** Sample position is hard-coded `0` until Sprint W6 (`handlers.rs:721-730`);
  nothing in this sprint should imply otherwise. The log console shows logs, the lifecycle shows connection
  state — nothing more.
- **Handle `Lagged` honestly.** The daemon drops log messages under load via `broadcast` `Lagged`
  (`websocket.rs:94-101`); the console must surface the gap (a dropped-messages marker), not silently pretend
  the stream is complete.
- **Distinguish `401` from offline.** An auth failure (`routes.rs:36-72`) is not a transient network blip;
  retrying it with backoff forever is wrong. Surface it as an auth state.
- **Same-origin in dev and prod must stay one code path.** Do not special-case absolute daemon URLs in the SPA;
  the dev proxy (F2) exists precisely so the SPA always uses relative URLs.

## Tasks (ordered, TDD-first)

Subagents: the proxy config (F1/F2), the log console (F3), and the connection lifecycle + seam guard (F4/F5) are
three independent tracks that can parallelize; F3 and F4 both depend on Sprint 0's `DaemonConnection`, so settle
its socket/health surface first if it is ambiguous. Each behavior task writes the failing test first, confirms
it fails, writes the minimum code to pass, and confirms green. Frontend tests are Vitest + React Testing Library
(component) and Playwright (E2E, headless, mock backend); there is no backend change, so no Rust lane runs.

1. **Connection state machine + backoff (F4) — do this first; the console depends on it.** Write failing Vitest
   unit tests against `connection.browser.ts` with fake timers: a transient failure schedules a reconnect whose
   delay grows exponentially and caps; a `/health` 200 resets state to `live` and clears backoff; a `401` lands
   in `unauthorized` and stops blind retrying. Confirm they fail, then implement the state enum (`connecting` /
   `live` / `offline` / `unauthorized`), the backoff-with-jitter schedule, and the reconnect `/health` re-probe.
   Confirm green.

2. **Log console rendering over a mock socket (F3).** Write failing component tests (Vitest + RTL) that mount
   `LogConsole` wired to a **mock** `DaemonConnection` socket: emitting the `{type:"connected"}` welcome flips
   the indicator to connected and records the version; `{type:"log",message}` frames append in order; a lag
   signal renders a "messages dropped" marker; a close transitions to disconnected and schedules a reconnect via
   F4. Confirm failure, then implement `useLogStream` (bounded line buffer) and `LogConsole.tsx` (autoscroll +
   connection indicator). Confirm green. Do not label any line as a state/playback event.

3. **Electron-seam lint guard (F5).** Add the ESLint restriction disallowing `fetch`/`new WebSocket` outside
   `webui/src/api/`. Write a failing guard test/fixture: a file under `webui/src/` (outside the api layer) that
   calls `fetch` must trip the rule; compliant code must pass. Confirm the rule fires, then confirm the existing
   tree is clean. The eslint run is warnings-as-errors, so a violation fails Lane A.

4. **Sidecar config + dev proxy (F1/F2) — config-and-doc, validated by Lane B.** Author `webui/deploy/Caddyfile`
   (SPA serve + `/api/*` prefix-strip proxy + `/ws` upgrade proxy + server-side `Bearer` injection + optional
   TLS) and the `nginx.conf` alternative; mirror it in `webui/vite.config.ts` `server.proxy` (`/api` rewrite +
   `/ws` `ws:true`). Write `webui/deploy/README.md` covering: point at a daemon, set the token env var, run the
   sidecar, optional TLS. Smoke the dev proxy in CI where feasible (Playwright headless against the mock backend
   confirms relative `/api` + `/ws` resolve through the dev server); full proxy correctness with `Bearer`
   injection and a real daemon is Lane B.

5. **Playwright headless E2E against the mock backend (Lane A).** Add a headless Playwright spec that loads the
   SPA, asserts the log console connects to the mock `/ws`, renders welcome + log frames, and survives a
   simulated socket close by reconnecting (backoff). Confirm green in CI.

6. **Lane B live-daemon session.** With the sidecar in front of a running `mqttaudio` daemon, load the SPA in a
   real browser: confirm it connects, the log console streams real tracing events, and a daemon restart is
   survived by backoff/reconnect (the console drops to disconnected, then re-streams) without a page reload.
   Confirm the token never appears in any URL the browser issues (verify via devtools network) — it is only in
   the sidecar's injected header.

## Files to create / touch

- **Create:** `webui/deploy/Caddyfile`, `webui/deploy/nginx.conf`, `webui/deploy/README.md`,
  `webui/src/features/logs/LogConsole.tsx`, `webui/src/features/logs/useLogStream.ts`,
  `webui/src/features/logs/LogConsole.test.tsx`, `webui/src/api/connection.browser.test.ts`,
  `webui/tests/e2e/log-console.spec.ts` (Playwright), an ESLint config rule + its guard fixture/test under
  `webui/`.
- **Touch:** `webui/vite.config.ts` (dev `server.proxy` for `/api` + `/ws`),
  `webui/src/api/connection.browser.ts` (lifecycle/backoff/health-reprobe/auth-state), the `webui/` ESLint
  config (seam rule), `webui/package.json`/scripts if a new test or lint target is needed,
  `docs/webui/` connection/deploy notes for the Electron seam (or fold into `webui/deploy/README.md`).

## Verification

### Lane A (CI / headless)

- `pnpm build`, `tsc --noEmit`, and eslint **warnings-as-errors** pass — including the new seam rule, which fails
  the build if any module outside `webui/src/api/` references `fetch`/`WebSocket` (F5).
- Vitest + RTL: the connection state-machine tests (F4) prove exponential-backoff growth + cap, a `/health` 200
  resetting to `live`, and a `401` landing in `unauthorized` without blind retry; the `LogConsole` tests (F3)
  prove welcome→connected, in-order log append, a `Lagged` dropped-messages marker, and close→disconnected with a
  scheduled reconnect.
- The seam guard test (F5) proves the lint rule trips on a non-api-layer `fetch` and passes on compliant code.
- Playwright headless against the mock backend: the SPA loads, the log console connects to the mock `/ws`,
  renders welcome + log frames, and reconnects after a simulated socket close.
- The dev proxy resolves relative `/api` and `/ws` through Vite's `server.proxy` (smoke-level assertion in the
  headless run).

### Lane B (real browser + live daemon)

- The sidecar (Caddy or nginx) serves the built SPA and proxies `/api/*` (prefix stripped) + `/ws` to a running
  daemon; the SPA loads same-origin and connects.
- The log console streams **real** tracing events from the live daemon (welcome frame shows the daemon version;
  log lines flow).
- A daemon restart is survived: the console drops to disconnected, backoff drives reconnect, and the stream
  resumes — no page reload.
- The `Bearer` token is injected only by the sidecar: no browser-issued URL contains `?token=` (verified in
  devtools network), confirming DW9.
- TLS path (where configured): the SPA connects over `https`/`wss` through the sidecar.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 1)

- [ ] A reverse-proxy sidecar config (Caddy or nginx) serves the built SPA and proxies `/api/*` + `/ws` to the daemon, injecting the Bearer header server-side; documented and runnable (DW1) `[A]`
- [ ] A dev proxy (Vite) mirrors the sidecar so same-origin behavior holds in dev `[A]`
- [ ] The `/ws` log stream renders in a live log console (connect, `{type:"connected"}`, `{type:"log"}`, lag/close handling); component test with a mock socket `[A]`
- [ ] No UI module imports `fetch`/`WebSocket` directly — all access goes through `DaemonConnection`; the Electron-future seam is documented (DW2) `[A]`
- [ ] Against a real daemon behind the sidecar, the SPA connects, streams logs, and survives a daemon restart with backoff/reconnect `[B]`

## Behavior-change / changelog notes

None — this sprint changes no daemon behavior and adds no daemon code. The sidecar is new deployment tooling
(config + docs) and the log console / connection lifecycle are pure frontend. DW1 rejected daemon static-serving,
so the Rust binary is untouched; there is nothing to record in `CHANGELOG.md` for the daemon. If Lane B work
surfaces a real daemon-side `/ws` or auth quirk, log it in `docs/bugs.md` tagged `Sprint W1` rather than patching
the daemon under this sprint.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint **warnings-as-errors** including the F5 seam rule · Vitest +
RTL for the connection lifecycle and log console · Playwright headless against the mock backend) · Lane B green
(SPA served through the real sidecar against a live daemon: connects, streams real logs, survives a daemon
restart via backoff/reconnect, token never in a URL) · the Caddyfile + nginx alternative + `webui/deploy/README.md`
are runnable and documented (DW1) · the Vite dev proxy mirrors the sidecar (DW1 same-origin) · the Electron seam
is guarded by lint and documented (DW2) · new tests added with no coverage reduction · no daemon code changed and
no `CHANGELOG.md` entry needed · out-of-scope discoveries logged in `docs/bugs.md` tagged `Sprint W1` · committed
atomically on the sprint branch with a clear message.
