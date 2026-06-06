# mqttaudio

A high-performance audio player controlled via MQTT or HTTP and built in Rust for speed and reliability. 

Originally built for interactive installations, escape rooms, and immersive entertainment, but potentially useful to all!

## What It Does

mqttaudio listens for JSON commands over MQTT or a REST endpoint, and plays audio with:

- **Multichannel routing** — Route any audio channel to any output (up to 16+ channels)
- **Voice grouping** — Control related sounds together (fade, stop, adjust volume)
- **Audio ducking** — Automatically lower background audio when narration plays
- **Live mixing** — Play 20+ sounds simultaneously without glitches
- **HTTP caching** — Stream audio from URLs with automatic caching
- **Variable speed** — Speed up, slow down, or reverse playback with optional pitch correction
- **Software LFE** — Extract low frequencies with a 4th-order Linkwitz-Riley crossover and route them to a designated subwoofer channel, with level kept independent of the number of source channels
- **REST API** — Optional HTTP server with REST endpoints mirroring MQTT commands, plus WebSocket for log streaming
- **Web UI** — A React + Material UI control & monitoring app lives in [`webui/`](webui/), served via a reverse-proxy sidecar. See [**docs/webui/README.md**](docs/webui/README.md) to run it, and the [sprint program](docs/webui/SPRINT-TRACKER.md) for how it was built. It monitors live playback and exercises the full feature set (multi-voice, channel-map matrix mixing, ducking/gain tuning, cache state, opt-in live progress + meters) without hand-crafting MQTT/HTTP.

## Quick Start

### 0. Install Prerequisites

**All platforms:**  

- Rust 1.70+.  https://rustup.rs/ will generate tailored instructions for your system.

- MQTT broker (Mosquitto is most common)



**Linux:**  Install ALSA and OpenSSL development libraries, C++ build tools, clang, and pkg-config:

```bash
# Debian/Ubuntu
sudo apt-get install libasound2-dev libssl-dev pkg-config build-essential clang libclang-dev

# Fedora/RHEL
sudo dnf install alsa-lib-devel openssl-devel pkgconf-pkg-config gcc-c++ clang clang-devel

# Arch
sudo pacman -S alsa-lib base-devel clang
```

### 1. Install

```bash
# From source (requires Rust 1.70+)
git clone -b refactor https://github.com/bandrews/mqttaudio.git
cd mqttaudio
cargo build --release

# Binary is at target/release/mqttaudio
```

### 2. Run mqttaudio

```bash
# Basic usage
./mqttaudio --server localhost --topic audio/commands

# List available audio devices
./mqttaudio --list-devices

# Use a specific device
./mqttaudio --server localhost --topic audio/commands --device "USB Audio"
```

### 3. Play Your First Sound

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/path/to/sound.wav"}'
```

## Common Commands

A full reference is available at [Commands](docs/commands.md) ;  the commands listed here are just to get you started.

### Play Audio

```bash
# Simple playback
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/effect.wav"
}'

# With options
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "https://example.com/music.mp3",
  "voice": "background",
  "volume": 0.5,
  "loop": true,
  "fade_in": 2000
}'
```

### Control Playback

```bash
# Stop everything
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'

# Fade out a voice group
mosquitto_pub -t audio/commands -m '{
  "command": "voice_fade_out",
  "voice": "background",
  "time": 3000
}'

# Adjust volume
mosquitto_pub -t audio/commands -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.3
}'
```

### Route to Specific Channels

```bash
# Play stereo audio on outputs 4 and 5
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/stereo.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5}
  ]
}'

# Downmix 4 channels to stereo with a per-route gain so the summed
# channels don't clip (gain is optional, default 1.0)
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/quad.wav",
  "channel_map": [
    {"src": 0, "dest": 0, "gain": 0.5},
    {"src": 2, "dest": 0, "gain": 0.5},
    {"src": 1, "dest": 1, "gain": 0.5},
    {"src": 3, "dest": 1, "gain": 0.5}
  ]
}'
```

## Configuration

For production use, create a config file:

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "USB Audio Interface",
    "sample_rate": 48000
  },
  "security": {
    "allowed_directories": ["/opt/sounds", "/home/user/audio"]
  }
}
```

Then run with:

```bash
./mqttaudio --config config.json
```

### Audio output behavior

A few mixer behaviors are worth knowing (see `docs/configuration.md` for the full reference):

