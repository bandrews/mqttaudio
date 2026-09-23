# Playback Control

Once a sound is playing you can move around in it, change its speed and pitch, run it backwards,
change its volume, and stop it. These commands pick their sounds with a selector; see
[Commands](../commands.md#sounds-and-voices) for the full parameter tables.

## Picking sounds

Give a sound an `id` when you play it, then use the `id` to address it:

```json
{"command": "play", "file": "/opt/sounds/radio.mp3", "id": "radio", "loop": true}
{"command": "speed", "id": "radio", "speed": 0.9}
```

`id` is your label, and several sounds may share it. You can also select by `voice`, by the exact
`file` string the sound was played with, or by `internal_id`, the unique id shown in
`GET /status/samples`. A sound matches if any of the given selectors match.

## Seek

```json
{"command": "seek", "id": "radio", "position_ms": 90000}
```

Jumps to a position in milliseconds from the start of the file, clamped to its end. `start_position_ms`
on a `play` does the same from the outset.

## Speed and reverse

```json
{"command": "speed", "id": "radio", "speed": 1.5}
```

| Speed | Effect |
|-------|--------|
| `1.0` | Normal |
| `0.5` | Half speed, an octave lower |
| `2.0` | Double speed, an octave higher |
| `-1.0` | Backwards at normal speed |

Without pitch correction, speed changes pitch like a tape machine, and any value from `-100` to `100`
works (values nearer zero than `0.01` become `±0.01`; `0` is rejected, use `stop`). Samples between
the original ones are found by cubic interpolation. Well above `1.0` the sound can pick up aliasing
artifacts; pitch correction avoids them.

Reverse playback starts where the sound is now. A sound that has only just started therefore ends at
once when reversed; `seek` into it first:

```json
{"command": "seek", "id": "tape", "position_ms": 8000}
{"command": "speed", "id": "tape", "speed": -1.0}
```

A looping sound played backwards wraps from its start to its end.

## Pitch correction

```json
{"command": "speed", "id": "radio", "speed": 0.8, "pitch_correction": true}
```

With `pitch_correction`, the tempo changes and the pitch stays. Speed is limited to `0.05`–`8.0` and
cannot be negative. It costs noticeably more CPU than a plain speed change.

Every `speed` command sets pitch correction on or off, so send `"pitch_correction": true` each time
you want to keep it. A sound still being decoded for its first play gets pitch correction when the
decode finishes; a warning says so.

## Volume

```json
{"command": "volume", "id": "radio", "volume": 0.4}
```

Sets the sound's own volume (`0.0`–`4.0`). It ramps over about 20 ms per unit, so it does not click.
The sound's voice has a separate level, set with `voice_volume`, and ducking rules can lower it
further; the three multiply. See [Voice Management](voice-management.md).

## Fades and stops

```json
{"command": "play", "file": "/opt/sounds/theme.mp3", "id": "theme", "fade_in": 3000}
{"command": "stop", "id": "theme", "fade_out_ms": 2000}
```

`stop` fades out over `fade_out_ms` (10 ms by default, just enough to avoid a click). `fadeall`
fades out everything; `voice_fade_out` fades one voice. A fade-out that starts while a sound is
fading in, or during another fade-out, continues from the sound's current level.

## Looping

```json
{"command": "play", "file": "/opt/sounds/rain.wav", "loop": true, "crossfade_ms": 250}
```

`loop` repeats the file until it is stopped. `crossfade_ms` blends the end into the start at each
repeat with an equal-power crossfade, for files that do not loop cleanly on their own. The file must
be longer than twice the crossfade; otherwise the crossfade is skipped with a warning.

## Windowed sounds

Long or large files may play [windowed](caching.md#full-and-windowed-plays), streaming through a
small buffer. A windowed sound can be stopped, faded and turned up or down, but it ignores `seek`
and `speed`, starts from the beginning, and loops without a crossfade (a windowed URL does not loop
at all); the `play` options it cannot honor are logged as warnings. `"mode": "full"` on the `play`
asks for a full load when the file fits in memory. `GET /status/samples` shows `"windowed": true` for
windowed sounds.

When a `seek` or `speed` selects by `voice`, and a windowed sound has played in that voice since the
voice was last silent, the whole command is ignored, including for the voice's fully loaded sounds.
Select those by `id` instead.
