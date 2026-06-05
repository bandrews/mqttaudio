# Command Reference

All commands are JSON messages published to the configured MQTT topic or sent via HTTP.

## Command Format

```json
{
  "command": "command_name",
  "param1": "value1",
  "param2": "value2"
}
```

---

## Playback Commands

### play

Play an audio file.

```json
{
  "command": "play",
  "file": "/path/to/sound.wav",
  "id": "my-sound-id",
  "voice": "effects",
  "volume": 0.8,
  "loop": false,
  "fade_in": 1000,
  "start_position_ms": 0,
  "channel_map": [
    {"src": 0, "dest": 2},
    {"src": 1, "dest": 3}
  ]
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `file` | string | *required* | File path or HTTP/HTTPS URL |
| `id` | string | auto | Unique ID for targeting this sound later |
| `voice` | string | auto | Voice group name |
| `volume` | float | 1.0 | Volume (0.0 to 1.0) |
| `loop` | boolean | false | Loop playback continuously |
| `crossfade_ms` | integer | 0 | Crossfade duration at loop boundaries (0 = disabled) |
| `fade_in` | integer | 0 | Fade-in duration (milliseconds) |
| `start_position_ms` | integer | 0 | Start position (milliseconds) |
| `channel_map` | array | auto | Channel routing (see below) |
| `mode` | string | `auto` | Load strategy: `auto` (decide by size/duration + memory budget), `full` (always in-memory), or `stream` (window a big/long file). Applies to local files and `http(s)://` URLs. A windowed voice plays forward only |
| `window_ms` | integer | config | Windowed-source ring depth override (streamed plays) |
| `prebuffer_ms` | integer | config | Windowed-source prebuffer override (streamed plays) |
| `freshness` | string | config | Cache freshness override: `trusting`, `dev`, or `pinned` |
| `cacheable` | boolean | `true` | HTTP windowed plays only: `true` (default) tees the download to the disk cache so a replay hits disk; `false` treats the source as live (window, never persist). A URL with no `Content-Length` is always live |

**Channel Mapping:**

Without `channel_map`, audio plays on sequential channels starting from 0. With `channel_map`, you specify exactly where each source channel goes:

```json
{
  "command": "play",
  "file": "/sound.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5}
  ]
}
```

You can use channel aliases (defined in config) instead of numbers:

```json
{
  "command": "play",
  "file": "/sound.wav",
  "channel_map": [
    {"src": 0, "dest": "front_left"},
    {"src": 1, "dest": "front_right"}
  ]
}
```

You can route one source to multiple destinations:

```json
{
  "command": "play",
  "file": "/mono.wav",
  "channel_map": [
    {"src": 0, "dest": 0},
    {"src": 0, "dest": 1},
    {"src": 0, "dest": 2}
  ]
}
```

Each route accepts an optional `gain` (default `1.0`). When several source channels are routed to the same
destination they sum, which can clip; a per-route `gain` lets you attenuate (or boost) each route. A route
without `gain` is unity, so existing maps are unaffected:

```json
{
  "command": "play",
  "file": "/quad.wav",
  "channel_map": [
    {"src": 0, "dest": 0, "gain": 0.5},
    {"src": 2, "dest": 0, "gain": 0.5},
    {"src": 1, "dest": 1, "gain": 0.5},
    {"src": 3, "dest": 1, "gain": 0.5}
  ]
}
```

### stopall

Stop all playing audio immediately.

```json
{"command": "stopall"}
```

---

## Sample Targeting Commands

These commands control specific playing samples by ID, filename, or voice.

### stop

Stop specific samples.

```json
{
  "command": "stop",
  "id": "my-sound-id",
  "fade_out_ms": 500
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id` | string | — | Target the exact sample by its system-assigned internal ID (from `/status/samples`); checked first |
| `id` | string | — | Stop sample with this ID |
| `file` | string | — | Stop all samples playing this file |
| `voice` | string | — | Stop all samples in this voice |
| `fade_out_ms` | integer | 0 | Fade-out duration (milliseconds) |

At least one of `internal_id`, `id`, `file`, or `voice` is required. Multiple selectors use OR logic (a sample matches if any one criterion matches). An empty selector matches nothing and is silently a no-op.

### seek

Jump to a position in a playing sample.

```json
{
  "command": "seek",
  "id": "my-sound-id",
  "position_ms": 60000
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id` | string | — | Target the exact sample by its system-assigned internal ID (from `/status/samples`); checked first |
| `id` | string | — | Target sample ID |
| `file` | string | — | Target all samples playing this file |
| `voice` | string | — | Target all samples in this voice |
| `position_ms` | integer | *required* | Position to seek to (milliseconds) |

### speed

Change playback speed.

```json
{
  "command": "speed",
  "id": "my-sound-id",
  "speed": 1.5,
  "pitch_correction": false
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id` | string | — | Target the exact sample by its system-assigned internal ID (from `/status/samples`); checked first |
| `id` | string | — | Target sample ID |
| `file` | string | — | Target samples playing this file |
| `voice` | string | — | Target samples in this voice |
| `speed` | float | *required* | Playback speed multiplier |
| `pitch_correction` | boolean | false | Maintain original pitch |

**Speed ranges:**
- Without pitch correction: -100.0 to 100.0 (negative = reverse)
- With pitch correction: 0.05 to 8.0 (reverse not supported)

### volume

