# Deployment

Running mqttaudio unattended: as a systemd service or a container, locked down, logging somewhere
useful, and monitored.

## Run under systemd

[`packaging/mqttaudio.service`](../packaging/mqttaudio.service) runs the release binary as a
dedicated unprivileged user, restarts it on failure, and locks it down: the filesystem is
read-only apart from a writable state directory, home directories are hidden, and only ALSA sound
devices are accessible.

```bash
cargo build --release
sudo install -Dm755 target/release/mqttaudio /usr/local/bin/mqttaudio
sudo useradd --system --no-create-home --shell /usr/sbin/nologin mqttaudio
sudo install -Dm640 -o root -g mqttaudio your-config.json /etc/mqttaudio/config.json
sudo install -Dm644 packaging/mqttaudio.service /etc/systemd/system/mqttaudio.service
sudo systemctl daemon-reload
sudo systemctl enable --now mqttaudio
sudo journalctl -u mqttaudio -f
```

Before starting it, point the disk cache at the state directory the unit makes writable:

```json
"cache": { "directory": "/var/lib/mqttaudio/cache" }
```

The default, `~/.mqttaudio/cache`, is hidden by the unit's `ProtectHome=true`. The daemon exits
at startup when it cannot create its cache directory, so without this setting the service restarts
in a loop. (Setting `"cache": {"enabled": false}` also works, at the cost of re-downloading remote
files after every restart.) For the same reason, keep sound files outside `/home`, `/root` and
`/run/user`.

The config file is installed readable only by root and the service group because it may hold the
MQTT password and the HTTP token.

What the unit does:

- **Restarts.** `Restart=on-failure`. The daemon exits non-zero when it cannot recover the output
  device after retrying with backoff, so systemd restarts it.
- **Device access.** The service user joins the `audio` group and may use ALSA character devices
  only. If an input device reports "Permission denied", check that group membership. A system
  service has no desktop session, so sound-server devices (`default`, `pipewire`, `pulse`) are
  usually unavailable: pick a hardware device such as `plughw:CARD=...` from `--list-devices`.
- **Memory backstop.** `MemoryMax=75%` caps the whole process. The daemon already bounds its
  decoded-audio cache (by default 40% of available memory, between 128 MiB and 1 GiB) and streams
  files that would not fit, so this limit only matters in a pathological case, where it restarts
  this one service instead of starving the machine. Use an absolute value such as
  `MemoryMax=1500M` if 75% does not suit the device. The unit deliberately has no `MemoryHigh=`:
  its reclaim throttling can stall the audio thread. Outside systemd, a container `--memory`
  limit or a cgroup `memory.max` gives the same protection.

Run `--list-devices` / `--list-inputs` as the service user to see exactly what the service sees:

```bash
sudo -u mqttaudio /usr/local/bin/mqttaudio --config /etc/mqttaudio/config.json --list-devices
```

To run the binary on another machine of the same architecture, install its runtime libraries
there: on Debian or Ubuntu, `libasound2`, `libssl3` and `libstdc++6`.

## Run in a container

The root [`Dockerfile`](../Dockerfile) builds a Debian-based image containing only the binary.

```bash
docker build --build-arg MQTTAUDIO_GIT_SHA="$(git rev-parse --short HEAD)" -t mqttaudio .
docker run --device /dev/snd -v /srv/mqttaudio:/config -p 8080:8080 mqttaudio
```

- The image runs `mqttaudio --config /config/mqttaudio.json`.
- Its health check calls `GET /ready` on port 8080, so enable the HTTP server on that port and
  bind an address reachable inside the container (for example `"bind_address": "0.0.0.0"` or
  `MQTTAUDIO_HTTP_BIND_ADDRESS=0.0.0.0`).
- With `MQTTAUDIO_HTTP_REQUIRE_AUTH=true` the health check also requires
  `MQTTAUDIO_HTTP_AUTH_TOKEN` in the container environment, and reports unhealthy without it even
  when the token is in the config file (`/ready` itself never needs a token).
- The image runs as root.
- The `MQTTAUDIO_GIT_SHA` build argument is compiled in and reported as `git_sha` by
  `GET /version`; it reads `unknown` when not supplied.

## Lock it down

mqttaudio runs open by default so it is easy to try on a trusted network. Each of these is opt-in:

| Risk | Setting |
|------|---------|
| Any local file the daemon can read is playable | `security.allowed_directories`: only paths inside these directories play (symlinks and `..` are resolved first). HTTP(S) URLs are not restricted; limit outbound traffic with a firewall if that matters |
| MQTT traffic and credentials in cleartext | `mqtt.tls` (with `ca_path` for a private CA); a warning is logged when credentials go to a non-loopback broker without TLS |
| Anyone who can reach the HTTP port can send commands | `http.auth_token` protects the command endpoints; `http.require_auth` extends it to status, metrics and WebSockets. Keep `http.bind_address` on loopback unless it must be remote; a warning is logged for a non-loopback bind without authentication |

The HTTP server speaks plain HTTP. Put a reverse proxy in front of it for TLS
([HTTP API: HTTPS](http-api.md#httpstls)).

To keep a token out of a checked-in config, use environment variables:

| Variable | Effect |
|----------|--------|
| `MQTTAUDIO_HTTP_AUTH_TOKEN` | Sets `http.auth_token` |
| `MQTTAUDIO_HTTP_REQUIRE_AUTH=true` | Sets `http.require_auth` and enables the HTTP server. Startup fails if no token is configured |
| `MQTTAUDIO_HTTP_BIND_ADDRESS` | Sets `http.bind_address` |

See [Configuration](configuration.md) for every option.

## Logging

Logs go to standard output, one line per event. Under systemd they land in the journal.

- `logging.level` (or `--verbose` for `debug`) sets the level; `RUST_LOG` overrides it per module
  for the console, for example `RUST_LOG=mqttaudio=info,mqttaudio::cache=debug`.
- `"logging": {"format": "json"}` writes one JSON object per line for Loki, Elasticsearch and
  similar collectors.
- `logging.mqtt_topic` (or `--log-topic`) also publishes each log line to an MQTT topic.
- `GET /ws` streams log lines to WebSocket clients, such as the web UI's log console.

## Monitoring

With the HTTP server enabled:

| Endpoint | Use |
|----------|-----|
| `GET /health` | Liveness: the process is up and serving |
| `GET /ready` | Readiness: `200` only when the output and every configured input are running |
| `GET /metrics` | Uptime, limiter clip count, audio stream errors (`xruns`), active counts, cache memory use and headroom, per-voice ducking, play-start latency |
| `GET /status/inputs` | Per-input capture health (overruns, starvation, trims) |

A rising `xruns` count means the output stream reported errors (usually underruns): try a larger
`audio.buffer_size`. A rising `clips` count means the mix is hitting the limiter: lower volumes or
`audio.master_gain`. See the [HTTP API](http-api.md) for response formats.

## Web UI

The [web control app](webui/README.md) runs as a static site behind a small reverse proxy that
serves it and forwards API calls to the daemon, keeping the auth token on the server side.
