# Configuration

mqttaudio can be configured via a JSON file, command-line arguments, or both. Command-line arguments override config file settings.

## Command-Line Options

```
mqttaudio [OPTIONS]

OPTIONS:
  -c, --config <FILE>          Load configuration from JSON file
  -s, --server <HOST>          MQTT server hostname [default: localhost]
  -p, --port <PORT>            MQTT server port [default: 1883]
  -t, --topic <TOPIC>          MQTT topic to subscribe to
  --mqtt-username <USER>       MQTT broker username for authentication
  --mqtt-password <PASS>       MQTT broker password for authentication
  -d, --device <NAME>          Audio output device name
  -r, --sample-rate <RATE>     Output sample rate [default: 48000]
  -n, --channels <COUNT>       Number of output channels [default: auto-detect]
  --lfe-channel <INDEX>        LFE (subwoofer) channel index for bass management
  --crossover-frequency <HZ>   Crossover frequency for bass management [default: 80]
  --log-topic <TOPIC>          MQTT topic to publish log messages to
  -v, --verbose                Enable verbose logging (debug level)
  --list-devices               List available audio output devices and exit
  --list-inputs                List available audio input devices and exit
  --help                       Print help information
  --version                    Print version information
```

## Configuration File

### Location

Specify with `--config`, or mqttaudio searches these locations:

1. `./mqttaudio.json` (current directory)
2. `~/.config/mqttaudio/config.json`
3. `/etc/mqttaudio/config.json` (Linux/macOS)

