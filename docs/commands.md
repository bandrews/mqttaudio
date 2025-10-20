# MQTT Command Reference

All commands are JSON objects published to the configured MQTT topic.

## Command Structure

```json
{
  "command": "command_name",
  "message": {
    // Command-specific parameters
  }
}
```

## Play Command

Play an audio file with optional multichannel routing and voice grouping.

```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/audio.wav",
    "voice": "ambience",
    "channel_map": [
      {"src": 0, "dest": 6},
      {"src": 1, "dest": 7}
    ],
    "volume": 0.8,
    "loop": false,
    "fade_in": 1000,
    "max_play_length": 30000
  }
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `file` | string | Yes | - | URL (http/https) or local file path |
| `voice` | string | No | Auto-generated | Voice name for grouping. If not specified, creates anonymous voice |
| `channel_map` | array | No | Auto (see below) | Channel routing configuration |
| `volume` | float | No | 1.0 | Sample volume (0.0 - 1.0) |
| `loop` | boolean | No | false | Whether to loop playback |
| `fade_in` | integer | No | 0 | Fade in duration (milliseconds) |
| `max_play_length` | integer | No | null | Maximum playback duration (milliseconds), -1 or null = unlimited |

### Channel Mapping

Channel map is an array of source-to-destination mappings:

```json
"channel_map": [
  {"src": 0, "dest": "front_left"},   // Can use named channels
  {"src": 1, "dest": 6},              // Or channel numbers
  {"src": 2, "dest": "subwoofer"}
]
```

**Auto-mapping (when channel_map not specified):**
- Mono file → Output channel 0
- Stereo file → Output channels 0, 1
- Multi-channel → Output channels 0, 1, 2, 3... (sequential)

**One-to-many routing:**
You can map a single source channel to multiple output channels by specifying multiple entries with the same `src` value:
```json
"channel_map": [
  {"src": 0, "dest": 0},  // Map source channel 0 to output 0
  {"src": 0, "dest": 1},  // AND to output 1
  {"src": 0, "dest": 2}   // AND to output 2
]
```
This is useful for PA announcements or broadcasting a mono signal to multiple zones.

**Channel names** are defined in config file (see configuration.md).

### Examples

**Simple stereo playback:**
```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/music.mp3",
    "volume": 0.7,
    "loop": true
  }
}
```

**4-channel surround to specific outputs:**
```json
{
  "command": "play",
  "message": {
    "file": "/opt/sounds/ambience.wav",
    "voice": "background",
    "channel_map": [
      {"src": 0, "dest": "front_left"},
      {"src": 1, "dest": "front_right"},
      {"src": 2, "dest": "rear_left"},
      {"src": 3, "dest": "rear_right"}
    ],
    "volume": 0.5,
    "loop": true
  }
}
```

**One-shot sound effect with fade in:**
```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/fx/thunder.ogg",
    "voice": "effects",
    "volume": 1.0,
    "fade_in": 500
  }
}
```

**Timed playback (30 seconds max):**
```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/preview.wav",
    "max_play_length": 30000
  }
}
```

## Stop All Command

Stop all currently playing audio immediately.

```json
{
  "command": "stopall"
}
```

No parameters. Stops all voices and all samples instantly.

## Voice Stop Command

Stop all samples in a specific voice immediately.

```json
{
  "command": "voice_stop",
  "message": {
    "voice": "ambience"
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `voice` | string | Yes | Name of voice to stop |

## Voice Fade Out Command

Fade out all samples in a voice over a specified duration.

```json
{
  "command": "voice_fade_out",
  "message": {
    "voice": "ambience",
    "time": 2000
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `voice` | string | Yes | Name of voice to fade out |
| `time` | integer | Yes | Fade out duration (milliseconds) |

After the fade completes, samples are automatically removed.

## Voice Volume Command

Adjust the volume of all samples in a voice.

```json
{
  "command": "voice_volume",
  "message": {
    "voice": "music",
    "volume": 0.5
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `voice` | string | Yes | Name of voice to adjust |
| `volume` | float | Yes | New volume level (0.0 - 1.0) |

This affects all currently playing samples in the voice and any future samples added to this voice.

## Global Fade Out Command

Fade out all audio (all voices) over a specified duration.

```json
{
  "command": "fadeout",
  "message": {
    "time": 3000
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `time` | integer | Yes | Fade out duration (milliseconds) |

Similar to stopall but with a fade. Legacy command from original mqttaudio.

## Precache Command

Pre-load and decode an audio file without playing it.

```json
{
  "command": "precache",
  "message": {
    "file": "http://example.com/bigfile.wav"
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file` | string | Yes | URL or local file path to precache |

**Purpose:** Reduce latency for first playback by downloading and decoding ahead of time.

**Note:** This is just an optimization. Playing a file has the same effect (caches for subsequent plays).

## Cache Clear Command

Clear the entire cache (both memory and disk).

```json
{
  "command": "cache_clear"
}
```

No parameters. Removes all cached audio files.

**Warning:** Next playback of any file will require re-downloading and re-decoding.

## Cache Invalidate Command

Invalidate a specific file in the cache.

```json
{
  "command": "cache_invalidate",
  "message": {
    "file": "http://example.com/updated.wav"
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file` | string | Yes | URL or path of file to invalidate |

**Purpose:** Force re-download of a file that changed on the server during development.

## Command Aliases (Legacy Compatibility)

For compatibility with legacy mqttaudio, the following aliases are supported:

| New Command | Legacy Alias |
|-------------|--------------|
| `play` | `soundPlay` |
| `stopall` | `soundStopAll` |
| `fadeout` | `soundFadeOut` |
| `precache` | `soundPrecache` |

## Error Handling

If a command fails (invalid file, security violation, etc.):
- Error is logged to stderr
- Command is ignored
- Other audio continues playing normally
- **Future enhancement**: Optional error response topic

## Security Notes

### File Path Validation

**Local files** must be within allowed directories (configured in config file):
```json
{
  "command": "play",
  "message": {
    "file": "/opt/sounds/test.wav"  // OK if /opt/sounds is in allowed_directories
  }
}
```

Attempts to access files outside allowed directories are rejected.

**HTTP/HTTPS URLs** are always allowed (assumed to be trusted MQTT network).

### Path Traversal Protection

File paths are canonicalized and checked:
```
/opt/sounds/../../../etc/passwd  // REJECTED
/opt/sounds/./subfolder/test.wav // OK
```

## Performance Notes

### Latency Expectations

| Scenario | Typical Latency |
|----------|-----------------|
| Cached file in memory | < 10ms |
| Cached file on disk | 20-50ms |
| Local file (first play) | 20-50ms |
| HTTP file (first play, good network) | 100-300ms |
| HTTP file (first play, slow network) | 500ms - several seconds |

### Simultaneous Playback

The mixer supports 20+ simultaneous samples without glitching on typical hardware. Practical limits depend on:
- CPU performance
- Audio buffer size
- Number of channels being routed
- Complexity of channel mapping

If too many samples are active, the mixer may drop new play commands or remove oldest samples.

## Complete Example Session

```json
// 1. Precache background music and ambience
{"command": "precache", "message": {"file": "http://example.com/music.mp3"}}
{"command": "precache", "message": {"file": "http://example.com/rain.wav"}}

// 2. Start background music at low volume
{
  "command": "play",
  "message": {
    "file": "http://example.com/music.mp3",
    "voice": "music",
    "volume": 0.3,
    "loop": true,
    "fade_in": 2000
  }
}

// 3. Start rain ambience on specific channels
{
  "command": "play",
  "message": {
    "file": "http://example.com/rain.wav",
    "voice": "ambience",
    "channel_map": [
      {"src": 0, "dest": "rear_left"},
      {"src": 1, "dest": "rear_right"}
    ],
    "volume": 0.5,
    "loop": true
  }
}

// 4. Play a one-shot sound effect
{
  "command": "play",
  "message": {
    "file": "/opt/sounds/doorbell.wav",
    "voice": "effects",
    "volume": 1.0
  }
}

// 5. Fade out music
{"command": "voice_fade_out", "message": {"voice": "music", "time": 3000}}

// 6. Stop all ambience
{"command": "voice_stop", "message": {"voice": "ambience"}}

// 7. Stop everything
{"command": "stopall"}
```