- **Output limiter.** The summed output is held below a configurable ceiling by a soft-knee limiter instead
  of a brickwall clamp, so loud mixes stay clean rather than distorting. Configure it under `audio`:
  `output_ceiling_db` (limiter ceiling in dBFS, default `-1.0`, range `-60.0`..`0.0`) and `master_gain` (a
  linear bus gain applied before limiting, default `1.0`, range `0.0`..`8.0`). The `/status` endpoint reports
  `clip_count`, how many output samples the limiter has had to hold at the ceiling.
- **Play `volume` is clamped to `[0, 1]`.** A `play` with `volume` above `1.0` (or negative) is clamped,
  matching the `voice_volume` command. (Previously a Play could amplify above unity.)
- **Per-channel calibration applies.** `audio.channel_volumes` (per-output-channel gains, by index or alias)
  is applied as a final gain stage.
- **Looped crossfades are seamless.** `loop: true` with `crossfade_ms` uses an equal-power crossfade with
  correct overlap on wrap (no midpoint dip, no double-triggered head). The crossfade applies **only at loop
  boundaries**, and only when the clip is longer than twice the crossfade. A `crossfade_ms` on a one-shot
  (no `loop: true`), or longer than half the clip, never engages and is logged as a warning at dispatch.
- **Ducking restore honors the rule's fade.** When a ducking primary goes idle, ducked voices recover over
  the triggering rule's `fade_duration_ms` (not a fixed 2 s).
- **Playback speed.** `speed` supports `-100`..`100` (negative = reverse) and uses cubic interpolation for
  clean fractional speeds. There is no pitch correction by default; set `pitch_correction: true` to preserve
  pitch. Note that speeds **above** `1.0` without pitch correction will alias (no anti-aliasing on the
  fast path) — use pitch correction for clean large speed-ups.

### Large files, memory, and streaming

mqttaudio keeps memory bounded automatically, so a long cue — even a 2-hour 5.1 mix — never has to fit in RAM:

- **Auto-windowing (default).** Every `play` decodes to f32 in memory, costing roughly
  `duration × rate × channels × 4 bytes` — about **1.4 GB/hour stereo** and **4.2 GB/hour for 5.1** at 48 kHz.
  By default (`mode=auto`) mqttaudio probes the file's header and, if its decoded size or duration exceeds the
  thresholds **or would not fit the memory budget**, plays it through a bounded **window** (a fixed ring,
  default 1.5 s) fed by a background decoder — `O(window)` memory and a low time-to-first-sample — instead of
  fully decoding it. Small assets (SFX, voiceovers) still fully load, with all features.
- **Windowed voices play forward only.** Seek, loop-crossfade, reverse, variable speed, and pitch correction
  do not apply to a windowed (streamed) voice. Force a full load with `"mode": "full"` on the `play` (it still
  cannot exceed the memory cap), or force windowing with `"mode": "stream"`. Tune the defaults with
  `cache.load_mode`, `cache.full_load_max_bytes`, `cache.full_load_max_seconds`, and `cache.stream_window_ms`.
  Windowing applies to both local files and `http(s)://` URLs: an uncached HTTP URL is windowed by the same
  size/budget decision (a live stream with no `Content-Length` always windows), streaming through a bounded,
  back-pressured reader so even a multi-hour remote cue stays within `O(window)` memory. A windowed HTTP URL
  that is cacheable (has a `Content-Length`) is also teed to the disk cache as it plays, so a replay hits disk
  with no extra download; pass `"cacheable": false` on the play to treat a URL as live (window, never persist).
- **Memory budget (never camps all RAM).** The decoded-audio cache has a hard cap. By default it auto-detects
  a bounded size — about 40 % of *available* RAM, clamped to `[128 MiB, 1 GiB]` — so it is safe on a 2 GB
  Raspberry Pi without starving other processes. Override with `cache.memory_budget`:
  `{"mode":"auto","fraction":0.4,"floor_mb":128,"ceiling_mb":1024}`, `{"mode":"explicit","mb":512}`, or
  `{"mode":"unlimited"}` (opt out — risks OOM). The simple `cache.max_memory_mb` still works: `0` (the
  default) = auto, a positive value = an explicit MiB cap. `GET /metrics` reports the cache's resident bytes,
  entries, and headroom.

### Cache freshness

By default a warm cache serves instantly but still picks up changes:

- **Local files:** on each play the file's mtime+size are checked (a cheap stat); if it changed on disk it is
  re-decoded. Edit an asset and the next play hears it — no restart.