### Complete Example

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "USB Audio Interface",
    "sample_rate": 48000,
    "buffer_size": 512,
    "channels": 8,
    "channel_aliases": {
      "front_left": 0,
      "front_right": 1,
      "center": 2,
      "lfe": 3,
      "surround_left": 4,
      "surround_right": 5
    }
  },
  "cache": {
    "directory": "~/.mqttaudio/cache",
    "precache": [
      "/opt/sounds/startup.wav",
      "/opt/sounds/effects",
      "https://example.com/common-effect.mp3"
    ]
  },
  "bass_management": {
    "enabled": true,
    "lfe_channel": "lfe",
    "crossover_frequency_hz": 80,
    "source_channels": ["front_left", "front_right", "center", "surround_left", "surround_right"],
    "remove_bass_from_sources": false
  },
  "inputs": [
    {
      "device": "USB Microphone",
      "volume": 0.8,
      "voice_id": "mic_1",
      "routes": [
        {"source_channel": 0, "dest_channel": "surround_left"},
        {"source_channel": 0, "dest_channel": "surround_right"}
      ],
      "latency_ms": 25
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ],
  "security": {
    "allowed_directories": [
      "/opt/sounds",
      "/home/user/audio"
    ]
  },
  "logging": {
    "level": "info",
    "mqtt_topic": "audio/logs"
  },
  "advanced": {
    "resampler_quality": "fast"
  }
}
```

---

## Configuration Sections

### mqtt

MQTT broker connection settings.

```json
"mqtt": {
  "server": "localhost",
  "port": 1883,
  "topic": "audio/commands",
  "username": "myuser",
  "password": "mypassword"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server` | string | `"localhost"` | MQTT broker hostname or IP |
| `port` | integer | `1883` | MQTT broker port |
| `topic` | string | *required* | Topic to subscribe to (supports `#` and `+` wildcards) |
| `username` | string | — | Username for MQTT authentication |
| `password` | string | — | Password for MQTT authentication |

**Authentication:** If your MQTT broker requires authentication, provide both `username` and `password`. These can also be passed via command line with `--mqtt-username` and `--mqtt-password`.

### audio

Audio output settings.

```json
"audio": {
  "device": "USB Audio Interface",
  "sample_rate": 48000,
  "buffer_size": 512,
  "channels": 8,
  "channel_aliases": {
    "front_left": 0,
    "front_right": 1,
    "lfe": 3,
    "gamemaster_speakers": 4
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `device` | string | system default | Audio device name (use `--list-devices` to see options) |
| `sample_rate` | integer | `48000` | Output sample rate in Hz |
| `buffer_size` | integer | `512` | Buffer size in frames (lower = less latency, more CPU) |
| `channels` | integer | auto-detect | Number of output channels |
| `channel_aliases` | object | `{}` | Named aliases for channel numbers |

#### Channel Aliases

The `channel_aliases` field lets you define meaningful names for channel numbers. These aliases can then be used anywhere a channel is referenced in the config file (bass management, input routes):

```json
"audio": {
  "channel_aliases": {
    "front_left": 0,
    "front_right": 1,
    "center": 2,
    "lfe": 3,
    "surround_left": 4,
    "surround_right": 5
  }
},
"bass_management": {
  "lfe_channel": "lfe",
  "source_channels": ["front_left", "front_right", "center"]
},
"inputs": [{
  "routes": [
    {"source_channel": 0, "dest_channel": "front_left"},
    {"source_channel": 0, "dest_channel": "front_right"}
  ]
}]
```

This makes configurations more readable and less error-prone. Channel aliases can also be used in MQTT play commands (see [Commands](commands.md)).

### cache

File caching settings.

```json
"cache": {
  "directory": "~/.mqttaudio/cache",
  "precache": [
    "/sounds/startup.wav",
    "/opt/effects",
    "https://example.com/common.mp3"
  ]
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `directory` | string | `~/.mqttaudio/cache` | Disk cache directory |
| `precache` | array | `[]` | Files or directories to cache on startup |

#### Precaching

The `precache` array accepts:
- **Individual files**: Exact paths to audio files
- **Directories**: All supported audio files (WAV, MP3, OGG, FLAC) in the folder are cached
- **HTTP URLs**: Remote files are downloaded and cached

```json
"precache": [
  "/sounds/critical-effect.wav",
  "/opt/installation/ambient",
  "https://cdn.example.com/intro.mp3"
]
```

Directory precaching is not recursive — only files directly in the specified folder are cached. Subdirectories must be listed separately if needed.

### bass_management

LFE/subwoofer routing.

```json
"bass_management": {
  "enabled": true,
  "lfe_channel": 3,
  "crossover_frequency_hz": 80,
  "source_channels": [0, 1, 2, 4, 5],
  "remove_bass_from_sources": false
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable bass management |
| `lfe_channel` | integer or string | — | Output channel for subwoofer (number or alias) |
| `crossover_frequency_hz` | integer | `80` | Crossover frequency in Hz |
| `source_channels` | array | — | Channels to extract bass from (numbers or aliases) |
| `remove_bass_from_sources` | boolean | `false` | Remove bass from source channels after extraction |

Channel aliases from `audio.channel_aliases` can be used for `lfe_channel` and `source_channels`.

See [Bass Management](features/bass-management.md) for details.

### inputs

Microphone/input device configuration.

```json
"inputs": [
  {
    "device": "USB Microphone",
    "volume": 0.8,
    "voice_id": "gm_mic",
    "routes": [
      {"source_channel": 0, "dest_channel": 4},
      {"source_channel": 0, "dest_channel": 5}
    ],
    "latency_ms": 25
  }
]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `device` | string | *required* | Input device name (use `--list-inputs`) |
| `volume` | float | `1.0` | Input volume (0.0 to 1.0) |
| `voice_id` | string | — | Voice name for ducking integration |
| `routes` | array | *required* | Channel routing (source → dest, can use aliases) |
| `latency_ms` | integer | `25` | Buffer latency (5-500ms) |

The `dest_channel` in routes can use channel aliases defined in `audio.channel_aliases`.

See [Microphone Input](features/microphone-input.md) for details.

### ducking_rules

Automatic volume ducking configuration.

```json
"ducking_rules": [
  {
    "primary_voice": "narration",
    "ducked_voices": ["music", "effects"],
    "target_volume": 0.15,
    "fade_duration_ms": 2000
  }
]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `primary_voice` | string | *required* | Voice that triggers ducking |
| `ducked_voices` | array | *required* | Voices to reduce in volume |
| `target_volume` | float | *required* | Volume to duck to (0.0 to 1.0) |
| `fade_duration_ms` | integer | *required* | Fade duration in milliseconds |

See [Audio Ducking](features/ducking.md) for details.

### security

File access restrictions.

```json
"security": {
  "allowed_directories": [
    "/opt/sounds",
    "~/audio"
  ]
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_directories` | array | `[]` | Directories allowed for local file access |

**Notes:**
- If empty, only HTTP/HTTPS URLs can be played
- `~` expands to the user's home directory
- Path traversal (`../`) is blocked

### logging

Log output settings.

```json
"logging": {
  "level": "info",
  "mqtt_topic": "audio/logs"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `level` | string | `"info"` | Log level: `error`, `warn`, `info`, `debug`, `trace` |
| `mqtt_topic` | string | — | Publish logs to this MQTT topic |

When `mqtt_topic` is set, log entries are published as JSON:

```json
{
  "timestamp": "2025-01-01T12:00:00.000000Z",
  "level": "INFO",
  "target": "mqttaudio",
  "message": "Playing: /sounds/effect.wav"
}
```

### advanced

Performance tuning and advanced settings. Most users won't need to change these.

```json
"advanced": {
  "resampler_quality": "fast"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `resampler_quality` | string | `"fast"` | Sample rate conversion quality preset |

#### Resampler Quality

When audio files have a different sample rate than the output device (e.g., a 44.1kHz MP3 played on a 48kHz device), mqttaudio resamples them. The quality setting controls the tradeoff between speed and audio fidelity.

| Preset | Speed | Quality | Use Case |
|--------|-------|---------|----------|
| `"fast"` | ~60ms/min | Good | **Default.** Real-time playback, games, interactive installations |
| `"medium"` | ~95ms/min | Better | Balanced option when you want slightly better quality |
| `"high"` | ~190ms/min | High | Critical listening, when all content is precached |
| `"maximum"` | ~230ms/min | Best | Studio quality, when latency doesn't matter |

**Notes:**
- Times shown are for resampling 1 minute of stereo audio (44.1kHz → 48kHz) on a typical system
- The "fast" preset is optimized for interactive use with <100ms cold start targets
- If all your audio files already match the output device sample rate, this setting has no effect
- For best latency, precache audio files that must start instantly (see `cache.precache`)

**Recommendation:** Leave at `"fast"` unless you have specific quality requirements AND your use case can tolerate longer loading times. If quality is critical, precache your audio files at startup.

---

## Example Configurations

### Minimal

```json
{
  "mqtt": {
    "topic": "audio/commands"
  }
}
```

### Simple Stereo

```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "audio/commands"
  },
  "security": {
    "allowed_directories": ["/opt/sounds"]
  }
}
```

### Multichannel Installation

```json
{
  "mqtt": {
    "server": "exhibit-server.local",
    "topic": "gallery/audio"
  },
  "audio": {
    "device": "MOTU 8A",
    "sample_rate": 48000,
    "channels": 8
  },
  "cache": {
    "precache": [
      "/sounds/ambient.mp3",
      "/sounds/welcome.wav"
    ]
  },
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "ambient"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ],
  "security": {
    "allowed_directories": ["/opt/gallery/sounds"]
  }
}
```

### Escape Room with Microphone

```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "escaperoom/audio"
  },
  "audio": {
    "device": "USB Audio Interface"
  },
  "inputs": [
    {
      "device": "Gamemaster Headset",
      "volume": 0.9,
      "voice_id": "gm_mic",
      "routes": [
        {"source_channel": 0, "dest_channel": 4},
        {"source_channel": 0, "dest_channel": 5},
        {"source_channel": 0, "dest_channel": 6},
        {"source_channel": 0, "dest_channel": 7}
      ],
      "latency_ms": 25
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "gm_mic",
      "ducked_voices": ["ambient", "effects"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    }
  ],
  "security": {
    "allowed_directories": ["/opt/escaperoom/audio"]
  }
}
```

### 5.1 Surround with Bass Management

```json
{
  "mqtt": {
    "topic": "theater/audio"
  },
  "audio": {
    "device": "Surround Sound Card",
    "channels": 6
  },
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2, 4, 5],
    "remove_bass_from_sources": true
  }
}
```

---

## Environment Variables

| Variable | Description |
|----------|-------------|
| `MQTTAUDIO_CONFIG` | Path to config file |
| `MQTTAUDIO_CACHE_DIR` | Override cache directory |
| `RUST_LOG` | Rust logging configuration (e.g., `mqttaudio=debug`) |
