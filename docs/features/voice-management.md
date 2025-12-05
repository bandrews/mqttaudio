# Voice Management

Voices are named groups of sounds that you can control together. They're essential for organizing audio in complex installations.

## Creating Voices

A voice is created automatically when you play audio with a voice name:

```json
{
  "command": "play",
  "file": "/sounds/rain.wav",
  "voice": "ambience"
}
```

Multiple sounds can belong to the same voice:

```json
{"command": "play", "file": "/sounds/rain.wav", "voice": "ambience", "loop": true}
{"command": "play", "file": "/sounds/wind.wav", "voice": "ambience", "loop": true}
{"command": "play", "file": "/sounds/thunder.wav", "voice": "ambience"}
```

All three sounds are now in the "ambience" voice.

## Voice Commands

### Stop a Voice

Stop all sounds in a voice immediately:

```json
{
  "command": "voice_stop",
  "voice": "ambience"
}
```

### Fade Out a Voice

Fade out all sounds in a voice smoothly:

```json
{
  "command": "voice_fade_out",
  "voice": "ambience",
  "time": 3000
}
```

After the fade completes, all sounds are removed.

### Adjust Voice Volume

Change the volume of all sounds in a voice:

```json
{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.5
}
```

This affects currently playing sounds and any new sounds added to this voice.

## Common Patterns

### Background Music with Effects

```bash
TOPIC="audio/commands"

# Start background music
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/music.mp3",
  "voice": "music",
  "loop": true,
  "volume": 0.4
}'

# Play sound effects as needed
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/doorbell.wav",
  "voice": "effects"
}'

# Lower music for an announcement
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.1
}'

# Restore music volume after announcement
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.4
}'
```

### Layered Ambience

Build up complex soundscapes by layering:

```bash
# Base layer
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/forest-base.wav",
  "voice": "ambience",
  "loop": true,
  "volume": 0.3
}'

# Add birds
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/birds.wav",
  "voice": "ambience",
  "loop": true,
  "volume": 0.5
}'

# Add stream
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/stream.wav",
  "voice": "ambience",
  "loop": true,
  "volume": 0.4
}'

# Stop everything at once
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_fade_out",
  "voice": "ambience",
  "time": 5000
}'
```

### Transition Between Scenes

```bash
# Fade out current scene
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_fade_out",
  "voice": "scene1",
  "time": 2000
}'

# Start next scene (fading in)
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/scene2-music.mp3",
  "voice": "scene2",
  "loop": true,
  "fade_in": 2000
}'
```

## Voice Naming Tips

1. **Use descriptive names** — "narration", "music", "effects" are better than "v1", "v2"
2. **Be consistent** — Stick to a naming convention across your installation
3. **Group by function** — Not by file or location
4. **Consider ducking** — Names that work well with ducking rules (see [Audio Ducking](ducking.md))

Example naming scheme for an escape room:
- `ambient` — Background atmosphere
- `music` — Background music
- `hints` — Gamemaster hints
- `effects` — One-shot sound effects
- `victory` — Win sounds
- `timer` — Timer warnings

## Anonymous Voices

If you don't specify a voice, an anonymous voice is created:

```json
{
  "command": "play",
  "file": "/sounds/effect.wav"
}
```

Anonymous voices can't be controlled with voice commands. Use named voices for any sounds you might need to control later.