- **Remote (HTTP) files:** refreshed in the background by a periodic tick past the revalidation window, so a
  play never blocks on the network.
- Set `cache.freshness` to `trusting` (default), `dev` (re-check every load), or `pinned` (never auto-check),
  or override per play with `"freshness": "..."`. The `cache_reload` command (MQTT, or `POST /cache/reload`)
  forces an entry fresh+instant — handy after a content pipeline republishes an asset.

### Live input behavior

When `config.inputs` is configured to mix a microphone or line input (see
[Microphone Input](docs/features/microphone-input.md)):

- **Drift-bounded capture.** Every input runs through async sample-rate conversion whose ratio is steered
  from the ring-buffer fill, so an input clocked by a different device than the output stays glitch-free over
  long sessions instead of slowly drifting into periodic dropouts — even when the nominal rates match.
- **`voice_volume` affects inputs.** A `voice_volume` command targeting an input's `voice_id` now ramps that
  input's level even when no sample is playing on the voice (previously a no-op).
- **`input_mute` restores the prior level.** Unmuting returns the input to the volume it had when muted (e.g.
  a calibrated `0.7`), not a hardcoded `1.0`. Setting an explicit `input_volume` clears the muted state.
- **Inputs can trigger ducking.** A mic whose `voice_id` is a ducking rule's `primary_voice` ducks that
  rule's background voices while its stream is open.
- **Out-of-range routes warn.** A route reading a source channel the device does not have is logged once at
  startup (the mixer still drops it).
- **Routing to the LFE channel bypasses the crossover.** With bass management enabled, an input route (or a
  Play `channel_map`) whose destination is the configured `lfe_channel` sends that content to the sub
  full-range — the crossover does not high-pass it — and bass management then sums the extracted bass on top.
  A configured input route that does this is logged once at startup. Route to the LFE deliberately. See
  [Bass Management](docs/features/bass-management.md).

### Hardening (optional)

mqttaudio runs open by default so it stays easy to use on a trusted LAN. Each of these lockdowns is **opt-in** and leaves existing open deployments unchanged when unset:

- **Restrict local file access** — set `security.allowed_directories` to the folders sounds may be loaded from (paths outside them, and traversal/symlink escapes, are rejected). Empty/unset = any local path is allowed.
- **Encrypt the MQTT connection** — add an `[mqtt.tls]` block (`ca_path` for a private CA, or omit it for public CAs). Plain TCP stays the default on every port, including 8883.
- **Require an HTTP token** — set `http.require_auth` to require the bearer token on the status/command/WebSocket endpoints (the health endpoint stays open). A warning is logged if the server binds a non-loopback address without auth.

See [Configuration](docs/configuration.md) and [HTTP API](docs/http-api.md) for details.

## HTTP REST API (Optional)

mqttaudio includes an optional HTTP server that provides REST endpoints mirroring all MQTT commands.  This server can be used in addition to or instead of an MQTT connection.

Enable it with `--http-port 8080` or via config file. See [HTTP API](docs/http-api.md) for full documentation.

```bash
# Quick start
./mqttaudio --server localhost --topic audio/commands --http-port 8080

# Play via HTTP
curl -X POST http://localhost:8080/play \
  -H "Content-Type: application/json" \
  -d '{"file": "/sounds/effect.wav"}'
```

The server also exposes observability endpoints (no auth in open mode):

- `GET /version` — build identity (`name`, `version`, and `git_sha` when the build injected `MQTTAUDIO_GIT_SHA`).
- `GET /metrics` — operational telemetry: `uptime_seconds`, `clips` (limiter holds), `xruns` (audio
  stream-error/dropout count), active voice/sample/input counts, and a per-voice `ducking` map of resolved
  multipliers. Every value is real, suitable for scraping into a monitor.
- `GET /status` and `/status/voices` also carry the limiter `clip_count`, the `xruns` counter, and (on
  `/status/voices`) each voice's current `ducking_multiplier`.

## Production Deployment

### Run under systemd

A hardened service unit is provided at [`packaging/mqttaudio.service`](packaging/mqttaudio.service). It runs
the release binary as a dedicated unprivileged user, restarts on failure, and locks the process down (read-only
filesystem except a writable cache state directory, no new privileges, ALSA device access only).

