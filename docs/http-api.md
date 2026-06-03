# HTTP REST API

mqttaudio includes an optional HTTP server that provides REST endpoints mirroring all MQTT commands. This is useful for:

- Integration testing without an MQTT broker
- Web-based admin interfaces
- Simple HTTP-based automation

## Enable HTTP Server

```bash
# Via command line
./mqttaudio --server localhost --topic audio/commands --http-port 8080

# Or via config file
./mqttaudio --config config.json
```

Config file example:

```json
{
  "mqtt": { "topic": "audio/commands" },
  "http": {
    "enabled": true,
    "port": 8080,
    "bind_address": "127.0.0.1",
    "auth_token": "your-secret-token",
    "websocket_enabled": true,
    "cors_permissive": false
  }
}
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | bool | false | Enable HTTP server |
| `port` | u16 | 8080 | Port number (0 = auto-select) |
| `bind_address` | string | "127.0.0.1" | Network interface to bind |
| `auth_token` | string | null | Optional Bearer token for authentication |
| `websocket_enabled` | bool | true | Enable WebSocket endpoint |
| `cors_permissive` | bool | false | Allow CORS from any origin |

## REST Endpoints

### Status Endpoints (No Authentication Required)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check (returns service status) |
| `/version` | GET | Build identity (`name`, `version`, optional `git_sha`) |
| `/metrics` | GET | Operational telemetry (uptime, clips, xruns, active counts, per-voice ducking) |
| `/status` | GET | Current playback status (samples, voices, cache) |
| `/status/samples` | GET | List of active samples |
| `/status/voices` | GET | List of active voices (with per-voice ducking multiplier) |
| `/status/inputs` | GET | List of configured live inputs |
| `/status/cache` | GET | Cache statistics |

### Command Endpoints (Authentication Required if configured)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/command` | POST | Send any command (same JSON as MQTT) |
| `/play` | POST | Play audio file |
| `/stop` | POST | Stop samples by selector |
| `/stopall` | POST | Stop all playback |
| `/volume` | POST | Set sample volume |
| `/seek` | POST | Seek to position |
| `/speed` | POST | Change playback speed |
| `/voice/volume` | POST | Set voice volume |
| `/voice/fade_out` | POST | Fade out a voice |
| `/voice/stop` | POST | Stop a voice |
| `/cache/clear` | POST | Clear all caches |
| `/cache/invalidate` | POST | Invalidate specific cache entry |
| `/precache` | POST | Pre-cache an audio file |

## Authentication

If `auth_token` is set in config, all command endpoints require authentication:

```bash
# Bearer token in header
curl -H "Authorization: Bearer your-secret-token" \
     -X POST http://localhost:8080/stopall

# Or token in query string
curl -X POST "http://localhost:8080/stopall?token=your-secret-token"
```

Status endpoints (`/health`, `/status/*`) do not require authentication.

## Status Response Formats

### `/status` Response

Returns a summary of playback and cache state:

