# Voice Management

A voice is a named group of sounds. Put related sounds in one voice (all the ambience, all the
music, all the narration) and you can fade, stop or level them together, and let
[ducking rules](ducking.md) react to them.

## Putting sounds in a voice

Name the voice when you play:

```json
{"command": "play", "file": "/opt/sounds/rain.wav", "voice": "ambience", "loop": true}
{"command": "play", "file": "/opt/sounds/wind.wav", "voice": "ambience", "loop": true}
{"command": "play", "file": "/opt/sounds/thunder.wav", "voice": "ambience"}
```

A voice exists while it has sounds playing. When its last sound ends it disappears, along with its
volume setting, and the next play into the same name starts fresh at volume `1.0`.

A play without a `voice` gets a voice of its own with a generated name (shown in
`GET /status/samples`), so nothing else is grouped with it. Name the voice for anything you may want
to control as a group later.

## Voice commands

**Fade out** every sound in the voice, then stop them:

```json
{"command": "voice_fade_out", "voice": "ambience", "time": 3000}
```

**Stop** them (with a 10 ms fade to avoid a click):

```json
{"command": "voice_stop", "voice": "ambience"}
```

**Set the voice's level:**

```json
{"command": "voice_volume", "voice": "music", "volume": 0.3}
```

The level ramps smoothly, applies to every sound playing in the voice, and is inherited by sounds
started in the voice while it still has sounds playing. A live input whose `voice_id` matches is set
too. Each command fails (HTTP `404`, logged over MQTT) when the voice has nothing playing.

## Levels multiply

A sound's loudness is its own `volume` × its voice's level × any ducking applied to the voice:

| Set with | Scope |
|----------|-------|
| `play` `volume`, `volume` command | One sound |
| `voice_volume` | Every sound in the voice |
| Ducking rules | The voice, automatically while a primary voice plays |

Each ranges up to `4.0` (+12 dB) except ducking, which only lowers. `GET /status/voices` shows each
voice's level and current ducking multiplier:

```json
{"voices": [{"id": "music", "sample_count": 1, "volume": 0.3, "ducking_multiplier": 0.2}]}
```

## Patterns

**Scene change.** Fade one scene out while the next fades in:

```bash
mosquitto_pub -t audio/commands -m '{"command": "voice_fade_out", "voice": "scene1", "time": 2000}'
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/scene2.mp3",
  "voice": "scene2", "loop": true, "fade_in": 2000}'
```

**Layered ambience.** Build a soundscape from several loops in one voice, each with its own volume,
and end it with one command:

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/forest.wav", "voice": "ambience", "loop": true, "volume": 0.3}'
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/birds.wav", "voice": "ambience", "loop": true, "volume": 0.5}'
mosquitto_pub -t audio/commands -m '{"command": "voice_fade_out", "voice": "ambience", "time": 5000}'
```

**Announcements over music.** Rather than lowering and restoring the music by hand, add a
[ducking rule](ducking.md) with `"primary_voice": "announcements"` and `"ducked_voices": ["music"]`.

## Naming

Voice names are case-sensitive. Name voices by what they do (`ambience`, `music`, `narration`,
`hints`, `effects`), not by file, and use the same names in your ducking rules. A typical escape room:

| Voice | Holds |
|-------|-------|
| `ambience` | Background atmosphere |
| `music` | Soundtrack |
| `hints` | Game master hints |
| `effects` | One-shot props and puzzle sounds |
| `timer` | Countdown warnings |
