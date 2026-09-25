# Configuration

mqttaudio reads one JSON file at startup. Every setting has a default, so a file only needs the
settings you change; the smallest useful one names an MQTT topic:

```json
{"mqtt": {"topic": "audio/commands"}}
```

[`config.example.json`](../config.example.json) is a fuller starting point, and
`mqttaudio --configure` edits the file interactively (see [Interactive editor](#interactive-editor)).

## How it is loaded

1. **The file.** `--config <path>` (or the `MQTTAUDIO_CONFIG` environment variable) names it.
   Otherwise the first of these that exists is used:
   1. `./mqttaudio.json`
   2. the user config directory: `~/.config/mqttaudio/config.json` on Linux,
      `~/Library/Application Support/mqttaudio/config.json` on macOS,
      `%APPDATA%\mqttaudio\config.json` on Windows
   3. `/etc/mqttaudio/config.json`

   With no file, the defaults apply. The startup log names the file it loaded.
2. **Command-line options** override the file.
3. **Environment variables** for the HTTP server override both.

The result is checked before the audio device opens. Every problem is printed and the daemon
exits, so fix them all at once. Keys the daemon does not know are ignored, which lets you keep notes
in keys like `"_comment"` but also means a misspelled key is silently ignored. Changes take effect on
the next start.

## Command line

```
mqttaudio [OPTIONS]
```

| Option | Sets |
|--------|------|
| `-c, --config <FILE>` | Config file to load |
| `-s, --server <HOST>` | `mqtt.server` |
| `-p, --port <PORT>` | `mqtt.port` |
| `-t, --topic <TOPIC>` | `mqtt.topic` |
| `--mqtt-username <USER>` | `mqtt.username` |
| `--mqtt-password <PASS>` | `mqtt.password` (visible to other local users in the process list; prefer the file) |
| `-d, --device <DEVICE>` | `audio.device` |
| `-r, --sample-rate <RATE>` | `audio.sample_rate` |
| `-n, --channels <COUNT>` | `audio.channels` |
| `--http-port <PORT>` | `http.port`, and turns the HTTP server on |
| `--max-cache-mb <MB>` | `cache.max_memory_mb` (no effect when `cache.memory_budget` is set) |
| `--log-topic <TOPIC>` | `logging.mqtt_topic` |
| `-v, --verbose` | `logging.verbose` |
| `--configure` | Open the interactive editor instead of starting |
| `--list-devices` | List output devices, with ready-to-use `--device` and config lines, and exit |
| `--list-inputs` | List input devices and exit |
| `--check-ready` | Ask the daemon this configuration describes for `GET /ready`, then exit: `0` when it is ready or the HTTP server is off, `1` otherwise. For health checks; needs a fixed `http.port` |
| `-h, --help` / `-V, --version` | Help / version |

`--list-devices`, `--list-inputs` and `--check-ready` still read the config file, so a file that is
not valid JSON stops them too.

## Environment variables

| Variable | Effect |
|----------|--------|
| `MQTTAUDIO_CONFIG` | Config file to load when `--config` is not given |
| `MQTTAUDIO_HTTP_AUTH_TOKEN` | Sets `http.auth_token` |
| `MQTTAUDIO_HTTP_REQUIRE_AUTH` | `true` (any case) sets `http.require_auth` and turns the HTTP server on |
| `MQTTAUDIO_HTTP_BIND_ADDRESS` | Sets `http.bind_address` |
| `RUST_LOG` | Replaces `logging.level` for console output with a per-module filter, such as `mqttaudio=info,mqttaudio::cache=debug` |

These keep secrets out of a checked-in config; see [Deployment](deployment.md).

## Settings

### mqtt

```json
"mqtt": {
  "server": "broker.local",
  "port": 8883,
  "topic": "audio/commands",
  "username": "mqttaudio",
  "password": "secret",
  "tls": {"ca_path": "/etc/mqttaudio/broker-ca.pem"}
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `server` | `"localhost"` | Broker host name or address |
| `port` | `1883` | Broker port |
| `topic` | none | Topic to take commands from; `#` and `+` wildcards work. Required unless `http.enabled` is on |
| `client_id` | random `mqttaudio_<8 hex digits>` | MQTT client id. Give each daemon on a broker its own |
| `reconnect_delay_seconds` | `10` | Wait between reconnection attempts (at least 1) |
| `username`, `password` | none | Broker login. Both are needed; a username alone is not sent |
| `tls` | none | Presence turns on TLS; see below |

The daemon keeps retrying an unreachable broker, and subscribes again after every reconnect.

**TLS** is off unless a `tls` section is present, whatever the port. `"tls": {}` verifies the broker
against the system's trusted certificate authorities; `ca_path` names a PEM certificate to trust
instead, for a private or self-signed broker. A warning is logged when credentials go to a broker
that is not on this machine without TLS.

### audio

```json
"audio": {
  "device": "plughw:CARD=UMC1820,DEV=0",
  "sample_rate": 48000,
  "channels": 8,
  "buffer_size": 512,
  "channel_aliases": {"front_left": 0, "front_right": 1, "center": 2, "lfe": 3},
  "channel_volumes": {"lfe": 1.5, "7": 0.8},
  "output_ceiling_db": -1.0,
  "master_gain": 1.0,
  "max_sounds": 256,
  "max_streamed_sounds": 64
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `device` | system default | Output device, as printed by `--list-devices`. On Linux that is an ALSA name such as `plughw:CARD=UMC1820,DEV=0`, not the card's description |
| `sample_rate` | `48000` | Output rate, `8000`–`192000` Hz. A rate the device lacks is replaced by the nearest one it has; the log shows the rate in use |
| `channels` | the most the device offers, up to 32 | Output channel count. A count the device lacks is raised to the next one it has. ALSA plugin devices such as `plughw:` and `default` accept any count, so without this they open 32 channels: set it to the card's real count |
| `buffer_size` | `512` | Frames per audio block, `64`–`8192`; used when the device accepts it, otherwise the device's own size. Smaller blocks lower latency and raise the risk of dropouts |
| `channel_aliases` | `{}` | Names for output channels, usable wherever a channel number is |
| `channel_names` | `{}` | Display labels, keyed by channel number: `{"6": "booth"}`. Accepted as `channel_volumes` keys, but not as channel names in routes |
| `channel_volumes` | `{}` | Per-output calibration gain, `0.0`–`4.0` |
| `output_ceiling_db` | `-1.0` | Output limiter ceiling in dBFS, `-60.0`–`0.0` |
| `master_gain` | `1.0` | Gain on the whole mix before the limiter, `0.0`–`8.0` |
| `max_sounds` | `256` | Most fully loaded sounds playing at once, `1`–`4096`. At the limit a new play replaces the oldest sound that is not looping (logged), or fails when every sound loops |
| `max_streamed_sounds` | `64` | Most windowed (streamed) sounds playing at once, `1`–`1024`. Each has its own decode thread. A windowed play over the limit fails |

**Channel aliases** let configs and commands say `"lfe"` instead of `3`: in play `channel_map`s,
input routes, bass management and `channel_volumes`. An unknown name is an error. If the name you
used is a `channel_names` label, the error says which alias to add.

**Channel volumes** level speakers against each other after mixing, bass management included, so
boosting the LFE channel raises both the crossed-over bass and anything routed there directly. Keys
are channel numbers, aliases or `channel_names` labels; an unknown key is an error, and a channel
beyond the device's count is ignored. Unlisted channels stay at `1.0`.

**The limiter** keeps the output peak at or below `output_ceiling_db`. Below 95% of the ceiling the
signal passes unchanged; above that it is compressed smoothly into the ceiling. `clip_count` on
`GET /status` counts samples that reached it.

### cache

```json
"cache": {
  "directory": "/var/lib/mqttaudio/cache",
  "precache": ["/opt/sounds/cues", "https://example.com/sounds/intro.mp3"],
  "precache_blocking": true
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `enabled` | `true` | Keep downloaded files in `directory` so replays and restarts skip the download |
| `directory` | `~/.mqttaudio/cache` | Disk cache location (`~` expands). While `enabled` is on it is created at startup if missing, and startup fails if it cannot be; an existing directory that is not writable only shows up as errors when the cache writes |
| `precache` | `[]` | Files, directories (their `.wav`, `.mp3`, `.ogg` and `.flac` files, not subdirectories) and URLs to load at startup |
| `precache_blocking` | `true` | `true`: finish loading each precache entry before starting. `false`: start loading each entry, then begin taking commands while they finish |
| `max_memory_mb` | `0` | Memory cache budget in MiB. `0` sizes it automatically: 40% of available memory, at least 128 MiB and at most 1024 MiB |
| `memory_budget` | none | Replaces `max_memory_mb` when present; see below |
| `load_mode` | `"auto"` | How plays load files: `auto`, `full` or `stream` (see [Caching](features/caching.md#full-and-windowed-plays)) |
| `full_load_max_bytes` | `33554432` (32 MiB) | `auto` windows a file whose decoded audio would be larger |
| `full_load_max_seconds` | `60` | `auto` windows a local file longer than this |
| `stream_window_ms` | `1500` | Buffer length of a windowed play, `100`–`60000` |
| `stream_prebuffer_ms` | `150` | Audio buffered before a windowed play starts; at most `stream_window_ms` |
| `stream_prebuffer_deadline_ms` | `300` | Longest a play waits for its first audio. A windowed play then starts anyway; a full play fails if nothing has decoded. At least `stream_prebuffer_ms` |
| `freshness` | `"trusting"` | How changed files are noticed: `trusting`, `dev` or `pinned` (see [Caching](features/caching.md#freshness)) |
| `revalidate_after_seconds` | `300` | How long a downloaded file is used before asking the server whether it changed |

`memory_budget` takes one of three forms:

```json
"memory_budget": {"mode": "auto", "fraction": 0.4, "floor_mb": 128, "ceiling_mb": 1024}
"memory_budget": {"mode": "explicit", "mb": 512}
"memory_budget": {"mode": "unlimited"}
```

`auto` fields are optional and default to the values shown; if available memory cannot be read, the
ceiling is used. `unlimited` removes the budget, so one long file can use all memory.
[Caching](features/caching.md) explains how the budget, windowing and freshness work together.

### security

```json
"security": {"allowed_directories": ["/opt/sounds", "~/show-audio"]}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `allowed_directories` | `[]` | When not empty, a local file plays or precaches only if it resolves (following symlinks and `..`) to a path inside one of these directories |

An empty list allows any local file the daemon can read. URLs are never restricted. A path outside
the list fails with HTTP `403`.

### logging

```json
"logging": {"level": "info", "format": "json", "mqtt_topic": "audio/logs"}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `level` | `"info"` | `error`, `warn`, `info`, `debug` or `trace` |
| `verbose` | `false` | Raise `level` to at least `debug`, and log cache statistics after cache operations |
| `format` | `"text"` | Console format: `text`, or `json` for one JSON object per line |
| `mqtt_topic` | none | Also publish each log line to this MQTT topic |

Logs go to standard output. Records published to `mqtt_topic` look like this:

```json
{"timestamp": "2026-09-23T12:00:00.123456+00:00", "level": "INFO", "target": "mqttaudio", "message": "Cache cleared successfully"}
```

### http

```json
"http": {
  "enabled": true,
  "port": 8080,
  "bind_address": "127.0.0.1",
  "auth_token": "a-long-random-token",
  "require_auth": false,
  "websocket_enabled": true,
  "cors_permissive": false
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `enabled` | `false` | Serve the [HTTP API](http-api.md) |
| `port` | `0` | Port; `0` picks a free one (the log shows which) |
| `bind_address` | `"127.0.0.1"` | IP address to listen on; `0.0.0.0` listens on every interface. A host name is an error |
| `auth_token` | none | Bearer token, at least 8 characters, required for commands and WebSockets |
| `require_auth` | `false` | Also require the token for status routes. Needs a token |
| `websocket_enabled` | `true` | Serve `/ws` and `/ws/state` |
| `cors_permissive` | `false` | Allow browser pages from any origin to call the API |

A warning is logged when the server listens on a non-loopback address without a token.

### inputs

Live inputs mix a capture device (a microphone, a line input) into the output. Each entry opens one
input:

```json
"inputs": [
  {
    "device": "hw:CARD=UMC1820,DEV=0",
    "voice_id": "gm_mic",
    "volume": 0.9,
    "routes": [
      {"source_channel": 0, "dest_channel": "front_left"},
      {"source_channel": 0, "dest_channel": "front_right"}
    ],
    "latency_ms": 20,
    "activity_threshold": 0.05
  }
]
```

| Setting | Default | Description |
|---------|---------|-------------|
| `device` | system default input | Capture device: its name from `--list-inputs`, or its number there as a string (`"0"`) |
| `routes` | *required* | Which capture channel plays on which output channel. `dest_channel` may be an alias |
| `voice_id` | `"mic"` | Voice the input plays in, for `voice_volume`, `input_*` commands and ducking rules |
| `volume` | `1.0` | Input volume, `0.0`–`4.0` |
| `latency_ms` | `20` | Capture buffering, `5`–`500`; the microphone-to-speaker delay is roughly twice this plus about 11 ms. Raised, with a warning, to the smallest value that works (11 ms at 48 kHz) |
| `channels` | smallest count that covers the routes | Capture channel count to open, `1`–`64`; must be one the device offers |
| `sample_rate` | the output rate | Capture rate to ask for, `8000`–`192000` |
| `activity_threshold` | none | Peak level (above `0.0`, up to `1.0`) at which the input counts as active for ducking rules. Without it the input is active whenever it is open and not muted. A muted input is never active |
| `activity_hold_ms` | `750` | How long the input stays active after the level drops, up to `10000` |

Inputs that share a device and settings share one capture stream. An input that fails to open is
reported by `GET /ready` and `/status/inputs`, and the daemon keeps running without it.
[Microphone Input](features/microphone-input.md) covers setup and tuning.

### ducking_rules

```json
"ducking_rules": [
  {
    "primary_voice": "narration",
    "ducked_voices": ["music", "ambience"],
    "target_volume": 0.2,
    "fade_duration_ms": 500
  }
]
```

| Setting | Description |
|---------|-------------|
| `primary_voice` | While this voice plays, the rule applies. A live input's `voice_id` works too |
| `ducked_voices` | Voices to turn down; must not be empty. Live inputs' voices work too |
| `target_volume` | Level to turn them down to, `0.0`–`1.0` |
| `fade_duration_ms` | Fade length, up to `60000` |

All four are required. [Ducking](features/ducking.md) explains overlapping rules and timing.

### bass_management

```json
"bass_management": {
  "enabled": true,
  "lfe_channel": "lfe",
  "source_channels": ["front_left", "front_right", "center", "rear_left", "rear_right"],
  "crossover_frequency_hz": 80,
  "remove_bass_from_sources": true,
  "lfe_gain": 1.0
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `enabled` | `false` | Send the bass of `source_channels` to `lfe_channel` |
| `lfe_channel` | `3` | Subwoofer output (number or alias) |
| `source_channels` | `[]` | Channels to take bass from; required when enabled. No duplicates, and not the LFE channel |
| `crossover_frequency_hz` | `80` | Crossover frequency, `10`–`200` Hz |
| `remove_bass_from_sources` | `true` | Also filter the bass out of the source channels, so it plays only from the subwoofer |
| `lfe_gain` | `1.0` | Gain on the combined bass, `0.0`–`8.0` |

The checks run only while `enabled` is on. See [Bass Management](features/bass-management.md).

### macros

Named sets of command parameters that commands merge in with a `macro` field
([Commands: Macros](commands.md#macros)):

```json
"macros": {
  "wholeroom": {"channel_map": [{"src": 0, "dest": 0}, {"src": 1, "dest": 1}, {"src": 0, "dest": 4}, {"src": 1, "dest": 5}]},
  "quiet": {"volume": 0.2},
  "music": {"voice": "music", "loop": true, "fade_in": 2000}
}
```

### advanced

| Setting | Default | Description |
|---------|---------|-------------|
| `resampler_quality` | `"fast"` | Quality of sample-rate conversion for files and live inputs: `fast`, `medium`, `high` or `maximum` |

A file whose rate differs from the output's is converted as it loads. Higher settings sound cleaner
and take longer; for one minute of 44.1 kHz stereo, roughly 60 ms (`fast`), 95 ms (`medium`), 190 ms
(`high`) and 230 ms (`maximum`) on a desktop machine, several times that on a Raspberry Pi. It makes
no difference to files already at the output rate. With a higher setting, precache files that must
start instantly.

### schema_version

An optional top-level integer. A value newer than this build understands (currently `1`) logs a
warning that some settings may be ignored; the config still loads. Leave it out normally.

## Interactive editor

`mqttaudio --configure` opens a terminal editor for the whole file. Add `--config <path>` to edit or
create a particular file; otherwise it opens the file the daemon would load.

- A help pane explains the selected section or setting, its range and default.
- Settings that refer to something else are picked from lists: channels from your aliases, voices
  from those named elsewhere in the config, and fixed choices from their options.
- The output device picker lists real devices and plays a 440 Hz test tone or a 50 Hz bass tone on
  any channel, or the test tone on every channel in turn, through the real signal path (channel
  volumes, master gain, limiter and bass management from the file being edited), with meters. The
  input picker shows live levels.
- Saving checks the file with the same rules as startup, except that command-line options and
  environment variables are not applied. It writes only the settings you have set, keeps settings
  it does not know, copies the previous file to `<file>.bak` (replacing an older backup), and keeps
  the file's permission bits. The saved file belongs to whoever ran the editor, so restore the group
  of a service's config afterwards (see [Deployment](deployment.md#run-under-systemd)). Keys come out
  in alphabetical order.

Restart mqttaudio to apply the saved file.

## Examples

**Stereo with a sound folder**

```json
{
  "mqtt": {"server": "broker.local", "topic": "gallery/audio"},
  "cache": {"precache": ["/opt/sounds"]},
  "security": {"allowed_directories": ["/opt/sounds"]}
}
```

**HTTP only, no broker**

```json
{"http": {"enabled": true, "port": 8080, "auth_token": "a-long-random-token"}}
```

**Escape room: game master microphone ducks the room**

```json
{
  "mqtt": {"topic": "escaperoom/audio"},
  "audio": {"channels": 8},
  "inputs": [
    {
      "device": "plughw:CARD=Headset,DEV=0",
      "voice_id": "gm_mic",
      "volume": 0.9,
      "activity_threshold": 0.05,
      "routes": [
        {"source_channel": 0, "dest_channel": 4},
        {"source_channel": 0, "dest_channel": 5},
        {"source_channel": 0, "dest_channel": 6},
        {"source_channel": 0, "dest_channel": 7}
      ]
    }
  ],
  "ducking_rules": [
    {"primary_voice": "gm_mic", "ducked_voices": ["ambience", "effects"], "target_volume": 0.1, "fade_duration_ms": 500}
  ],
  "security": {"allowed_directories": ["/opt/escaperoom/audio"]}
}
```

While the game master speaks (level above `0.05`), ambience and effects fall to 10%; they recover
750 ms after the microphone goes quiet.

**5.1 with a subwoofer**

```json
{
  "mqtt": {"topic": "theater/audio"},
  "audio": {
    "channels": 6,
    "channel_aliases": {"front_left": 0, "front_right": 1, "center": 2, "lfe": 3, "rear_left": 4, "rear_right": 5}
  },
  "bass_management": {
    "enabled": true,
    "lfe_channel": "lfe",
    "source_channels": ["front_left", "front_right", "center", "rear_left", "rear_right"]
  }
}
```
