# mqttaudio

A high-performance audio player controlled via MQTT and built in Rust for speed and reliability. 

Originally built for interactive installations, escape rooms, and immersive entertainment, but potentially useful to all!

## What It Does

mqttaudio listens for JSON commands over MQTT and plays audio with:

- **Multichannel routing** — Route any audio channel to any output (up to 16+ channels)
- **Voice grouping** — Control related sounds together (fade, stop, adjust volume)
- **Audio ducking** — Automatically lower background audio when narration plays
- **Live mixing** — Play 20+ sounds simultaneously without glitches
- **HTTP caching** — Stream audio from URLs with automatic caching
- **Variable speed** — Speed up, slow down, or reverse playback with optional pitch correction
- **Software LFE** — Extract low frequencies with configurable crossover and route them to a designated channel, allowing fine grained control over subwoofer output

## Quick Start

### 0. Install Prerequisites

**All platforms:**  

- Rust 1.70+.  https://rustup.rs/ will generate tailored instructions for your system.

- MQTT broker (Mosquitto is most common)



**Linux:**  Install ALSA development libraries as well:

```bash
sudo apt-get install libasound2-dev
```

### 1. Install

```bash
# From source (requires Rust 1.70+)
git clone https://github.com/bandrews/mqttaudio.git
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
mosquitto_pub -t audio/commands -m '{"command": "play", "message": {"file": "/path/to/sound.wav"}}'
```

## Common Commands

A full reference is available at [Commands](docs/commands.md) ;  the commands listed here are just to get you started.

### Play Audio

```bash
# Simple playback
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {"file": "/sounds/effect.wav"}
}'

# With options
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "https://example.com/music.mp3",
    "voice": "background",
    "volume": 0.5,
    "loop": true,
    "fade_in": 2000
  }
}'
```

### Control Playback

```bash
# Stop everything
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'

# Fade out a voice group
mosquitto_pub -t audio/commands -m '{
  "command": "voice_fade_out",
  "message": {"voice": "background", "time": 3000}
}'

# Adjust volume
mosquitto_pub -t audio/commands -m '{
  "command": "voice_volume",
  "message": {"voice": "music", "volume": 0.3}
}'
```

### Route to Specific Channels

```bash
# Play stereo audio on outputs 4 and 5
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/stereo.wav",
    "channel_map": [
      {"src": 0, "dest": 4},
      {"src": 1, "dest": 5}
    ]
  }
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

## Documentation

| Document                                   | Description                               |
| ------------------------------------------ | ----------------------------------------- |
| [Getting Started](docs/getting-started.md) | Installation, first steps, basic concepts |
| [Commands](docs/commands.md)               | Complete command reference                |
| [Configuration](docs/configuration.md)     | Config file and CLI options               |
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

| Document                             | Description                 |
| ------------------------------------ | --------------------------- |
| [Architecture](docs/architecture.md) | System design and internals |

## Supported Formats

- WAV (all bit depths and sample rates)
- MP3
- OGG/Vorbis
- FLAC

Files are automatically resampled to match your output device.

## Platform Support

- **macOS** — CoreAudio (no dependencies)
- **Linux** — ALSA (requires `libasound2-dev`)
- **Windows** — WASAPI (no dependencies)

## Performance

- Audio callback: < 1ms typical
- Cached playback latency: < 10ms
- HTTP first-play latency: 100-300ms (network dependent)
- Simultaneous samples: 20+ without glitches

## License

MIT

## Contributing

Contributions welcome! Please:

1. Run tests: `cargo test`
2. Check formatting: `cargo fmt`
3. Run lints: `cargo clippy`
