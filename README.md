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
- **Software LFE** — Extract low frequencies with configurable crossover and route them to a designated channel, allowing fine grained control over subwoofer output
- **REST API** — Optional HTTP server with REST endpoints mirroring MQTT commands, plus WebSocket for log streaming

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
  correct overlap on wrap (no midpoint dip, no double-triggered head).
- **Ducking restore honors the rule's fade.** When a ducking primary goes idle, ducked voices recover over
  the triggering rule's `fade_duration_ms` (not a fixed 2 s).
- **Playback speed.** `speed` supports `-100`..`100` (negative = reverse) and uses cubic interpolation for
  clean fractional speeds. There is no pitch correction by default; set `pitch_correction: true` to preserve
  pitch. Note that speeds **above** `1.0` without pitch correction will alias (no anti-aliasing on the
  fast path) — use pitch correction for clean large speed-ups.

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
