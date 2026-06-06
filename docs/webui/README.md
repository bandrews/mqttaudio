# mqttaudio Web Control & Monitoring

A React + Material UI app for monitoring a running mqttaudio daemon and exercising its full feature set —
multi-voice live mixing, channel-map matrix routing, ducking/gain tuning, cache state, and (opt-in) live sample
progress and output meters — without hand-crafting MQTT/HTTP.

The app code lives in [`webui/`](../../webui/); this directory holds its [sprint program](SPRINT-TRACKER.md),
the locked [decisions](DECISIONS.md), and the code-verified [API contract](API-CONTRACT.md).

## What you get

A single page with tabs:

- **Monitor** — health header (active counts, clip / stream-error badges, uptime, cache-budget gauge), the
  now-playing board, voices/inputs racks, the cache table, **output meters** (when telemetry is on), and a live
  log console.
- **Mixer** — per-sample transport (seek scrubber + a live playhead when telemetry is on, speed with a
  pitch-correction toggle and reverse, windowed/streamed gating), plus voice and live-input control strips
  (volume / fade / mute — these apply **immediately**).
- **Console** — a form per command with client-side validation and the OR-logic selector (with an
  empty-selector warning), a cue launcher, and a raw `/command` editor for the full play surface
  (`channel_map`/`mode`/`freshness`/…) that the typed `/play` endpoint drops.
- **Matrix** — a src×dest channel-routing grid with per-route gain, clip-risk badges, and fan-out.
- **Config** — the running config (read-only, secrets redacted), a live ducking visualization, and editors that
  emit **restart-required** config snippets (the daemon reads config once at startup; only per-input
  volume/mute change live).

A **Telemetry** switch (top bar) opts in to live sample positions + output meters — it is **off by default**
because it adds a little real-time work; the daemon does nothing extra until you enable it.

## Run it

The app is a static SPA fronted by a lightweight **reverse-proxy sidecar** (Caddy or nginx) that serves it and
proxies `/api` + `/ws` to the daemon, so the browser is same-origin and the auth token stays server-side. See
[`webui/deploy/README.md`](../../webui/deploy/README.md) for the full topology and the token model.

### Docker (sidecar + SPA in one image)

```bash
docker build -t mqttaudio-webui webui
docker run -p 8088:8088 \
  -e DAEMON_HOST=host.docker.internal:8080 \
  -e DAEMON_TOKEN=your-daemon-token \   # omit if the daemon is open
  mqttaudio-webui
# open http://localhost:8088 — on the connect screen leave the URL at /api
```

### Local dev (no sidecar)

```bash
cd webui
pnpm install
VITE_DAEMON_TARGET=http://127.0.0.1:8080 pnpm dev   # Vite proxies /api + /ws to the daemon
```

Start a daemon with its HTTP server enabled, e.g. `./mqttaudio --http-port 8080` (HTTP-only is fine).

## Connecting

On the connect screen, the **Daemon URL** defaults to `/api` (same-origin, behind the sidecar/dev proxy). If
the daemon requires auth, a Bearer-token field appears after the first attempt; behind the sidecar the token is
injected server-side and never appears in a URL.

## Develop / test

```bash
cd webui
pnpm typecheck          # tsc --noEmit
pnpm lint               # eslint, warnings are errors
pnpm test               # Vitest + React Testing Library
pnpm test:e2e           # Playwright headless (mocked backend) + axe a11y — Chromium (Lane A)
pnpm test:e2e:crossbrowser  # the same specs + axe across Chromium, WebKit (Safari) & Firefox (Gecko)
pnpm test:e2e:laneb     # Lane B: spawns a real daemon, drives the SPA through the dev proxy
pnpm build              # type-check + production build to webui/dist
```

`test:e2e:crossbrowser` needs the extra engines once: `pnpm exec playwright install webkit firefox`.

CI (`.github/workflows/webui-ci.yml`) runs the Lane A gate (build, typecheck, lint, unit/component, Playwright
headless incl. a11y) on Chromium. Lane B (real browser + live daemon) runs locally on a machine with an audio
device. Cross-engine rendering/interaction/a11y parity (WebKit + Gecko) is covered by `test:e2e:crossbrowser`;
the residual human pass — a real screen-reader walk-through and an ears-on "audio unaffected" listen — is
tracked in [`MANUAL-VERIFICATION.md`](MANUAL-VERIFICATION.md).

## Electron (future)

All daemon access goes through one swappable `DaemonConnection` (no UI module touches `fetch`/`WebSocket`
directly — enforced by lint + a test). An Electron repackage swaps only `webui/src/api/connection.electron.ts`
(a reserved stub today), gaining a native header-capable WebSocket and multi-instance discovery with no UI
rewrite.
