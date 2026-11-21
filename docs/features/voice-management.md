# Voice Management

Voices are named groups of sounds that you can control together. They're essential for organizing audio in complex installations.

## Voice Lifecycle

A voice is created the first time any of these name it:

- `play` with a `voice`
- `voice_volume` or `voice_speed` — so a voice can be set up before anything
  plays in it
- a microphone's `voice_id` in the config file, registered at startup when
  the input opens

Once created, a voice and its settings last until `voice_clear` or `reset`,
including while nothing is playing in it. Stopping or fading a voice stops
its sounds but keeps its settings, and the next `play` into it starts at the
stored volume and speed.

`voice_stop` and `voice_fade_out` on a voice with nothing playing succeed
and report 0 samples; they do not create the voice.

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

A voice exists from the moment a play naming it arrives, while the play's file
is still loading. Once one of those plays starts, the voice stays. If none of
them starts (each fails or is cancelled) and no setting was stored on the
voice, it ends with the last of them. Voice commands sent while a play loads
reach it: `voice_volume` and `voice_speed` set the volume and speed it starts
at, and `voice_stop`, `voice_fade_out` or `voice_clear` cancels it before it
is heard (see
[Commands sent while a play is loading](../commands.md#commands-sent-while-a-play-is-loading)).

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

After the fade completes, all sounds are removed. The voice keeps its
settings.

### Adjust Voice Volume

Change the volume of all sounds in a voice:

```json
{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.5
}
```

This affects currently playing sounds, any new sounds added to this voice,
and any microphone whose `voice_id` is this voice. The volume is remembered
until the voice is cleared.

### Adjust Voice Speed

Change the playback speed of all sounds in a voice:

```json
{
  "command": "voice_speed",
  "voice": "music",
  "speed": 0.8,
  "pitch_correction": true
}
```

Like volume, this affects currently playing sounds and any new sounds added
to this voice, and is remembered until the voice is cleared. It sets each
sound's speed (it does not multiply it); a `speed` command aimed at one sound
overrides that sound until the next `voice_speed`, and a `play` that gives
its own `speed` starts at that speed instead. Windowed (streamed) sources
play forward only and stay at normal speed. See
[Playback Control](playback-control.md) for speed ranges and pitch
correction.

### Clear a Voice

Stop a voice's sounds and forget the voice and its settings:

```json
{
  "command": "voice_clear",
  "voice": "music"
}
```

Use `"voice": "*"` to clear every voice at once. A microphone's voice stays
registered with default settings.

### Reset Everything

`reset` returns the daemon to its startup state: all playback stops, every
voice is forgotten, and microphones return to their configured volume,
unmuted. Precached files stay cached. See [Command Reference](../commands.md#reset).

```json
{"command": "reset"}
```

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

### Predictable Start Between Games

Send `reset` when a game ends, then set up the next one. Nothing from the
previous game - a lowered music volume, a slowed voice, a muted mic -
carries over:

```bash
mosquitto_pub -t $TOPIC -m '{"command": "reset"}'

# Configure voices before anything plays in them
mosquitto_pub -t $TOPIC -m '{"command": "voice_volume", "voice": "music", "volume": 0.4}'
mosquitto_pub -t $TOPIC -m '{"command": "voice_volume", "voice": "hints", "volume": 0.9}'
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

## Microphone Voices

Each configured microphone's `voice_id` is registered as a voice at startup.
`voice_volume` on it adjusts the microphone (on top of its own
`input_volume`), and it appears in `/status/voices` as active from the start.

## Anonymous Voices

If you don't specify a voice, an anonymous voice is created:

```json
{
  "command": "play",
  "file": "/sounds/effect.wav"
}
```

Anonymous voices can't be controlled with voice commands. Use named voices for any sounds you might need to control later.

Unlike named voices, an anonymous voice is forgotten as soon as its sound
finishes, so one-off plays do not accumulate.
