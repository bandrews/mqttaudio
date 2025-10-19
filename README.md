# mqttaudio

**MQTT-controlled multichannel audio player for interactive entertainment**

mqttaudio is a high-performance, real-time audio engine that receives commands over MQTT to play sounds with precise multichannel routing, voice grouping, and smooth fading. Built in Rust for rock-solid stability and minimal latency.

## Features

- **Polyphonic Mixing**: Play multiple audio files simultaneously
- **Multichannel Routing**: Route audio to specific output channels (supports up to 16+ channels)
- **Voice Grouping**: Group sounds together for coordinated control
- **Smooth Fading**: Fade in/out individual samples or entire voices
- **HTTP Caching**: Automatically cache remote audio files for instant playback
- **Format Support**: WAV, MP3, OGG, FLAC via symphonia decoder
- **Sample Rate Conversion**: Automatic resampling to match output device
- **Low Latency**: Sub-millisecond mixing performance, handles 20+ simultaneous sounds
- **Zero-Copy Architecture**: Efficient memory usage with Arc-based buffer sharing

## Installation

### Prerequisites

- Rust 1.70+ (install from [rustup.rs](https://rustup.rs))
- Audio system: ALSA (Linux), CoreAudio (macOS), or WASAPI (Windows)
- MQTT broker (e.g., Mosquitto)

### From Source

```bash
git clone https://github.com/yourusername/mqttaudio.git
cd mqttaudio
cargo build --release
```

The binary will be at `target/release/mqttaudio`.

### System Dependencies

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install libasound2-dev
```

**macOS:**
No additional dependencies needed (CoreAudio is built-in).

**Windows:**
No additional dependencies needed (WASAPI is built-in).

## Quick Start

### 1. Start an MQTT Broker

```bash
# Using Mosquitto
mosquitto -v
```

### 2. Run mqttaudio

```bash
# Basic usage with defaults
mqttaudio --server localhost --topic audio/commands

# With custom audio device
mqttaudio --server localhost --topic audio/commands --list-devices
mqttaudio --server localhost --topic audio/commands --device "Your Device Name"

# With config file
mqttaudio --config config.json
```

### 3. Send Commands

```bash
# Play a simple stereo file
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/path/to/sound.wav"
  }
}'

# Play with fade in and volume
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "http://example.com/music.mp3",
    "volume": 0.7,
    "fade_in": 2000,
    "loop": true,
    "voice": "background"
  }
}'

# Stop all audio
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'
```

## Configuration

### Command Line Options

```
mqttaudio [OPTIONS]

OPTIONS:
  --server <HOST>         MQTT server hostname [default: localhost]
  --port <PORT>           MQTT server port [default: 1883]
  --topic <TOPIC>         MQTT topic to subscribe to [default: audio/commands]
  --device <NAME>         Audio output device name
  --list-devices          List available audio devices and exit
  --sample-rate <RATE>    Output sample rate [default: 48000]
  --buffer-size <SIZE>    Audio buffer size in frames [default: 512]
  --channels <COUNT>      Number of output channels [default: auto-detect]
  --cache-dir <PATH>      HTTP cache directory [default: ~/.cache/mqttaudio]
  --config <FILE>         Load configuration from JSON file
  --verbose              Enable verbose logging (debug level)
  --help                  Print help information
  --version               Print version information
```

### Configuration File

Create `config.json`:

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "My Audio Interface",
    "sample_rate": 48000,
    "buffer_size": 512,
    "channels": 8
  },
  "cache": {
    "directory": "~/.cache/mqttaudio",
    "max_memory_mb": 500
  },
  "security": {
    "allowed_directories": [
      "/opt/sounds",
      "/home/user/audio"
    ]
  },
  "logging": {
    "level": "info"
  }
}
```

Then run:
```bash
mqttaudio --config config.json
```

## MQTT Commands

All commands are JSON objects sent to the configured MQTT topic.

### Play Audio

```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/audio.wav",
    "voice": "effects",
    "channel_map": [
      {"src": 0, "dest": 2},
      {"src": 1, "dest": 3}
    ],
    "volume": 0.8,
    "loop": false,
    "fade_in": 1000,
    "max_play_length": 30000
  }
}
```

**Parameters:**
- `file` (required): URL (http/https) or local file path
- `voice` (optional): Voice name for grouping
- `channel_map` (optional): Array of `{"src": N, "dest": M}` channel routes
- `volume` (optional): 0.0 to 1.0, default 1.0
- `loop` (optional): Loop playback, default false
- `fade_in` (optional): Fade in duration in milliseconds
- `max_play_length` (optional): Maximum playback duration in milliseconds

### Stop All Audio

```json
{
  "command": "stopall"
}
```

### Voice Commands

**Stop a voice:**
```json
{
  "command": "voice_stop",
  "message": {
    "voice": "background"
  }
}
```

**Fade out a voice:**
```json
{
  "command": "voice_fade_out",
  "message": {
    "voice": "background",
    "time": 3000
  }
}
```

