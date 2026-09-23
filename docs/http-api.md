# HTTP API

mqttaudio can serve a small HTTP API alongside MQTT, or instead of it. It accepts every
[command](commands.md), reports what is playing, and streams logs and live state over WebSockets.
Use it for control panels, for scripting with `curl`, or to run without a broker.

## Turn it on

```json
{
  "http": {
    "enabled": true,
    "port": 8080,
    "auth_token": "a-long-random-token"
  }
}
```

or `mqttaudio --http-port 8080`. The server listens on `127.0.0.1` unless `http.bind_address` says
otherwise, and logs the address it bound (`HTTP server listening on http://127.0.0.1:8080`). Port `0`,
the default, picks a free port. All `http` settings are listed in
[Configuration](configuration.md#http).

With only the HTTP server enabled (no `mqtt.topic`), no broker is needed. If the HTTP server cannot
start and there is no MQTT connection either, the daemon exits with an error.

## Authentication

| Routes | Without `auth_token` | With `auth_token` | With `auth_token` and `require_auth` |
|--------|----------------------|-------------------|--------------------------------------|
| `/health`, `/ready` | open | open | open |
| [Status routes](#status-routes) | open | open | token required |
| [Command routes](#command-routes), `POST /telemetry` | open | token required | token required |
| `/ws`, `/ws/state` | open | token required | token required |

Send the token as `Authorization: Bearer <token>` or as a `token` query parameter
(`?token=<token>`, percent-encoded if it contains special characters). A request without a valid
token gets `401` with an empty body.

```bash
curl -H "Authorization: Bearer a-long-random-token" -X POST http://localhost:8080/stopall
curl -X POST "http://localhost:8080/stopall?token=a-long-random-token"
```

Browsers cannot set headers on a WebSocket, so use the query parameter there. Tokens shorter than 8
characters are rejected at startup, and `require_auth` without a token is a startup error.

## Command routes

All take `POST` with a JSON body (`Content-Type: application/json`).

| Route | Command | Body |
|-------|---------|------|
| `/command` | any | The full command JSON, exactly as sent over MQTT |
| `/play` | `play` | `file`, `id`, `volume`, `voice`, `fade_in`, `start_position_ms`, `loop`, `crossfade_ms`, `channel_map` |
| `/stop` | `stop` | Selector fields and `fade_out_ms` |
| `/stopall` | `stopall` | none |
| `/fadeall` | `fadeall` | `time` (send `{}` for the default) |
| `/volume` | `volume` | Selector fields and `volume` |
| `/seek` | `seek` | Selector fields and `position_ms` |
| `/speed` | `speed` | Selector fields, `speed` and `pitch_correction` |
| `/voice/stop` | `voice_stop` | `voice` |
| `/voice/fade_out` | `voice_fade_out` | `voice` and `time_ms` |
| `/voice/volume` | `voice_volume` | `voice` and `volume` |
| `/input/volume` | `input_volume` | `input` and `volume` |
| `/input/mute` | `input_mute` | `input` and `mute` |
| `/precache` | `precache` | `file` |
| `/cache/clear` | `cache_clear` | none |
| `/cache/invalidate` | `cache_invalidate` | `file` |
| `/cache/reload` | `cache_reload` | `file` |
| `/talkback/acquire` | `talkback_acquire` | `client_id`, `source_id`, `destination`, `gain`, `lease_ms` |
| `/talkback/release` | `talkback_release` | `client_id` and `lease_id` |
| `/talkback/hard-mute` | `talkback_hard_mute` | none |
| `/telemetry` | | `{"enabled": true}` or `false`; see [Telemetry](#telemetry) |

The parameters mean the same as in the [command reference](commands.md). The typed routes accept
only the fields listed: for `mode`, `window_ms`, `prebuffer_ms`, `freshness` or `cacheable` on a play,
use `/command`. Unlisted fields are ignored.

```bash
curl -X POST http://localhost:8080/play -H "Content-Type: application/json" \
  -d '{"file": "/opt/sounds/rain.wav", "voice": "ambience", "loop": true, "fade_in": 2000}'

curl -X POST http://localhost:8080/command -H "Content-Type: application/json" \
  -d '{"command": "play", "file": "https://example.com/bed.mp3", "mode": "stream"}'

curl -X POST http://localhost:8080/voice/fade_out -H "Content-Type: application/json" \
  -d '{"voice": "ambience", "time_ms": 3000}'
```

### Command results

A command route waits until the command has been carried out (for a play, until its sound is
queued to start) and reports the outcome:

```json
{"success": true, "message": "Command completed"}
{"success": false, "error": "No sample matches the selector"}
```

| Status | Meaning |
|--------|---------|
| `200` | Done |
| `400` | The command is malformed: bad JSON, unknown command, a missing or invalid parameter, no selector, an unknown channel name, `speed: 0` |
| `403` | Not allowed: a path outside `security.allowed_directories`, a refused talkback request, unmuting an input held by talkback |
| `404` | Nothing to act on: a file that is missing or cannot be decoded, a URL that fails, a selector that matches no sound, an empty voice, an input that did not open |
| `409` | A pending play or cache command was cancelled by `stopall` or `fadeall` |
| `500` | The daemon is overloaded (32 loads already in flight, or its audio queue is full) or failed internally |
| `504` | No result within 30 seconds |

A typed route whose body cannot be read as its fields answers with a plain-text error from the web
framework: `400` for broken JSON, `415` without a JSON `Content-Type`, `422` for a missing or
mistyped field. `/command` answers those cases with a `400` in the JSON shape above.

## Status routes

All take `GET`.

| Route | Returns |
|-------|---------|
| `/health` | `{"status": "ok", "service": "mqttaudio", "version": "..."}` while the process is serving |
| `/ready` | Whether audio output and every configured input are running |
| `/version` | Name, version and, in builds that set it, `git_sha` |
| `/status` | Counts, limiter and xrun counters, cache totals |
| `/status/samples` | Every playing sound |
| `/status/voices` | Every voice, with its volume and ducking level |
| `/status/inputs` | Every configured live input and its capture health |
| `/status/cache` | Memory and disk cache totals |
| `/status/talkback` | The talkback lease |
| `/status/meters` | Output peak levels (with [telemetry](#telemetry) on) |
| `/metrics` | Monitoring counters |
| `/config` | The configuration the daemon started with |
| `/telemetry` | `{"enabled": false}`: whether telemetry is on |

### /ready

`200` when ready, `503` otherwise:

```json
{
  "status": "not_ready",
  "ready": false,
  "checks": {"output": true, "inputs": false},
  "failed_inputs": [{"voice_id": "gm_mic", "error": "..."}]
}
```

Readiness is decided when the daemon starts. An input that fails later does not change it.

### /status

```json
{
  "status": "running",
  "version": "2.1.0-rc.1",
  "active_samples": 2,
  "active_inputs": 1,
  "active_voices": 2,
  "output_channels": 8,
  "clip_count": 0,
  "xruns": 0,
  "cache": {
    "memory": {"entries": 3, "size_bytes": 1048576},
    "disk": {"entries": 10, "size_bytes": 5242880}
  }
}
```

- `active_inputs` is the number of configured inputs, including any that failed to open.
- `clip_count` counts output samples that reached the limiter's ceiling since startup.
- `xruns` counts errors reported by the audio output stream since startup, including underruns the
  audio system recovered from on its own.

### /status/samples

```json
{
  "samples": [
    {
      "internal_id": "7",
      "id": "rain",
      "voice": "ambience",
      "file": "/opt/sounds/rain.wav",
      "position": 48000,
      "position_ms": 1000,
      "total_frames": 480000,
      "total_ms": 10000,
      "sample_rate": 48000,
      "volume": 0.6,
      "voice_volume": 1.0,
      "speed": 1.0,
      "loop_mode": true,
      "windowed": false,
      "progress_percent": 10.0
    }
  ]
}
```

- `internal_id` is the value to use in an `internal_id` selector.
- `position`, `position_ms` and `progress_percent` are live only while [telemetry](#telemetry) is on,
  and `0` otherwise. They are always `0` for a windowed sound.
- `total_frames` and `total_ms` are `0` for a windowed sound, and an estimate while a sound's first
  load is still decoding.
- `speed` is the value last requested, before clamping.

### /status/voices

```json
{
  "voices": [
    {"id": "music", "sample_count": 1, "volume": 1.0, "ducking_multiplier": 0.2},
    {"id": "narration", "sample_count": 1, "volume": 1.0, "ducking_multiplier": 1.0}
  ]
}
```

`ducking_multiplier` is the level a ducking rule is taking the voice to (`1.0` when not ducked).

### /status/inputs

```json
{
  "inputs": [
    {
      "index": 0,
      "voice_id": "gm_mic",
      "volume": 0.8,
      "channels": 1,
      "muted": false,
      "unmuted_volume": 0.8,
      "ready": true,
      "last_error": null,
      "backlog_frames": 512,
      "max_backlog_frames": 2304,
      "dropped_frames": 0,
      "trimmed_frames": 0,
      "underrun_frames": 0
    }
  ]
}
```

| Field | Meaning |
|-------|---------|
| `index` | Position in the config's `inputs` list, usable as an `input` selector |
| `volume`, `muted`, `unmuted_volume` | The state the audio thread has applied. `unmuted_volume` is what unmuting restores |
| `ready`, `last_error` | Whether the capture stream opened at startup, and why not |
| `backlog_frames` | Audio waiting between capture and output right now |
| `max_backlog_frames` | Backlog above which old audio is discarded to keep latency bounded |
| `dropped_frames` | Captured audio lost because the buffer was full |
| `trimmed_frames` | Audio discarded to bring latency back down |
| `underrun_frames` | Output frames that found no captured audio waiting |

The three counters only grow. [Microphone Input](features/microphone-input.md#monitoring) explains
what rising values mean.

### /status/cache

```json
{
  "memory": {"entries": 3, "size_bytes": 1572864, "size_mb": 1.5},
  "disk": {"entries": 10, "size_bytes": 5242880, "size_mb": 5.0}
}
```

### /status/talkback

```json
{
  "now_ms": 51234,
  "talkback": {
    "state": "live",
    "applied_live": true,
    "lease_id": "lease-0003",
    "owner_client_id": "panel-1",
    "source_id": "GM_MIC",
    "destination": "GUEST_ALL",
    "gain": 0.0,
    "lease_expires_at_ms": 52000,
    "last_transition": "acquired",
    "last_error": null
  }
}
```

`state` is `live` while a lease is held and `muted` otherwise. `now_ms` and `lease_expires_at_ms` are
milliseconds since the daemon started, so their difference is the time left. `last_transition` is
`acquired`, `renewed`, `released`, `expired` or `hard-muted`; `last_error` is the last refused
request's reason. The lease holder needs `lease_id` to release it.

### /metrics

```json
{
  "uptime_seconds": 3600.5,
  "clips": 0,
  "xruns": 0,
  "active_voices": 1,
  "active_samples": 2,
  "active_inputs": 1,
  "output_channels": 8,
  "cache": {
    "memory_bytes": 1572864,
    "memory_entries": 3,
    "memory_headroom_bytes": 858993459,
    "memory_cap_bytes": 1073741824,
    "disk_bytes": 5242880
  },
  "ducking": {"music": 0.2},
  "latency": {
    "play_to_first_mix_ns": {"last": 12400000, "max": 18100000},
    "plays_measured": 42
  },
  "input_capture": {
    "gm_mic": {"resample_errors": 0, "overflow_dropped_samples": 0, "ratio_rejects": 0, "scratch_regrows": 0}
  },
  "pitch_scratch_regrows": 0
}
```

| Field | Meaning |
|-------|---------|
| `clips`, `xruns` | As on `/status` |
| `cache.memory_cap_bytes` | The memory cache budget (`null` when unlimited) |
| `cache.memory_headroom_bytes` | Budget still free for new full loads (`null` when unlimited) |
| `ducking` | Voices currently ducked, with the level they are ducked to |
| `latency.play_to_first_mix_ns` | Time from a play being queued to its first audio being mixed: the most recent and the largest since startup |
| `input_capture.<voice_id>` | Capture-path error counters per input; all should stay `0` |
| `pitch_scratch_regrows` | Should stay `0`; a rising value means the audio device delivers larger blocks than expected |

### /config

The configuration as the daemon applied it at startup, after command-line and environment
overrides, with `mqtt.password` and `http.auth_token` replaced by `null`. Changes to the file do not
show here until a restart.

## Telemetry

Live playback positions and output meters cost work on the audio thread, so they are off until a
client asks for them. `POST /telemetry` with `{"enabled": true}` turns them on for everyone, and
`{"enabled": false}` turns them off again; the answer echoes the new state. The setting is not saved
across restarts.

While telemetry is on:

- `/status/samples` reports live positions;
- `/status/meters` returns each output channel's peak level since the previous audio block, as a
  linear amplitude after the limiter: `{"output": [0.42, 0.40, 0.0, ...]}` (all zeros while
  telemetry is off);
- `/ws/state` sends updates.

## WebSockets

### /ws: log stream

Streams the daemon's log. The first message is
`{"type": "connected", "message": "Connected to mqttaudio log stream", "version": "..."}`; after it,
each log line arrives as `{"type": "log", "message": "<formatted line>"}`. New clients get no earlier
lines, and a client that falls more than 1000 lines behind skips the lines it missed. Turn it off
with `http.websocket_enabled: false`.

```javascript
const ws = new WebSocket("ws://localhost:8080/ws?token=a-long-random-token");
ws.onmessage = (event) => console.log(JSON.parse(event.data).message);
```

### /ws/state: live positions and meters

While telemetry is on, sends about 15 messages per second:

```json
{
  "type": "tick",
  "samples": [{"internal_id": "7", "position_ms": 1000, "progress_percent": 10.0}],
  "meters": {"output": [0.42, 0.40, 0.0, 0.0]}
}
```

Nothing is sent while telemetry is off.

## Cross-origin requests

`http.cors_permissive: true` lets web pages from any origin call the API. Browsers do not treat the
`Authorization` header as covered by that permission, so a cross-origin page should pass the token as
the `token` query parameter. A page served from the same origin (for example through the
[web UI's proxy](webui/README.md)) needs neither.

## HTTPS

The server speaks plain HTTP. To reach it over a network, bind it to loopback and put a reverse
proxy in front of it for TLS, forwarding WebSocket upgrades:

```nginx
server {
    listen 443 ssl;
    server_name audio.example.com;

    ssl_certificate     /etc/ssl/audio.example.com.pem;
    ssl_certificate_key /etc/ssl/audio.example.com.key;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```
