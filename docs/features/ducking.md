# Audio Ducking

Audio ducking automatically reduces the volume of background sounds when foreground sounds play. This ensures speech, narration, and announcements are clearly audible over music and effects.

## How It Works

1. You define rules in your config file
2. When a "primary" voice starts playing, "ducked" voices fade down
3. When the primary voice stops, ducked voices fade back up
4. All transitions are smooth and glitch-free

## Configuration

Add ducking rules to your config file:

```json
{
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `primary_voice` | Voice that triggers ducking when it plays |
| `ducked_voices` | Voices that will be reduced in volume |
| `target_volume` | Volume level to duck to (0.0 to 1.0) |
| `fade_duration_ms` | Fade time in milliseconds |

## Examples

### Simple Narration Ducking

Duck music and effects when narration plays:

```json
{
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ]
}
```

**Usage:**
```bash
# Start background music
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/music.mp3",
    "voice": "music",
    "loop": true,
    "volume": 0.7
  }
}'

# Play narration - music automatically ducks to 15%
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/narration.wav",
    "voice": "narration"
  }
}'

# When narration finishes, music automatically restores to 70%
```

### Priority Hierarchy

Create multiple rules for different priority levels:

```json
{
  "ducking_rules": [
    {
      "primary_voice": "announcement",
      "ducked_voices": ["music", "effects", "narration"],
      "target_volume": 0.02,
      "fade_duration_ms": 500
    },
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    },
    {
      "primary_voice": "dialog",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.10,
      "fade_duration_ms": 1500
    }
  ]
}
```

This creates a 3-tier priority:
1. **Announcements** (highest) — Ducks everything to 2%
2. **Dialog** — Ducks music and effects to 10%
3. **Narration** — Ducks music and effects to 15%
4. **Music/Effects** (lowest) — Never triggers ducking

### Escape Room Setup

```json
{
  "ducking_rules": [
    {
      "primary_voice": "gm_mic",
      "ducked_voices": ["ambient", "effects", "music"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    },
    {
      "primary_voice": "hints",
      "ducked_voices": ["ambient", "music"],
      "target_volume": 0.2,
      "fade_duration_ms": 1000
    }
  ]
}
```

When the gamemaster speaks (through the microphone), everything else ducks quickly. Pre-recorded hints also duck background audio but less aggressively.

## Multiple Rules

When multiple rules apply simultaneously:
- The **lowest target volume** is used
- The **fastest fade** is used

For example, if both narration (15%, 2000ms) and dialog (10%, 1500ms) are playing, music ducks to 10% with a 1500ms fade.

## Microphone Input Ducking

Ducking works with microphone inputs too. Configure the microphone with a `voice_id`:

```json
{
  "inputs": [
    {
      "device": "USB Microphone",
      "voice_id": "presenter_mic",
      "routes": [{"source_channel": 0, "dest_channel": 0}]
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "presenter_mic",
      "ducked_voices": ["music"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    }
  ]
}
```

Now whenever the presenter speaks, music automatically ducks.

## Tips

1. **Start with conservative settings** — It's easier to adjust from "too quiet" than to fix overloaded audio
2. **Match fade times to content** — Fast fades for live mic, slower for pre-recorded narration
3. **Test with real content** — Ducking behavior can be surprising with certain audio combinations
4. **Consider silence detection** — Very quiet primary voices may not trigger ducking noticeably

## Troubleshooting

**Ducking not working:**
- Verify voice names match exactly (case-sensitive)
- Check that the primary voice is actually playing audio
- Confirm ducking rules are in your config file

**Fades are abrupt:**
- Increase `fade_duration_ms`

**Ducking is too aggressive:**
- Increase `target_volume` (e.g., 0.15 → 0.25)

**Audio pops during transitions:**
- This shouldn't happen — please report as a bug