```bash
# Build, then install the binary, a service user, your config, and the unit:
cargo build --release
sudo install -Dm755 target/release/mqttaudio /usr/local/bin/mqttaudio
sudo useradd --system --no-create-home --shell /usr/sbin/nologin mqttaudio
sudo install -Dm644 your-config.json /etc/mqttaudio/config.json
sudo install -Dm644 packaging/mqttaudio.service /etc/systemd/system/mqttaudio.service

# Point the on-disk cache at the writable state directory the unit grants:
#   "cache": { "directory": "/var/lib/mqttaudio/cache" }

sudo systemctl daemon-reload
sudo systemctl enable --now mqttaudio
sudo journalctl -u mqttaudio -f
```

The daemon exits non-zero when it cannot recover the audio device, so systemd's `Restart=on-failure` brings it
back. See the comments at the top of the unit for the full install/hardening notes.

The unit also sets `MemoryMax=75%` (with `MemoryAccounting=true`) as an OS-level memory backstop. The daemon
already auto-sizes its decoded-audio cache to a fraction of available RAM and windows assets that would not fit
(see [Large files, memory, and streaming](#large-files-memory-and-streaming)), but this hard cgroup limit bounds
the *whole* process — so even a pathological case can only OOM-kill this one service (which then restarts),
never the box. Tune it to your hardware (e.g. an absolute `MemoryMax=1500M`). It deliberately omits `MemoryHigh=`,
whose reclaim throttling can stall the audio thread; the hard ceiling alone is the backstop. If you run the
daemon outside systemd, apply an equivalent cgroup `memory.max` (or a container `--memory` limit) to get the
same guarantee.

### Structured (JSON) logging

For log aggregation, set the log format to JSON. Each record is then emitted as one JSON object per line
(line-delimited JSON), which `journalctl`, Loki, or the ELK stack can parse directly:

```json
{
  "logging": {
    "level": "info",
    "format": "json"
  }
}
```

`format` defaults to `"text"` (the human-readable console rendering). The MQTT log topic (if configured)
keeps publishing alongside whichever console format is selected.

## Documentation

| Document                                   | Description                               |
| ------------------------------------------ | ----------------------------------------- |
| [Getting Started](docs/getting-started.md) | Installation, first steps, basic concepts |
| [Commands](docs/commands.md)               | Complete command reference                |
| [Configuration](docs/configuration.md)     | Config file and CLI options               |
| [HTTP API](docs/http-api.md)               | REST endpoints and WebSocket streaming    |
| [Troubleshooting](docs/troubleshooting.md) | Common issues and solutions               |

### Feature Guides

| Feature                                               | Description                     |
| ----------------------------------------------------- | ------------------------------- |
| [Channel Routing](docs/features/channel-routing.md)   | Multichannel audio routing      |
| [Voice Management](docs/features/voice-management.md) | Grouping and controlling sounds |
| [Audio Ducking](docs/features/ducking.md)             | Automatic volume reduction      |
| [Bass Management](docs/features/bass-management.md)   | LFE/subwoofer routing           |
| [Microphone Input](docs/features/microphone-input.md) | Live audio input mixing         |
| [Playback Control](docs/features/playback-control.md) | Seek, speed, reverse            |
| [Caching](docs/features/caching.md)                   | HTTP caching and precaching     |

### For Developers

| Document                             | Description                       |
| ------------------------------------ | --------------------------------- |
| [Architecture](docs/architecture.md) | System design and internals       |
| [Contributing](CONTRIBUTING.md)      | Building and testing instructions |

## Supported Formats

- WAV (all bit depths and sample rates)
- MP3
- OGG/Vorbis
- FLAC

Files are automatically resampled to match your output device.

## Platform Support

- **macOS** — CoreAudio (no dependencies)
- **Linux** — ALSA (requires `libasound2-dev`, `libssl-dev`, `pkg-config`, `clang`, and `libclang-dev`)
- **Windows** — WASAPI (requires C++ build tools)

## Performance

- Audio callback: < 1ms typical
- Cached playback latency: < 10ms
- HTTP first-play latency: 100-300ms (network dependent)
- Simultaneous samples: 20+ without glitches

## License

[MIT License](LICENSE)

## AI Statement

While this tool is human designed, reviewed, tested and maintained, the bulk of core development was performed by Claude Opus 4.5 or later.  

If you prefer a purely human developed alternative, the much simpler 1.0 version hand-built in C++ is still available.  Be aware the legacy version is end-of-life and all further development and maintenance will take place on the 2.0 branch.

## Contributing

Contributions welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, testing, and guidelines.

## Copyright

Copyright (c) 2016-2025 Mo Fang Heavy Industries LLC.  All Rights Reserved.