**Adjust voice volume:**
```json
{
  "command": "voice_volume",
  "message": {
    "voice": "music",
    "volume": 0.5
  }
}
```

### Cache Commands

**Precache a file:**
```json
{
  "command": "precache",
  "message": {
    "file": "http://example.com/bigfile.wav"
  }
}
```

**Clear entire cache:**
```json
{
  "command": "cache_clear"
}
```

**Invalidate specific file:**
```json
{
  "command": "cache_invalidate",
  "message": {
    "file": "http://example.com/updated.wav"
  }
}
```

### Global Fade Out

```json
{
  "command": "fadeout",
  "message": {
    "time": 5000
  }
}
```

## Examples

### Background Music with Sound Effects

```bash
# Start looping background music
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/ambient.ogg",
    "voice": "music",
    "volume": 0.3,
    "loop": true,
    "fade_in": 2000
  }
}'

# Play one-shot sound effect
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/doorbell.wav",
    "voice": "effects",
    "volume": 1.0
  }
}'

# Fade out music after 30 seconds
sleep 30
mosquitto_pub -t audio/commands -m '{
  "command": "voice_fade_out",
  "message": {
    "voice": "music",
    "time": 3000
  }
}'
```

### Multichannel Surround Sound

```bash
# Play quad audio routed to specific outputs
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/surround.wav",
    "channel_map": [
      {"src": 0, "dest": 0},
      {"src": 1, "dest": 1},
      {"src": 2, "dest": 6},
      {"src": 3, "dest": 7}
    ],
    "volume": 0.8
  }
}'
```

### HTTP Streaming with Caching

```bash
# First play downloads and caches
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "https://example.com/audio/theme.mp3"
  }
}'

# Second play is instant (served from cache)
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "https://example.com/audio/theme.mp3"
  }
}'
```

## Performance

mqttaudio is designed for real-time audio with strict latency requirements:

- **Audio callback time**: < 1% of buffer duration (typically ~20 μs for 10 samples)
- **Max simultaneous samples**: 20+ without glitches
- **Latency (cached files)**: < 10 ms
- **Latency (HTTP files, first play)**: 100-300 ms (network dependent)
- **Memory**: Efficient Arc-based sharing, minimal allocations in audio thread

Run benchmarks:
```bash
cargo bench
```

## Development

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test mixer

# With output
cargo test -- --nocapture
```

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# With verbose output
cargo build --verbose
```

### Project Structure

```
mqttaudio/
├── src/
│   ├── main.rs           # Entry point, CLI handling
│   ├── config.rs         # Configuration loading
│   ├── audio/
│   │   ├── engine.rs     # Audio engine coordinator
│   │   ├── mixer.rs      # Real-time mixer (audio callback)
│   │   ├── decoder.rs    # Audio file decoding
│   │   ├── resampler.rs  # Sample rate conversion
│   │   └── types.rs      # Audio data types
│   ├── mqtt/
│   │   ├── client.rs     # MQTT connection
│   │   └── commands.rs   # Command parsing
│   ├── cache/
│   │   ├── disk.rs       # Disk cache implementation
│   │   └── memory.rs     # Memory cache
│   └── voice.rs          # Voice management
├── benches/
│   └── mixer_benchmark.rs # Performance benchmarks
├── docs/                 # Architecture documentation
├── tests/
│   └── audio/            # Test audio files
└── legacy/               # Original Python implementation
```

## Troubleshooting

### No Audio Output

1. List available devices:
   ```bash
   mqttaudio --list-devices
   ```

2. Specify device explicitly:
   ```bash
   mqttaudio --device "Your Device Name"
   ```

3. Check sample rate compatibility:
   ```bash
   mqttaudio --sample-rate 44100
   ```

### MQTT Connection Issues

1. Verify broker is running:
   ```bash
   mosquitto_sub -t audio/commands -v
   ```

2. Check server/port:
   ```bash
   mqttaudio --server mqtt.example.com --port 1883
   ```

3. Enable verbose logging:
   ```bash
   mqttaudio --verbose
   ```

### HTTP Download Failures

1. Check network connectivity
2. Verify URL is accessible
3. Check cache directory permissions:
   ```bash
   ls -la ~/.cache/mqttaudio
   ```

### Audio Glitches

1. Increase buffer size (trades latency for stability):
   ```bash
   mqttaudio --buffer-size 1024
   ```

2. Check CPU usage and reduce simultaneous samples
3. Run benchmarks to verify performance

## License

MIT

## Credits

Built with:
- [cpal](https://github.com/RustAudio/cpal) - Cross-platform audio I/O
- [symphonia](https://github.com/pdeljanov/Symphonia) - Audio decoding
- [rubato](https://github.com/HEnquist/rubato) - Sample rate conversion
- [rumqttc](https://github.com/bytebeamio/rumqtt) - MQTT client

## Contributing

Contributions welcome! Please:
1. Run tests: `cargo test`
2. Run benchmarks: `cargo bench`
3. Format code: `cargo fmt`
4. Check lints: `cargo clippy`

See `docs/` for architecture details and development guidelines.