Adjust volume of specific samples.

```json
{
  "command": "volume",
  "id": "my-sound-id",
  "volume": 0.5
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id` | string | — | Target the exact sample by its system-assigned internal ID (from `/status/samples`); checked first |
| `id` | string | — | Target sample ID |
| `file` | string | — | Target samples playing this file |
| `voice` | string | — | Target samples in this voice |
| `volume` | float | *required* | New volume (0.0 to 1.0) |

---

## Voice Commands

### voice_stop

Stop all samples in a voice immediately.

```json
{
  "command": "voice_stop",
  "voice": "background"
}
```

### voice_fade_out

Fade out all samples in a voice.

```json
{
  "command": "voice_fade_out",
  "voice": "background",
  "time": 2000
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `voice` | string | *required* | Voice name |
| `time` | integer | *required* | Fade duration (milliseconds) |

### voice_volume

Adjust volume for all samples in a voice.

```json
{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.5
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `voice` | string | *required* | Voice name |
| `volume` | float | *required* | New volume (0.0 to 1.0) |

---

## Cache Commands

### precache

Download and decode a file without playing it.

```json
{
  "command": "precache",
  "file": "https://example.com/large-file.wav"
}
```

Use this to eliminate first-play latency for files you'll need later.

### cache_clear

Clear the entire cache (memory and disk).

```json
{"command": "cache_clear"}
```

### cache_invalidate

Remove a specific file from cache.

```json
{
  "command": "cache_invalidate",
  "file": "https://example.com/updated-file.wav"
}
```

### cache_reload

Invalidate a cached entry and immediately re-precache it, so the next play is both fresh and instant. Useful
after a content pipeline republishes an asset.

```json
{
  "command": "cache_reload",
  "file": "/sounds/updated-cue.wav"
}
```

---

## Input Commands

### input_volume

Adjust volume for a microphone/input device.

```json
{
  "command": "input_volume",
  "input": "gamemaster_mic",
  "volume": 0.5
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `input` | string | *required* | Input name (voice_id) or index |
| `volume` | float | *required* | Volume (0.0 to 1.0) |

### input_mute

Mute or unmute an input.

```json
{
  "command": "input_mute",
  "input": "0",
  "mute": true
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `input` | string | *required* | Input name or index |
| `mute` | boolean | *required* | true = mute, false = unmute |

---

## Examples

### Complete Session

```bash
TOPIC="audio/commands"

# Precache files for instant playback
mosquitto_pub -t $TOPIC -m '{"command": "precache", "file": "/sounds/music.mp3"}'
mosquitto_pub -t $TOPIC -m '{"command": "precache", "file": "/sounds/narration.wav"}'

# Start background music with crossfade for smooth looping
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/music.mp3",
  "voice": "music",
  "volume": 0.3,
  "loop": true,
  "crossfade_ms": 100,
  "fade_in": 2000
}'

# Play a sound effect
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/doorbell.wav",
  "voice": "effects"
}'

# Duck music and play narration
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.1
}'

mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/narration.wav",
  "voice": "narration"
}'

# Restore music (after narration finishes)
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.3
}'

# Fade out music
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_fade_out",
  "voice": "music",
  "time": 5000
}'

# Stop all audio
mosquitto_pub -t $TOPIC -m '{"command": "stopall"}'
```

### Multichannel Surround

```bash
# Play 4-channel audio to rear speakers (channels 4-7)
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/quad-ambience.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5},
    {"src": 2, "dest": 6},
    {"src": 3, "dest": 7}
  ]
}'
```

### Variable Speed Playback

```bash
# Play at double speed (chipmunk effect)
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/speech.wav",
  "id": "speech-1"
}'

mosquitto_pub -t $TOPIC -m '{
  "command": "speed",
  "id": "speech-1",
  "speed": 2.0
}'

# Slow down to half speed with pitch correction
mosquitto_pub -t $TOPIC -m '{
  "command": "speed",
  "id": "speech-1",
  "speed": 0.5,
  "pitch_correction": true
}'
```

---

## Macros

Commands can reference macros defined in the config file to apply preset parameters.

```json
{
  "command": "play",
  "file": "/sounds/music.mp3",
  "macro": "wholeroom"
}
```

Parameters specified in the command take precedence over macro values. Multiple macros can be specified as an array, with earlier macros taking precedence over later ones:

```json
{
  "command": "play",
  "file": "/sounds/music.mp3",
  "macro": ["quiet", "wholeroom"],
  "voice": "background"
}
```

In this example: `voice` comes from the command, `volume` from the "quiet" macro, and `channel_map` from "wholeroom".

See [Configuration](configuration.md#macros) for defining macros.

---

## Legacy Format

For backward compatibility, commands also accept parameters wrapped in a `message` object:

```json
{
  "command": "play",
  "message": {
    "file": "/path/to/sound.wav",
    "volume": 0.8
  }
}
```

This format is equivalent to the flattened format shown throughout this document. When both formats are present in the same message, the `message` object takes precedence (its contents replace the flattened keys wholesale rather than merging).

### Legacy command names

Three commands also accept a legacy alias for their `command` value:

| Canonical | Legacy alias |
|-----------|--------------|
| `play` | `soundPlay` |
| `stopall` | `soundStopAll` |
| `precache` | `soundPrecache` |

These aliases are accepted only for those three commands; every other command uses its canonical name. Command names are matched case-sensitively.
