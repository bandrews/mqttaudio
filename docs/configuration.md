# Configuration

mqttaudio can be configured via a JSON file, command-line arguments, or both. Command-line arguments override config file settings.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `MQTTAUDIO_CONFIG` | Path to the config file, used when `--config` is not given |
| `RUST_LOG` | Per-module log filtering (e.g. `mqttaudio=debug,mqttaudio::cache=trace`); overrides `logging.level` for console output |

## Command-Line Options

```
mqttaudio [OPTIONS]

OPTIONS:
  -c, --config <FILE>          Load configuration from JSON file
  -s, --server <HOST>          MQTT server hostname (config default: localhost)
  -p, --port <PORT>            MQTT server port (config default: 1883)
  -t, --topic <TOPIC>          MQTT topic to subscribe to
  --mqtt-username <USER>       MQTT broker username for authentication
  --mqtt-password <PASS>       MQTT broker password for authentication
  -d, --device <DEVICE>        Device ID from --list-devices (ALSA ID on Linux)
  -r, --sample-rate <RATE>     Output sample rate (config default: 48000)
  -n, --channels <COUNT>       Number of output channels (config default: auto-detect)
  --log-topic <TOPIC>          MQTT topic to publish log messages to
  --http-port <PORT>           Enable the HTTP REST/WebSocket server on this port
  --max-cache-mb <MB>          Override the memory cache cap in MiB (0 = auto-detect a bounded cap)
  -v, --verbose                Enable verbose logging (debug level)
  --configure                  Launch the interactive configuration editor and exit
  --list-devices               List output Device IDs with CLI and JSON examples
  --list-inputs                List available audio input devices and exit
  --help                       Print help information
  --version                    Print version information
```

The value-taking flags are optional overrides: when omitted they fall back to the config file, and the
"config default" shown is the built-in value applied when neither the flag nor the config sets it. These are
config-level defaults, not clap defaults, so `--help` does not display them. `--max-cache-mb 0` (and leaving
`cache.max_memory_mb` at `0`) selects an auto-detected **bounded** cap — not an unlimited cache.

## Interactive Editor

`mqttaudio --configure` opens a full-screen terminal editor covering every setting documented on this page,
with built-in guidance, the same validation the daemon applies at startup, and live device testing:

- A **help pane** at the top of the screen explains the selected section or setting as you move: what the
  feature does, what the values mean, and the allowed ranges. Lists such as ducking rules and live inputs
  describe themselves and summarize each entry (e.g. `"narration" ducks music → 20% over 500ms`).
- **Pickers instead of typing** wherever a setting references something defined elsewhere: channel fields
  offer your `channel_aliases` (plus plain channel numbers), voice fields offer the voice ids named
  elsewhere in the config (free text still allowed — voices are created at runtime), and enums list their
  options. Free-text entry remains one keystroke away for anything not listed.
- **Macros are edited as guided forms**: each macro opens as a list of its parameters, with a picker of the
  Play command's parameters (volume, voice, fade_in, loop, …) when adding one. Unknown parameters remain
  editable as raw JSON, so macros for other commands still work.
- The **output device picker** lists real devices and can play a test tone on any single output channel
  through the actual playback pathway — channel volumes, master gain, the limiter, and bass management from
  the in-progress config all apply, with live per-channel peak meters. A 50 Hz bass tone option makes an
  enabled bass-management crossover audible, and a sweep mode steps through all channels for speaker
  identification.
- The **input device picker** shows a live per-channel level meter through the daemon's real capture path.
- Saves are sparse: only settings you explicitly changed are written, so future default changes still reach
  your installation. Unset fields display their built-in default. An existing file is backed up to
  `<name>.bak` first, and unknown keys (such as `_comment` annotations) are preserved.
- `--configure --config <path>` edits (or creates) a specific file; without `--config` the daemon's normal
  search locations are used.

After saving, restart mqttaudio to apply the changes.

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
    "device": null,
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

### schema_version

Optional top-level integer naming the config schema version this file targets.

```json
"schema_version": 1
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema_version` | integer | absent (= current) | Config schema version. If it is **newer** than the running build understands, a one-time startup warning is logged that newer fields may be ignored; the config still loads and runs. |

Leave it out unless you are pinning a config to a specific schema; an absent value is treated as the current
version and never warns.

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
| `tls` | object | — | Opt-in TLS settings (see below). Absent = plain TCP. |

**Authentication:** If your MQTT broker requires authentication, provide both `username` and `password`. These can also be passed via command line with `--mqtt-username` and `--mqtt-password`.

