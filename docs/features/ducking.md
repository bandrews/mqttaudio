# Ducking

Ducking turns background voices down while a foreground voice plays, and brings them back
afterwards, so narration, hints and announcements stay clear without anyone riding the faders.

## A rule

```json
"ducking_rules": [
  {
    "primary_voice": "narration",
    "ducked_voices": ["music", "ambience"],
    "target_volume": 0.15,
    "fade_duration_ms": 1000
  }
]
```

While any sound plays in the `narration` voice, the `music` and `ambience` voices fade to 15% of
their level over one second. When the last narration sound ends, they fade back.

| Setting | Meaning |
|---------|---------|
| `primary_voice` | The voice whose activity triggers the rule. A live input's `voice_id` works too |
| `ducked_voices` | Voices to turn down. May include live inputs' voices |
| `target_volume` | Multiplier while ducked, `0.0`–`1.0` |
| `fade_duration_ms` | Fade length, up to `60000` |

Ducking multiplies with each sound's `volume` and its voice's `voice_volume`, so levels you set
while a voice is ducked are kept and heard in full once it recovers. A sound that starts in a
ducked voice starts ducked.

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/theme.mp3", "voice": "music", "loop": true, "volume": 0.7}'
# The music drops to 15% of 0.7 over one second...
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/welcome.wav", "voice": "narration"}'
# ...and returns to 0.7 when welcome.wav ends.
```

## Several rules

Rules can overlap. While several rules duck the same voice at once:

- it goes to the **lowest** of their `target_volume`s,
- using the **shortest** of their fades;
- when they have all ended, it recovers over the **longest** fade that ducked it.

```json
"ducking_rules": [
  {"primary_voice": "announcement", "ducked_voices": ["music", "ambience", "narration"], "target_volume": 0.05, "fade_duration_ms": 300},
  {"primary_voice": "narration", "ducked_voices": ["music", "ambience"], "target_volume": 0.2, "fade_duration_ms": 1000}
]
```

Here an announcement ducks everything, narration included, almost to silence; narration alone
ducks only the background.

## Live microphones

**As a trigger.** Give the input an `activity_threshold` and use its `voice_id` as a
`primary_voice`. The input counts as active from the moment its level reaches the threshold until it
has stayed below it for `activity_hold_ms` (750 ms by default), which keeps the duck from pumping
between words:

```json
{
  "inputs": [
    {
      "device": "plughw:CARD=Headset,DEV=0",
      "voice_id": "gm_mic",
      "activity_threshold": 0.05,
      "routes": [{"source_channel": 0, "dest_channel": 0}, {"source_channel": 0, "dest_channel": 1}]
    }
  ],
  "ducking_rules": [
    {"primary_voice": "gm_mic", "ducked_voices": ["ambience", "music"], "target_volume": 0.1, "fade_duration_ms": 300}
  ]
}
```

The level is the peak of the input's routed channels as captured, checked every 20 ms, before the
input's volume and mute. Set the threshold above the room's background noise: watch the levels in
the config editor's input picker to find it. Because mute does not change the captured level, a
muted microphone that picks up sound still triggers its rules.

Without `activity_threshold`, an input is active the whole time it is open, so its rules duck
permanently.

**As a ducked voice.** List an input's `voice_id` in `ducked_voices` to turn the microphone down
while the primary plays.

## Checking it

`GET /status/voices` shows each voice's `ducking_multiplier` (below `1.0` while ducked), and
`GET /metrics` lists the currently ducked voices. If nothing ducks, check that the voice names match
exactly (they are case-sensitive) and that the primary voice really has a sound playing.

## Choosing values

- Speech over music usually sits well at `0.1`–`0.3`.
- Short fades (200–500 ms) suit live voices and hints; longer ones (1–2 s) suit narration over music.
- Since recovery uses the longest fade, a slow rule makes the background come back slowly even after
  a fast one.
