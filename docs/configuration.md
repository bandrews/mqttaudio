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
  -n, --channels <COUNT>       Number of output channels [default: use max available]
  --lfe-channel <INDEX>        LFE (subwoofer) channel override for bass management
  --crossover-frequency <HZ>   Crossover frequency override for bass management
  --log-topic <TOPIC>          MQTT topic to publish log messages to
  --http-port <PORT>           Enable the HTTP server on this port
  --max-cache-mb <MB>          Memory cache limit in MB (0 = unlimited)
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
| `client_id` | string | auto | MQTT client id; when unset a random `mqttaudio_<hex>` id is generated |
| `reconnect_delay_seconds` | integer | `10` | Wait between reconnect attempts after an MQTT error |

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
| `channel_names` | object | `{}` | Display labels for channel numbers |
| `channel_volumes` | object | `{}` | Per-channel output gain |

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

#### Channel Names vs Channel Aliases

Two fields name channels and they are not interchangeable:

| Field | Direction | Used for |
|-------|-----------|----------|
| `channel_aliases` | name → number | Resolving channels in routes, bass management and play commands |
| `channel_names` | number → name | Labelling channels for display |

```json
"audio": {
  "channel_aliases": { "booth": 6 },
  "channel_names": { "6": "booth" }
}
```

Routing resolves against `channel_aliases` only. A name defined in
`channel_names` and then used as a `dest_channel` fails validation with the
entry you need to add:

```
inputs[0].routes[0].dest_channel: Unknown channel 'booth': audio.channel_names
labels channel 6 as 'booth', but routing resolves against audio.channel_aliases
- add "booth": 6 there
```

#### Channel Volumes

`channel_volumes` sets a per-channel output gain, applied to the finished mix.
Use it to level speakers against each other, or to lift an underpowered
subwoofer:

```json
"audio": {
  "channel_aliases": { "lfe": 3, "surround_left": 4 },
  "channel_volumes": {
    "lfe": 1.6,
    "surround_left": 0.85,
    "7": 0.9
  }
}
```

Keys may be a channel number, a `channel_aliases` name, or a `channel_names`
label. Unity is 1.0, the maximum is 4.0 (+12 dB), and entries beyond the
device's channel count are ignored. Gain is applied after bass management, so
boosting the LFE channel raises the crossed-over bass along with anything
routed there directly. The mixer saturates its output, so an over-enthusiastic
boost clips rather than wrapping.

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
| `enabled` | boolean | `true` | Reserved; caching is currently always on |
| `revalidate_after_seconds` | integer | `300` | Reserved; cached files are never revalidated against the server (use `cache_invalidate`) |
| `precache` | array | `[]` | Files or directories to cache on startup |
| `precache_blocking` | boolean | `true` | Block startup until precache completes |
| `max_memory_mb` | integer | `512` | Maximum memory cache size in MB (0 = unlimited) |

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

#### Precache Blocking Mode

The `precache_blocking` option controls startup behavior:

- **`true` (default)**: App waits for all precache files to fully load before accepting MQTT commands. This ensures all sounds are instantly ready, but delays startup.
- **`false`**: App starts immediately and begins accepting commands while files load in the background. Playback of files still loading may start with a brief delay or silence until data is available.

```json
"cache": {
  "precache": ["/sounds/startup.wav"],
  "precache_blocking": false
}
```

**Note:** The MQTT `precache` command always operates in non-blocking mode, regardless of this setting. It queues the file for loading and returns immediately.

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
| `volume` | float | `1.0` | Input volume (0.0 to 4.0, unity is 1.0) |
| `voice_id` | string | — | Voice name for ducking integration |
| `routes` | array | *required* | Channel routing (source → dest, can use aliases) |
| `latency_ms` | integer | `20` | Buffer latency (5-500ms) |
| `channels` | integer | *auto* | Capture channels to open (1-64). Defaults to the smallest count that covers every `source_channel` |
| `sample_rate` | integer | *auto* | Capture rate to request (8000-192000). Defaults to the output rate, which avoids resampling |

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
| `primary_voice` | string | *required* | Voice that triggers ducking. Must be a playback voice; a live input's `voice_id` here never triggers (inputs have no activity detection) |
| `ducked_voices` | array | *required* | Voices to reduce in volume. May include live input `voice_id`s |
| `target_volume` | float | *required* | Volume to duck to (0.0 to 1.0) |
| `fade_duration_ms` | integer | *required* | Fade duration in milliseconds; restore uses the same duration |

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
- When non-empty, `play`/`precache` paths must resolve inside one of the
  listed directories (symlinks are followed; `../` traversal is blocked)
- If empty, local file access is unrestricted
- `~` expands to the user's home directory

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

### macros

Reusable parameter presets for commands. Useful for defining common channel mappings, volume levels, or other settings that you frequently use together.

```json
"macros": {
  "wholeroom": {
    "channel_map": [{"src": 0, "dest": "left"}, {"src": 1, "dest": "right"}],
    "volume": 0.2
  },
  "quiet": {
    "volume": 0.1
  },
  "music_defaults": {
    "voice": "music",
    "volume": 0.3,
    "fade_in": 2000
  }
}
```

Macros can be referenced in commands using the `macro` field. See [Commands](commands.md#macros) for usage details.

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
      "primary_voice": "narration",
      "ducked_voices": ["ambient", "effects", "gm_mic"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    }
  ],
  "security": {
    "allowed_directories": ["/opt/escaperoom/audio"]
  }
}
```

Prerecorded narration on the `narration` voice ducks the room ambience,
effects, and the gamemaster microphone. (A microphone cannot itself be a
`primary_voice` - live inputs have no activity detection - so to lower
playback while the gamemaster speaks, send `voice_volume` commands from the
show-control system.)

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

