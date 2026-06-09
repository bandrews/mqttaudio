# Deploying the mqttaudio web app (reverse-proxy sidecar)

The web app is a static SPA fronted by a small **reverse-proxy sidecar** (DW1). The sidecar serves the built
bundle and proxies the daemon's surface, so the browser is **same-origin** (no CORS) and the auth token is
injected **server-side** — the browser never carries it (DW9). The daemon itself needs no changes and no
static-serving.

```
browser ──http(s)──▶ sidecar (Caddy/nginx) ──▶ mqttaudio daemon
                       ├─ /            → webui/dist (the SPA)
                       ├─ /api/*       → daemon (prefix stripped) + Bearer injected
                       └─ /ws          → daemon /ws (WebSocket) + Bearer injected
```

The SPA always talks to **relative** URLs (`/api`, `/ws`); the same-origin contract holds in both dev and prod.

## Build the SPA

```bash
cd webui
pnpm install
pnpm build        # outputs webui/dist
```

## Run the sidecar

### Caddy (recommended)

```bash
SITE_ADDRESS=:8088 \
DAEMON_HOST=127.0.0.1:8080 \
DAEMON_TOKEN=your-daemon-token \
WEBUI_DIST="$(pwd)/dist" \
caddy run --config deploy/Caddyfile
```

Open `http://localhost:8088`. In the connect screen, leave the URL as `/api` (same-origin).
For automatic HTTPS/`wss://`, set `SITE_ADDRESS=audio.example.com` (a real hostname Caddy can get a cert for).

### nginx

Edit `deploy/nginx.conf` — set the `upstream` to your daemon, replace `<DAEMON_TOKEN>` (or remove the
`Authorization` lines if the daemon is open), and point `root` at `webui/dist`. Then include it in your nginx
config and reload. For TLS, wrap the `server` block in `listen 443 ssl;` with your certificates.

## Dev (no sidecar needed)

`pnpm dev` runs Vite with a proxy that mirrors the sidecar: `/api` → daemon (prefix stripped), `/ws` upgraded.
Point it at a daemon with `VITE_DAEMON_TARGET`:

```bash
VITE_DAEMON_TARGET=http://127.0.0.1:8080 pnpm dev
```

The dev token (if the daemon requires auth) is handled by the daemon/proxy target; the SPA still uses `/api`.

## Token handling (DW9)

- **Behind the sidecar:** the token lives only in the sidecar's environment (`DAEMON_TOKEN`) and is injected as
  `Authorization: Bearer …` on the proxied requests. It is **never** in a browser URL. This is the secure
  default.
- **Direct mode (advanced):** if you point the SPA straight at a daemon URL (`http://host:8080`) without the
  proxy, the browser attaches the token as a header for REST, but a browser WebSocket cannot send a header — so
  `/ws` would carry the token as `?token=`, which leaks into URLs/logs. Use the sidecar to avoid this.

## Electron seam (DW2 — future, not built here)

All daemon access goes through one swappable transport, `DaemonConnection` (`webui/src/api/connection.ts`). No
UI module imports `fetch`/`WebSocket` directly — that is enforced by an ESLint rule (`no-restricted-globals`
scoped to `src/` outside `src/api/connection.*.ts`) **and** a guard test (`tests/no-direct-transport.test.ts`).
Because of that, an **Electron** repackage swaps only the connection implementation
(`webui/src/api/connection.electron.ts`, currently a reserved stub): a native, header-capable WebSocket and
direct daemon URLs, with no UI rewrite and no need for the proxy's token injection. This is a designed seam, not
implemented in v1 (DW13).
