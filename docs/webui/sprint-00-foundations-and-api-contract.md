# Sprint 0 — Foundations, API Contract & CI Harness

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | — |
| Effort | M |
| Lanes | A |
| Subagents | Optional (scaffold / API client / CI can parallelize) |

## Goal

Stand up the React + TypeScript + Material UI application skeleton in `webui/`, a green CI lane, the swappable
`DaemonConnection` transport layer (DW2), and a typed API client that faithfully encodes `API-CONTRACT.md`
including its transport quirks. After this sprint the app builds clean (`pnpm build` + `tsc --noEmit` + eslint
warnings-as-errors), connects to a daemon through the connection abstraction, reads `/health` + `/version`, and
detects open vs `require_auth`. It renders **no feature UI** — only the connect/bootstrap surface needed to
prove the seams work.

This is the load-bearing foundation every later sprint imports: the connection abstraction (DW2) that makes a
future Electron repackage a swap rather than a rewrite, the typed client where the daemon's quirks live once
instead of being re-litigated per sprint, and the TanStack Query layer (DW4) the dashboard (Sprint W2) consumes.
It must not break by being too clever — keep the client a thin, faithful mirror of the contract, and keep every
`fetch`/`WebSocket` call confined to the connection layer so the Electron seam stays real.

## Why

Everything downstream needs a buildable, tested foundation. The CI lane (`[A]`) is the gate every other sprint
runs against; without it green here, no later sprint can claim a clean lane. The API client is where the
daemon's quirks live — `loop` vs `loop_mode` (`handlers.rs:194`), the `voice_fade_out` wire key `time` vs the
typed-endpoint `time_ms` (`handlers.rs:550`), `internal_id`/`input` carried as strings (`commands.rs:80-86`,
`commands.rs` `input_volume`), the typed-`/play` field gap that silently drops `channel_map`/`mode`/`window_ms`/
`prebuffer_ms`/`freshness`/`cacheable` (`handlers.rs:181-198`). Encode them once here or every later sprint
re-discovers them the hard way, by watching a UI silently do nothing.

The connection abstraction (DW2) is the other reason this sprint exists now. The partner wants the option to
repackage as an Electron desktop app later — native header-capable WebSocket, multi-instance discovery — with
**no UI rewrite**, only a new connection implementation. That is only achievable if the seam is established
before any feature code is written and no module reaches for `fetch`/`WebSocket` directly. Modeling "a
connection" as a first-class entity (`{id,label,baseUrl,token?}`) now, while the cost is one type and a
registry, is far cheaper than retrofitting multi-instance later.

## Scope

**In scope**

- **Vite + React + TS + MUI scaffold in `webui/`** (DW4): pnpm; eslint with **warnings-as-errors** + `tsc
  --noEmit` + prettier; Vitest + React Testing Library + Playwright headless; a CI workflow that runs build +
  typecheck + lint + Vitest + Playwright.
- **A swappable `DaemonConnection` transport interface** (REST `get`/`post` JSON + WS `subscribe`) with a
  browser/proxy implementation; **"a connection" = `{id,label,baseUrl,token?}`** modeled as a first-class entity
  (multi-instance-ready, DW2); a registry that can hold many; an Electron impl signature reserved as a stub.
- **A typed API client over `DaemonConnection`** covering every runtime command (API-CONTRACT §2) and read
  endpoint (§5), encoding the transport quirks (DW10: `/command` for the full play surface; `loop`/`loop_mode`;
  `time`/`time_ms`; `internal_id` string; `input` string; lowercase enums for `mode`/`freshness`).
- **Connection bootstrap:** read `/health` + `/version`, detect open vs `require_auth`, surface an optional
  Bearer-token field (DW9, in-memory by default).
- **A TanStack Query layer** wired to the client with the DW6 poll cadences as defaults (consumed by Sprint W2).

**Out of scope** (owned by other sprints — coordinate, do not duplicate)

- **The reverse-proxy sidecar + dev proxy config** — owned by **Sprint W1**, coordinate, do not duplicate. This
  sprint's browser connection uses `fetch`/`WebSocket` against whatever base URL it is given; wiring the proxy
  and same-origin dev config is W1's box.
- **Any dashboard / feature rendering** (health header, now-playing board, voices/inputs racks, command forms,
  matrix mixer, transport, telemetry, config panels) — owned by **Sprints W2 and later**, coordinate, do not
  duplicate. This sprint renders only the connect/bootstrap surface.