```json
{
  "status": "running",
  "version": "2.0.0",
  "active_samples": 2,
  "active_inputs": 0,
  "active_voices": 1,
  "output_channels": 2,
  "clip_count": 0,
  "xruns": 0,
  "cache": {
    "memory": { "entries": 3, "size_bytes": 1048576 },
    "disk": { "entries": 10, "size_bytes": 5242880 }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `active_samples` | integer | Number of samples currently playing |
| `active_inputs` | integer | Number of active live inputs |
| `active_voices` | integer | Number of active voice groups |
| `output_channels` | integer | Output channel count |
| `clip_count` | integer | Output samples the limiter held at the ceiling since startup |
| `xruns` | integer | Audio stream-error callbacks (dropouts/underruns that triggered a stream rebuild) since startup |

### `/version` Response

Build identity. `git_sha` is present only when the build injected `MQTTAUDIO_GIT_SHA`.

```json
{
  "name": "mqttaudio",
  "version": "2.0.0",
  "git_sha": "a1b2c3d"
}
```

### `/metrics` Response

Operational telemetry for monitoring. Every field is real — no placeholders.

```json
{
  "uptime_seconds": 3600.5,
  "clips": 0,
  "xruns": 0,
  "active_voices": 1,
  "active_samples": 2,
  "active_inputs": 0,
  "output_channels": 2,
  "ducking": { "music": 0.1 }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `uptime_seconds` | float | Seconds since the daemon started |
| `clips` | integer | Output samples the limiter held at the ceiling since startup |
| `xruns` | integer | Audio stream-error callbacks (dropouts/underruns) since startup |
| `active_voices` | integer | Number of active voice groups |
| `active_samples` | integer | Number of samples currently playing |
| `active_inputs` | integer | Number of active live inputs |
| `output_channels` | integer | Output channel count |
| `ducking` | object | Map of voice id to its resolved ducking multiplier (`< 1.0` = ducked); voices at full volume are omitted |

### `/status/voices` Response

Each voice carries its resolved ducking multiplier (`1.0` when not ducked):

```json
{
  "voices": [
    { "id": "music", "sample_count": 1, "volume": 1.0, "ducking_multiplier": 0.1 },
    { "id": "narration", "sample_count": 1, "volume": 1.0, "ducking_multiplier": 1.0 }
  ]
}
```

### `/status/samples` Response

Returns active samples with playback position and timing information:

```json
{
  "samples": [
    {
      "internal_id": "1",
      "id": "user-provided-id",
      "voice": "background",
      "file": "/sounds/music.mp3",
      "position": 48000,
      "position_ms": 1000,
      "total_frames": 480000,
      "total_ms": 10000,
      "sample_rate": 48000,
      "volume": 0.8,
      "voice_volume": 1.0,
      "speed": 1.0,
      "loop_mode": true,
      "progress_percent": 10
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `internal_id` | string | System-assigned unique ID |
| `id` | string/null | User-provided sample ID (if any) |
| `voice` | string | Voice group this sample belongs to |
| `file` | string | Source file path |
| `position` | integer | Current position in frames |
| `position_ms` | integer | Current position in milliseconds |
| `total_frames` | integer | Total audio length in frames |
| `total_ms` | integer | Total audio length in milliseconds |
| `sample_rate` | integer | Sample rate in Hz |
| `volume` | float | Sample volume (0.0-1.0) |
| `voice_volume` | float | Voice group volume (0.0-1.0) |
| `speed` | float | Playback speed multiplier |
| `loop_mode` | boolean | Whether looping is enabled |
| `progress_percent` | float | Playback progress (0-100) |

## WebSocket Log Streaming

Connect to `/ws` for real-time log streaming:

```javascript
const ws = new WebSocket('ws://localhost:8080/ws');
ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log(data.message);
};
```

## Example Usage

```bash
# Play a sound
curl -X POST http://localhost:8080/play \
  -H "Content-Type: application/json" \
  -d '{"file": "/sounds/effect.wav", "volume": 0.8}'

# Play with looping
curl -X POST http://localhost:8080/play \
  -H "Content-Type: application/json" \
  -d '{"file": "/sounds/music.mp3", "loop": true, "voice": "background"}'

# Stop all playback
curl -X POST http://localhost:8080/stopall

# Get status
curl http://localhost:8080/status

# Send raw command (same format as MQTT)
curl -X POST http://localhost:8080/command \
  -H "Content-Type: application/json" \
  -d '{"command": "play", "file": "/sounds/music.mp3", "loop": true}'

# Fade out a voice
curl -X POST http://localhost:8080/voice/fade_out \
  -H "Content-Type: application/json" \
  -d '{"voice": "background", "time_ms": 3000}'
```

## Running Without MQTT

The HTTP server can run independently without an MQTT broker:

```json
{
  "http": {
    "enabled": true,
    "port": 8080
  }
}
```

When HTTP is enabled and MQTT connection fails, mqttaudio continues in HTTP-only mode.

## HTTPS/TLS

The HTTP server does not support HTTPS directly. For production use with TLS, use a reverse proxy like nginx or Caddy:

```nginx
server {
    listen 443 ssl;
    server_name audio.example.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```
