# Playback Control

mqttaudio provides real-time control over playing audio, including seeking, speed changes, and reverse playback.

## Sample Targeting

To control a playing sample, you need to identify it. Use any combination of:

| Selector | Description |
|----------|-------------|
| `id` | Unique ID you assigned when playing |
| `file` | Filename or URL of the playing audio |
| `voice` | Voice group name |

Multiple selectors use OR logic — the command affects any sample matching any selector.

### Using IDs

Assign an ID when playing:

```json
{
  "command": "play",
  "message": {
    "file": "/sounds/music.mp3",
    "id": "background-track",
    "loop": true
  }
}
```

Then control by ID:

```json
{
  "command": "speed",
  "message": {
    "id": "background-track",
    "speed": 0.5
  }
}
```

## Seek

Jump to a specific position:

```json
{
  "command": "seek",
  "message": {
    "id": "background-track",
    "position_ms": 60000
  }
}
```

Position is in milliseconds from the start.

**Examples:**
- `0` — Jump to beginning
- `60000` — Jump to 1 minute
- `300000` — Jump to 5 minutes

## Speed Control

Change playback speed in real-time:

```json
{
  "command": "speed",
  "message": {
    "id": "background-track",
    "speed": 1.5
  }
}
```

### Speed Values

| Speed | Effect |
|-------|--------|
| 0.5 | Half speed (slower) |
| 1.0 | Normal speed |
| 1.5 | 1.5x speed |
| 2.0 | Double speed |
| -1.0 | Reverse at normal speed |
| -0.5 | Reverse at half speed |

### Speed Ranges

**Without pitch correction:**
- Range: -100.0 to 100.0
- Negative values play in reverse
- Pitch changes with speed (faster = higher pitch)

**With pitch correction:**
- Range: 0.05 to 8.0
- Reverse playback not supported
- Original pitch is maintained

## Pitch Correction

Enable pitch correction to maintain the original pitch when changing speed:

```json
{
  "command": "speed",
  "message": {
    "id": "background-track",
    "speed": 0.5,
    "pitch_correction": true
  }
}
```

**Without pitch correction (default):**
- Faster = higher pitch ("chipmunk effect")
- Slower = lower pitch
- Reverse playback works
- Lower CPU usage

**With pitch correction:**
- Pitch stays constant at any speed
- Uses time-stretching algorithm
- Higher CPU usage
- No reverse playback

## Reverse Playback

Play audio backwards (without pitch correction only):

```json
{
  "command": "speed",
  "message": {
    "id": "effect-1",
    "speed": -1.0
  }
}
```

Negative speeds play the audio in reverse:
- `-1.0` — Reverse at normal speed
- `-0.5` — Reverse at half speed
- `-2.0` — Reverse at double speed

## Volume Control

Adjust volume of specific samples:

```json
{
  "command": "volume",
  "message": {
    "id": "background-track",
    "volume": 0.3
  }
}
```

This differs from `voice_volume` in that it targets specific samples, not entire voice groups.

## Stop Specific Samples

Stop samples with optional fade-out:

```json
{
  "command": "stop",
  "message": {
    "id": "background-track",
    "fade_out_ms": 1000
  }
}
```

Or stop by file:

```json
{
  "command": "stop",
  "message": {
    "file": "/sounds/music.mp3"
  }
}
```

Or by voice:

```json
{
  "command": "stop",
  "message": {
    "voice": "effects",
    "fade_out_ms": 500
  }
}
```

## Play Options

### Start Position

Start playback from a specific position:

```json
{
  "command": "play",
  "message": {
    "file": "/sounds/long-track.mp3",
    "start_position_ms": 120000
  }
}
```

## Examples

### DJ-Style Speed Control

```bash
# Start a track
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/music/track.mp3",
    "id": "deck-a",
    "loop": true
  }
}'

# Slow down for transition
mosquitto_pub -t audio/commands -m '{
  "command": "speed",
  "message": {"id": "deck-a", "speed": 0.95}
}'

# Speed up
mosquitto_pub -t audio/commands -m '{
  "command": "speed",
  "message": {"id": "deck-a", "speed": 1.05}
}'

# Back to normal
mosquitto_pub -t audio/commands -m '{
  "command": "speed",
  "message": {"id": "deck-a", "speed": 1.0}
}'
```

### Slow-Motion Effect

```bash
# Play sound and slow it down dramatically
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/explosion.wav",
    "id": "slowmo"
  }
}'

mosquitto_pub -t audio/commands -m '{
  "command": "speed",
  "message": {
    "id": "slowmo",
    "speed": 0.25,
    "pitch_correction": true
  }
}'
```

### Rewind Effect

```bash
# Play in reverse for a "rewind" effect
mosquitto_pub -t audio/commands -m '{
  "command": "speed",
  "message": {
    "id": "music",
    "speed": -3.0
  }
}'
```

### Jump to Chorus

```bash
# Skip to a specific section
mosquitto_pub -t audio/commands -m '{
  "command": "seek",
  "message": {
    "id": "background-music",
    "position_ms": 90000
  }
}'
```