- **The `/ws` log console** — owned by **Sprint W1**, coordinate, do not duplicate. The connection's `subscribe`
  method exists here as a typed seam, but rendering the log stream is W1's.
- **Any daemon (Rust) change** — owned by **Sprints W6/W7/W8**, coordinate, do not duplicate. This sprint adds a
  separate frontend app and touches no Rust.

## Work items

### F1 — App scaffold + tooling + CI lane
- **Statement:** There is no frontend app yet. Create the Vite + React + TS + MUI scaffold in `webui/` with the
  full DW4 toolchain and a CI workflow that runs the whole Lane A gate.
- **Where:** new `webui/` (`webui/package.json`, `webui/vite.config.ts`, `webui/tsconfig.json`,
  `webui/.eslintrc.cjs` (or flat `eslint.config.js`), `webui/.prettierrc`, `webui/playwright.config.ts`,
  `webui/src/`, `webui/tests/`, `webui/e2e/`); CI at `.github/workflows/webui-ci.yml`.
- **Priority:** blocker — every other box in this program runs against this lane.
- **Rationale:** The Charter's green-gate requirement (`SPRINT-TRACKER.md:38-41`) is `pnpm build` + `tsc
  --noEmit` + eslint warnings-as-errors + Vitest + Playwright. None of it exists until this scaffold does. DW4
  fixes the stack (Vite + React + TS + MUI + TanStack Query + Vitest/RTL/Playwright + pnpm), so there is no
  choice to make — only wiring to do.
- **Approach:** Scaffold with Vite's React-TS template under `webui/`. Add MUI (`@mui/material`,
  `@emotion/react`, `@emotion/styled`) and TanStack Query (`@tanstack/react-query`). Define scripts in
  `package.json`: `build` (`tsc --noEmit && vite build`), `typecheck` (`tsc --noEmit`), `lint` (`eslint . --max-
  warnings=0`), `test` (`vitest run`), `test:e2e` (`playwright test`). Configure eslint with the TS + React
  plugins and **`--max-warnings=0`** so warnings fail. Configure Vitest with the `jsdom` environment and an RTL
  setup file. Configure Playwright for headless Chromium against a fixture/mock backend (see F5/Tasks). Write
  `webui-ci.yml` to install pnpm, run `pnpm install --frozen-lockfile`, then `pnpm typecheck`, `pnpm lint`,
  `pnpm build`, `pnpm test`, and `pnpm test:e2e` (installing the Playwright browser in CI). Build must be
  warning-free.

### F2 — `DaemonConnection` transport interface + browser/proxy impl + connection model (DW2)
- **Statement:** Define the single transport seam all daemon access goes through, implement it for the browser,
  and model a connection as a first-class, multi-instance-ready entity.
- **Where:** `webui/src/api/connection.ts` (interface + `Connection` entity + registry),
  `webui/src/api/connection.browser.ts` (browser/proxy impl), `webui/src/api/connection.electron.ts` (reserved
  stub signature, not implemented).
- **Priority:** blocker — DW2 is the whole reason the program is structured this way.
- **Rationale:** DW2 (`DECISIONS.md:31-36`) requires that all daemon access goes through one swappable
  `DaemonConnection`, that "a connection" (base URL + optional token) is a first-class entity, and that **no
  React UI module imports `fetch`/`WebSocket` directly** so an Electron repackage is a connection swap with no UI
  rewrite. The browser's native `WebSocket` cannot set an `Authorization` header (DW1, `DECISIONS.md:23-29`),
  which is exactly why the transport must be abstracted: the Electron impl will authenticate differently.
- **Approach:** Define `interface DaemonConnection` with `get<T>(path): Promise<T>`, `post<T>(path, body):
  Promise<T>`, and `subscribe(path, handlers): Subscription` (the WS seam W1 renders against). Model `Connection`
  as `{ id: string; label: string; baseUrl: string; token?: string }`. Implement `ProxyBrowserConnection`
  constructed from a `Connection`: `get`/`post` use `fetch` with JSON headers (and, when a `token` is set and the
  deployment is direct rather than proxied, an `Authorization: Bearer` header — note DW9: behind the proxy the
  token is injected server-side and the SPA never puts it in a URL); `subscribe` uses `WebSocket`. Add a
  `ConnectionRegistry` that holds many connections keyed by id (multi-instance-ready) even though v1 uses one.
  Provide `connection.electron.ts` exporting only the impl **signature** (a constructor type / interface
  conformance assertion) with a `throw new Error("Electron connection not implemented in v1")` body — it must
  type-check and prove the seam, nothing more. Enforce the "no direct `fetch`/`WebSocket` outside this layer"
  rule with an eslint `no-restricted-globals`/`no-restricted-properties` rule scoped to `src/` excluding
  `src/api/connection.*.ts`, **and** a unit test that greps the built source for stray usages (belt and braces,
  per the caveat).

### F3 — Typed API client over the connection, mirroring API-CONTRACT
- **Statement:** Build a typed client over `DaemonConnection` exposing every runtime command (§2) and read
  endpoint (§5), emitting exactly the JSON the daemon expects including every transport quirk.
- **Where:** `webui/src/api/client.ts` (the command/read methods), `webui/src/api/contract.ts` (the
  request/response TypeScript types + the quirk-encoding helpers).
- **Priority:** blocker — the quirks ARE the contract; encoding them wrong silently breaks every later sprint.
- **Rationale:** API-CONTRACT §2/§5 is the code-verified surface. The quirks most likely to silently break a UI
  are spelled out: nested `{command, message}` where **nested wins wholesale** (`commands.rs:25-33`); the full
  play surface (`channel_map`/`mode`/`window_ms`/`prebuffer_ms`/`freshness`/`cacheable`) only reachable via
  `/command`, silently dropped by typed `/play` (`handlers.rs:181-198`, DW10); `loop` over MQTT/flattened vs
  `loop` **or** `loop_mode` on typed `/play` (`handlers.rs:194`); `voice_fade_out` wire key `time` vs typed
  `/voice/fade_out` body key `time_ms` (`handlers.rs:550`); `input_mute` `mute` required over MQTT but defaulting
  `false` on the typed endpoint (`handlers.rs:631`); `internal_id` sent as a string parsed to `u64`
  (`commands.rs:80-86`); `input` a string index or `voice_id`, a bare number errors (API-CONTRACT §2);
  lowercase `mode`/`freshness` enums; the empty selector that matches nothing and is a silent no-op
  (`commands.rs:114-119`, API-CONTRACT §3).
- **Approach:** In `contract.ts`, declare a TS type per command's params and per read endpoint's payload,
  mirroring the API-CONTRACT tables verbatim — including the string-typed `internal_id`/`input`, the lowercase
  literal-union enums for `mode` (`"auto" | "stream" | …` per the contract) and `freshness`
  (`"trusting" | …`), and the play surface fields. In `client.ts`, expose one method per command (`play`,
  `stop`, `stopall`, `volume`, `seek`, `speed`, `voiceVolume`, `voiceFadeOut`, `voiceStop`, `inputVolume`,
  `inputMute`, `precache`, `cacheClear`, `cacheInvalidate`, `cacheReload`) and one per read endpoint (`health`,
  `version`, `metrics`, `status`, `statusSamples`, `statusVoices`, `statusCache`, `statusInputs`). Route the
  **full play surface through `POST /command`** as raw `{command:"play", message:{…}}` (DW10), and route the
  fields the typed endpoints carry every bit of through the typed routes where it is cleaner — but encode the
  typed-endpoint field names there (`loop_mode`, `time_ms`). **Validate and WARN, do not silently clamp:** an
  empty selector logs a `console.warn` and the client surfaces it as a no-op (it must not invent a selector);
  out-of-range `volume`/`speed` is passed through (the daemon clamps downstream, `mixer.rs:369-379`) but the
  client warns so the UI can choose to surface it. Keep the client a thin faithful mirror — do **not** "clean
  up" the quirks. Unit tests assert the **exact emitted JSON** per command (the request body the connection's
  `post` receives), including the nested `{command, message}` shape, the string `internal_id`/`input`, and the
  `time`-vs-`time_ms` / `loop`-vs-`loop_mode` divergence.

### F4 — Connection bootstrap + auth detection
- **Statement:** On connect, read `/health` + `/version`, detect whether the daemon is open or `require_auth`,
  and surface an optional in-memory Bearer-token field; a component test covers the auth-required path.
- **Where:** `webui/src/api/bootstrap.ts` (the bootstrap logic) and `webui/src/components/Connect.tsx` (the
  connect component: base URL + optional token field).
- **Priority:** high — bootstrap is the only user-facing surface this sprint ships and the auth gate is
  load-bearing for every `[B]` lane later.
- **Rationale:** DW9 (`DECISIONS.md:83-89`) requires the UI read `require_auth`/open mode (via `/health`
  behavior) to decide whether to require a token, and hold the optional Bearer token **in memory** by default.
  The read endpoints are open by default and gated under `require_auth` (API-CONTRACT §5,
  `routes.rs:101-108`); a gated probe returning `401` implies a token is needed (`routes.rs:62`).
- **Approach:** `bootstrap(connection)` reads `/health` (liveness + version) and `/version` (the once-fetched
  `{name, version, git_sha?}`). To detect auth mode, probe a gated endpoint: a `200` means open; a `401` means
  `require_auth` and a token is needed. Return a `BootstrapResult` (`{ healthy, version, authRequired }`). The
  `Connect` component takes a base URL, runs `bootstrap`, and — when `authRequired` (or a probe `401` after a
  no-token attempt) — reveals an optional Bearer-token field; the token is held **in memory** on the
  `Connection` (DW9), persisting to `localStorage` only if later explicitly opted in with a visible warning
  (out of scope here). On success it shows `/health` + `/version` and nothing else (no feature UI). The
  component test (RTL) drives the auth-required path: a mock connection whose gated probe returns `401`, asserting
  the token field appears and that supplying a token re-probes and succeeds.

### F5 — Query/cache layer
- **Statement:** Wire a TanStack Query layer over the typed client with the DW6 poll cadences as defaults, ready
  for Sprint W2 to consume.
- **Where:** `webui/src/state/queries.ts` (the query hooks + `QueryClient` defaults), `webui/src/state/
  QueryProvider.tsx` (the provider).
- **Priority:** high — the dashboard (Sprint W2) is blocked on this; getting the cadences right here avoids
  re-litigating them.
- **Rationale:** DW4 fixes server state on TanStack Query; DW6 (`DECISIONS.md:54-58`) fixes the poll cadences:
  `/metrics` and `/status*` at 1–2 s, `/status/cache` at 2–5 s, `/version` once. Encoding the cadences as query
  defaults here means W2 wires UI, not polling policy.
- **Approach:** Create a `QueryClient` and per-endpoint hooks: `useVersion()` (fetched once — `staleTime:
  Infinity`, no refetch interval), `useMetrics()`/`useStatus()`/`useStatusSamples()`/`useStatusVoices()`/
  `useStatusInputs()` (`refetchInterval` ~1500 ms, in the 1–2 s band), `useStatusCache()` (`refetchInterval`
  ~3000 ms, in the 2–5 s band), and `useHealth()` (slow poll, the connection-loss probe). Each hook calls the
  corresponding `client` read method through the active connection. Expose a `QueryProvider` wrapping
  `QueryClientProvider`. These hooks render nothing this sprint — they exist as the consumable seam, and a unit
  test asserts the configured intervals match the DW6 bands (do not hard-poll a real daemon in CI; assert the
  `refetchInterval`/`staleTime` config or use fake timers + a mock client).

## Caveats (do not chase ghosts / do not break)

- **Do not build the proxy/sidecar here.** The reverse-proxy sidecar and the dev proxy config are **Sprint W1**.
  The browser connection in this sprint talks to whatever base URL it is handed; do not add Caddy/nginx/Vite
  proxy wiring.
- **Do not fake any data.** Render `/health` + `/version` only because they exist today. Do **not** render
  sample position, meters, or progress — `/status/samples` `position`/`position_ms`/`progress_percent` are
  hard-coded `0` until Sprint W6 (`handlers.rs:721-730`, API-CONTRACT §5). The client may expose those fields as
  typed `0`, but no UI in this sprint may present them as live.
- **Keep the client a thin, faithful mirror of API-CONTRACT — the quirks ARE the contract; do not "clean them
  up."** Do not normalize `internal_id`/`input` to numbers, do not merge flattened + nested params (nested wins
  wholesale, `commands.rs:25-33`), do not rename `time`→`time_ms` on the MQTT-style path, do not silently route
  the full play surface through typed `/play` (it drops fields, `handlers.rs:181-198`).
- **No module may import `fetch`/`WebSocket` outside the connection layer.** This is the DW2 Electron seam.
  Enforce it with an eslint rule **and** a test — a future contributor who reaches for `fetch` in a component
  must be stopped by the gate, not by review.
- **Do not over-build the connection model.** Multi-instance-ready means the *registry can hold many* and the
  entity is first-class — it does **not** mean building instance discovery, switching UI, or persistence. YAGNI
  (Charter): one connection is wired in v1; the registry just must not assume singletons.
- **Validate and warn; never silently clamp.** The empty selector is a no-op the UI must be able to surface
  (`commands.rs:114-119`); out-of-range numerics are clamped downstream, not at the client — the client passes
  them through but warns. Swallowing either turns into a "why did nothing happen" bug in a later sprint.
- **This sprint touches no Rust.** Do not edit `src/`. Any daemon gap you notice (e.g. the missing `GET /config`,
  the hard-coded `0` positions) is already owned by W6/W7/W8 — note it in `docs/bugs.md` tagged by its owning
  sprint, do not implement it.

## Tasks (ordered, TDD-first)

Subagents (optional, DW4): the **scaffold/CI** (F1), the **connection + client** (F2/F3), and the **bootstrap +
query layer** (F4/F5) split cleanly across three agents once the scaffold from task 1 lands; merge on the
shared `webui/src/api/` types. Each behavior task writes the failing test first, confirms it fails, writes the
minimum code to pass, then confirms green. Frontend tests = Vitest + RTL; the E2E smoke = Playwright headless
against a fixture/mock backend.

1. **Scaffold + tooling (F1) — do this first; everything else builds on it.** Create the Vite + React + TS + MUI
   app under `webui/` with pnpm, eslint (`--max-warnings=0`), prettier, `tsc --noEmit`, Vitest + RTL setup, and
   Playwright config. Add the `package.json` scripts. Confirm `pnpm install`, `pnpm typecheck`, `pnpm lint`, and
   `pnpm build` all run clean (warning-free). Commit a trivial `App.tsx` rendering nothing but a placeholder so
   the build has an entry point.

2. **`DaemonConnection` interface + connection model + registry (F2).** Write a failing unit test asserting a
   `Connection` entity round-trips `{id,label,baseUrl,token?}`, that a `ConnectionRegistry` can register and
   retrieve more than one connection by id, and that the `DaemonConnection` interface exposes `get`/`post`/
   `subscribe`. Confirm it fails (no module yet). Implement `connection.ts`. Confirm green.

3. **`ProxyBrowserConnection` (F2).** Write a failing test (with `fetch`/`WebSocket` mocked via the Vitest
   environment) asserting `get`/`post` issue the right method + JSON headers to `baseUrl + path`, attach the
   Bearer header when a token is set on a direct (non-proxied) connection, and that `subscribe` opens a
   `WebSocket` and forwards `onMessage`/`onClose`. Confirm it fails. Implement `connection.browser.ts`. Confirm
   green.

4. **Electron seam stub + the "no stray `fetch`/`WebSocket`" gate (F2).** Add `connection.electron.ts` with the
   reserved signature (throwing). Add the eslint `no-restricted-globals`/`no-restricted-properties` rule scoped
   to `src/` excluding `src/api/connection.*.ts`. Write a failing test that scans `webui/src/` (excluding the
   connection-layer files) and asserts zero `fetch(`/`new WebSocket(` occurrences; confirm it fails if you plant
   a stray usage, then passes once removed. Confirm `pnpm lint` flags a planted stray usage.

5. **Contract types (F3).** Write a failing type-level/`tsc` test (a `contract.ts` consumer asserting the
   string-typed `internal_id`/`input`, the lowercase enum unions for `mode`/`freshness`, and the play-surface
   field set). Implement `contract.ts` mirroring the API-CONTRACT §2/§5 tables. Confirm `tsc --noEmit` green.

6. **Command client + emitted-JSON assertions (F3).** For each command, write a failing test that calls the
   client method through a mock connection and asserts the **exact** body handed to `post` — the nested
   `{command, message}` shape, the full play surface routed to `/command` (DW10), `loop` vs `loop_mode`, `time`
   vs `time_ms`, string `internal_id`/`input`, lowercase enums. Include the empty-selector case asserting a
   `console.warn` and no fabricated selector. Confirm they fail, implement `client.ts` command methods, confirm
   green.

7. **Read-endpoint client methods (F3).** Write failing tests asserting `health`/`version`/`metrics`/`status`/
   `statusSamples`/`statusVoices`/`statusCache`/`statusInputs` call the right paths via `get` and parse into the
   contract types (fixtures mirroring the API-CONTRACT payloads, including the hard-coded `0` positions). Confirm
   they fail, implement, confirm green.

8. **Bootstrap + auth detection (F4).** Write a failing unit test for `bootstrap(connection)`: an open daemon
   (gated probe `200`) returns `authRequired:false` with health+version; a `require_auth` daemon (gated probe
   `401`) returns `authRequired:true`. Confirm it fails, implement `bootstrap.ts`, confirm green.

9. **`Connect` component + the auth-required path (F4).** Write a failing RTL component test: render `Connect`
   with a mock connection whose gated probe returns `401`; assert the Bearer-token field appears, that entering a
   token and resubmitting re-probes, and that on success the `/health` + `/version` values render (and no feature
   UI). Confirm it fails, implement `Connect.tsx`, confirm green.

10. **Query/cache layer (F5).** Write a failing test asserting the query hooks configure the DW6 cadences
    (`/version` once / `staleTime: Infinity`; `/metrics` + `/status*` ~1–2 s; `/status/cache` ~2–5 s) and call
    the matching client read methods (fake timers + a mock client; do not poll a real daemon). Confirm it fails,
    implement `queries.ts` + `QueryProvider.tsx`, confirm green.

11. **Playwright headless smoke (F1/F5).** Write a failing Playwright test that loads the app against a
    fixture/mock backend (a static fixture server or a route-mocked page) and asserts the connect surface renders
    `/health` + `/version` from the fixture. Confirm it fails, wire the mock backend + the app render path,
    confirm green headless.

12. **CI wiring (F1).** Add `.github/workflows/webui-ci.yml` running `pnpm install --frozen-lockfile`, `pnpm
    typecheck`, `pnpm lint`, `pnpm build`, `pnpm test`, and `pnpm test:e2e` (with the Playwright browser
    installed). Confirm the workflow is green end to end (run the same sequence locally to prove it).

## Files to create / touch

- **Create:**
  - `webui/package.json`, `webui/pnpm-lock.yaml`, `webui/vite.config.ts`, `webui/tsconfig.json`,
    `webui/tsconfig.node.json`, `webui/.eslintrc.cjs` (or `webui/eslint.config.js`), `webui/.prettierrc`,
    `webui/index.html`, `webui/playwright.config.ts`, `webui/vitest.setup.ts`
  - `webui/src/main.tsx`, `webui/src/App.tsx`
  - `webui/src/api/connection.ts`, `webui/src/api/connection.browser.ts`, `webui/src/api/connection.electron.ts`
  - `webui/src/api/client.ts`, `webui/src/api/contract.ts`
  - `webui/src/api/bootstrap.ts`
  - `webui/src/components/Connect.tsx`
  - `webui/src/state/queries.ts`, `webui/src/state/QueryProvider.tsx`
  - `webui/tests/connection.test.ts`, `webui/tests/client.test.ts`, `webui/tests/bootstrap.test.ts`,
    `webui/tests/connect.test.tsx`, `webui/tests/queries.test.ts`, `webui/tests/no-direct-transport.test.ts`
  - `webui/e2e/smoke.spec.ts`, `webui/e2e/fixtures/` (mock-backend fixtures)
  - `.github/workflows/webui-ci.yml`
- **Touch:**
  - `README.md` (link the new `webui/` app + its getting-started; full getting-started doc is Sprint W9, a stub
    link here is enough)
  - `docs/webui/SPRINT-TRACKER.md` (set Sprint 0 status to `In progress`, then tick the boxes when genuinely
    green)
  - `docs/bugs.md` (only if an out-of-scope daemon gap is noticed; tag by owning sprint)

## Verification

### Lane A (CI / headless) — the only lane this sprint
- `pnpm build` succeeds **warning-free**; `tsc --noEmit` passes; `eslint . --max-warnings=0` passes (a planted
  stray `fetch`/`WebSocket` in a non-connection module fails lint — verify once).
- F2: `connection.test.ts` is green — `Connection` round-trips `{id,label,baseUrl,token?}`, the registry holds
  more than one connection, `ProxyBrowserConnection.get`/`post`/`subscribe` issue the right calls, the Bearer
  header attaches on a direct connection. `no-direct-transport.test.ts` asserts zero `fetch(`/`new WebSocket(`
  outside `src/api/connection.*.ts`.
- F3: `client.test.ts` asserts the **exact emitted JSON** per command — the nested `{command, message}` shape,
  the full play surface via `/command`, `loop` vs `loop_mode`, `time` vs `time_ms`, string `internal_id`/`input`,
  lowercase `mode`/`freshness`, and a `console.warn` (no fabricated selector) on the empty-selector case. Read
  methods hit the right paths and parse into the contract types (including the hard-coded `0` positions, not
  presented as live).
- F4: `bootstrap.test.ts` shows an open daemon (probe `200`) → `authRequired:false` and a `require_auth` daemon
  (probe `401`) → `authRequired:true`, both with health+version. `connect.test.tsx` covers the **auth-required
  path**: the token field appears on `401`, a supplied token re-probes and succeeds, and `/health` + `/version`
  render with no feature UI.
- F5: `queries.test.ts` asserts the DW6 cadences (`/version` once; `/metrics` + `/status*` ~1–2 s; `/status/
  cache` ~2–5 s) and that each hook calls the matching client method (fake timers / mock client — no real
  daemon).
- E2E: the Playwright headless smoke loads the app against the fixture/mock backend and renders `/health` +
  `/version`, green.
- CI: `.github/workflows/webui-ci.yml` runs typecheck + lint + build + Vitest + Playwright end to end, green.

(No Lane B or Lane C this sprint — there is no live-daemon or cross-browser/a11y surface yet. No Rust lanes — no
daemon change.)

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 0)

- [ ] `webui/` scaffolds and builds: Vite + React + TS + MUI; `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) all pass in CI `[A]`
- [ ] A swappable `DaemonConnection` transport interface exists with a browser/proxy implementation; "a connection" (base URL + optional token) is a first-class entity (multi-instance-ready, per DW2) `[A]`
- [ ] A typed API client covers every runtime command + read endpoint from `API-CONTRACT.md`, encoding the transport quirks (`loop`/`loop_mode`, `time`/`time_ms`, `internal_id` as string, `/command` for the full play surface); unit tests assert the emitted JSON shapes `[A]`
- [ ] Connection bootstrap reads `/health` + `/version`, detects open vs `require_auth`, and surfaces an optional Bearer-token field; a component test covers the auth-required path `[A]`
- [ ] CI runs Vitest + React Testing Library and a Playwright headless smoke against a fixture/mock backend, green `[A]`

## Behavior-change / changelog notes

None — this sprint adds a new, separate frontend app under `webui/` and touches no daemon code. There is no
observable change to the running daemon, no new or altered daemon endpoint, and nothing to record in
`CHANGELOG.md`. The daemon-facing decisions this sprint *encodes* (DW2 connection seam, DW6 cadences, DW9 token
handling, DW10 `/command` routing) are client-side faithful mirrors of the existing contract, not changes to it.
The behavior-changing decisions (DW3 telemetry opt-in, DW11 `GET /config`) belong to Sprints W6/W7/W8 and are
out of scope here.

## Definition of Done

Lane A green (`pnpm build` warning-free · `tsc --noEmit` · eslint `--max-warnings=0` · Vitest + RTL · Playwright
headless against a fixture/mock backend) · the `DaemonConnection` seam + browser impl + first-class `Connection`
entity + multi-instance-ready registry exist and the "no direct `fetch`/`WebSocket` outside the connection
layer" rule is enforced by eslint **and** a test (DW2) · the typed client mirrors every §2 command and §5 read
endpoint with the transport quirks encoded and emitted-JSON assertions green (DW10) · bootstrap reads `/health`
+ `/version`, detects open vs `require_auth`, surfaces the in-memory Bearer field, and the auth-required
component test passes (DW9) · the TanStack Query layer carries the DW6 cadences and is ready for Sprint W2 ·
CI workflow runs the whole gate green · new tests added (no coverage reduction) · committed on the branch ·
no daemon change so no `CHANGELOG.md` entry · any out-of-scope daemon gap noticed is logged in `docs/bugs.md`
tagged by its owning sprint.