**TLS (optional):** TLS is **opt-in** and never enabled implicitly — without a `tls` block the connection is plain TCP on *every* port, including 8883, so an existing plaintext broker keeps working unchanged. Add a `tls` block to encrypt the connection:

```json
"mqtt": {
  "server": "broker.example.com",
  "port": 8883,
  "topic": "audio/commands",
  "tls": {
    "ca_path": "/etc/mqttaudio/broker-ca.pem"
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ca_path` | string | — | PEM CA certificate to trust (for private/self-signed brokers). Omit to use the system root certificate store (public CAs). |

If you send a `username`/`password` to a non-loopback broker **without** TLS, mqttaudio logs a non-fatal warning that the credentials are travelling in cleartext.

### audio

Audio output settings. `device: null` uses the system default. For a specific
output, run `./mqttaudio --list-devices` and copy its **Device ID** into
`audio.device`, or use the printed `--device` option (which overrides the config).
On Linux, this is an ALSA ID such as `"plughw:CARD=HD,DEV=0"`, **not** the
description `"GIGAPort HD+, USB Audio"`. macOS and Windows generally use
human-readable device names. See [Finding Your Audio Device](getting-started.md#finding-your-audio-device)
for platform examples and ALSA prefix guidance.

```json
"audio": {
  "device": null,
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
| `device` | string or null | system default | Exact Device ID from `--list-devices`; Linux uses an ALSA ID. `null` selects the system default. |
| `sample_rate` | integer | `48000` | Output sample rate in Hz |
| `buffer_size` | integer | `512` | Buffer size in frames for output and capture streams (lower = less latency, more CPU) |
| `channels` | integer | auto-detect | Number of output channels |
| `channel_aliases` | object | `{}` | Named aliases for channel numbers |
| `channel_volumes` | object | `{}` | Per-output-channel calibration gain, `0.0`–`4.0`, keyed by channel number or alias |
| `output_ceiling_db` | number | `-1.0` | Limiter ceiling in dBFS (`-60.0`–`0.0`); the output peak is held at or below this level |
| `master_gain` | number | `1.0` | Linear gain applied to the whole bus before limiting (`0.0`–`8.0`) |

#### Channel Volumes (per-channel calibration)

`channel_volumes` applies a fixed gain to each output channel after mixing, useful for level-matching
speakers. Keys are channel numbers or `channel_aliases`; values are `0.0`–`1.0`. Channels not listed
play at unity. Entries that reference a channel beyond the output count, or an unknown alias, are ignored
with a warning.

```json
"audio": {
  "channel_aliases": { "front_left": 0, "front_right": 1 },
  "channel_volumes": { "front_left": 0.8, "1": 1.0 }
}
```

#### Output Limiter

The summed output bus passes through a soft-knee limiter so peaks never exceed `output_ceiling_db`
(default `-1.0` dBFS). Signal below the knee is unchanged; louder material is smoothly compressed toward
the ceiling rather than hard-clipped. `master_gain` is applied to the bus before limiting. The number of
samples the limiter held at the ceiling is reported as `clip_count` on `/status`.

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
| `enabled` | boolean | `true` | Disk-cache downloaded files. `false` = play from memory only, re-download after restart |
| `revalidate_after_seconds` | integer | `300` | Seconds a cached URL is served before checking the server for changes (0 = every access) |
| `precache` | array | `[]` | Files or directories to cache on startup |
| `precache_blocking` | boolean | `true` | Block startup until precache completes |
| `max_memory_mb` | integer | `0` | Simple memory-cache cap in MB. `0` (default) = auto-detect a bounded cap; a positive value is an explicit hard cap. Overridden by `memory_budget`. **(Changed: `0` no longer means unlimited.)** |
| `memory_budget` | object | *auto* | Advanced budget: `{"mode":"auto","fraction":0.4,"floor_mb":128,"ceiling_mb":1024}`, `{"mode":"explicit","mb":512}`, or `{"mode":"unlimited"}`. When present, overrides `max_memory_mb` |
| `load_mode` | string | `auto` | Default load strategy: `auto` (decide from size/duration + the budget), `full`, or `stream` |
| `full_load_max_bytes` | integer | `33554432` | `auto` threshold: a local asset whose estimated decoded size exceeds this is windowed (streamed) |
| `full_load_max_seconds` | integer | `60` | `auto` threshold: a local asset longer than this is windowed |
| `stream_window_ms` | integer | `1500` | Windowed-source ring depth in ms (bounds per-stream memory) |
| `stream_prebuffer_ms` | integer | `150` | Audio prebuffered before a windowed source starts playing |
| `stream_prebuffer_deadline_ms` | integer | `300` | Max wait for the prebuffer before starting anyway |
| `freshness` | string | `trusting` | `trusting` (serve cache, refresh remote in background), `dev` (re-check every load), or `pinned` (never auto-check) |
| `revalidate_after_seconds` | integer | `300` | Revalidate a remote (HTTP) cache entry once it is older than this |

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

#### Tuning first-start latency

Cold-play latency (command received → first audible sample) is dominated by a few
knobs. Measured shapes (x86 reference machine; Pi-class hardware is several times
slower, but the *shapes* hold — see `docs/sprints/sprint-12-first-start-latency.md`
for the measured tables):

- **Warm plays** (memory-cache hit) are effectively instant (~150 ns to a playable
  buffer) — `precache` anything that must fire on a cue.
- **Cold full-load plays** return a playable buffer in well under a millisecond and
  fill in the background; audio begins as soon as the first decoded chunk lands
  (typically a few ms for local files). This no longer scales with file length.
- **Windowed plays** start after `stream_prebuffer_ms` of audio is buffered, gated
  event-driven (no polling quantum). Lowering it starts sound sooner at higher
  underrun risk on slow storage/networks; `stream_prebuffer_deadline_ms` bounds the
  worst case (playback starts anyway at the deadline, with the underrun fade
  covering any gap). On a stable LAN, `stream_prebuffer_ms: 50` with a `150` ms
  deadline is a reasonable aggressive setting; keep the defaults for internet
  sources.
- **Uncached HTTP plays** pay one request (the windowing probe's connection is
  reused for the download); latency is network-dominated. Cacheable downloads are
  teed to the disk cache during playback, so the replay needs no network.
- **`audio.buffer_size`** sets the callback period — the floor on every start
  (~10.7 ms at 512 frames / 48 kHz). Smaller buffers cut latency at higher xrun
  risk on constrained hardware.
- `GET /metrics` reports `latency.play_to_first_mix_ns{last,max}` so these effects
  can be measured on the target hardware.

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

`source_channels` must not contain a duplicate channel, nor the `lfe_channel` itself (a number and an alias
that resolve to the same channel count as a duplicate); either is a configuration error naming the offending
channel. Routing other content (an input route or a Play `channel_map`) **to** the `lfe_channel` is allowed
but bypasses the crossover — that content reaches the sub full-range and the extracted bass is added on top
(a one-time startup warning flags a configured input route that does this).

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
| `device` | string | *required* | Input device name or `--list-inputs` index (e.g. `"0"`) |
| `volume` | float | `1.0` | Input volume (0.0 to 4.0, unity is 1.0) |
| `voice_id` | string | — | Voice name for ducking integration |
| `routes` | array | *required* | Channel routing (source → dest, can use aliases) |
| `latency_ms` | integer | `20` | Buffer latency (5-500ms). Raised with a warning when the ring cannot hold one resampler chunk (11 ms at 48 kHz) |
| `activity_threshold` | float | `null` | Peak level (0.0-1.0) above which this input counts as speaking for ducking rules; `null` disables activity detection |
| `activity_hold_ms` | integer | `750` | How long activity persists after the level drops (0-10000ms) |
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
| `primary_voice` | string | *required* | Voice that triggers ducking. A live input's `voice_id` works here when the input has an `activity_threshold` configured |
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
  "format": "text",
  "mqtt_topic": "audio/logs"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `level` | string | `"info"` | Log level: `error`, `warn`, `info`, `debug`, `trace` |
| `format` | string | `"text"` | Console log format: `text` (human-readable) or `json` (line-delimited JSON for log aggregation) |
| `mqtt_topic` | string | — | Publish logs to this MQTT topic |

Setting `format` to `json` emits one JSON object per line on the console, which `journalctl`, Loki, or the ELK
stack can parse directly. The `mqtt_topic` sink (when set) publishes alongside whichever console format is
chosen.

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
| `resampler_quality` | string | `"fast"` | Sample rate conversion quality preset, used for file decoding and live-input capture alike |

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
    "device": null
  },
  "inputs": [
    {
      "device": "Gamemaster Headset",
      "volume": 0.9,
      "voice_id": "gm_mic",
      "activity_threshold": 0.05,
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

When the gamemaster speaks (capture level above `activity_threshold`), the
room ambience and effects duck to 10% and recover 750 ms after the mic goes
quiet.

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

