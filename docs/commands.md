# Commands

Every command is a JSON object with a `command` field and its parameters. Publish it to the
configured MQTT topic, or POST it to the [HTTP API](http-api.md).

```json
{"command": "play", "file": "/opt/sounds/doorbell.wav", "voice": "effects", "volume": 0.8}
```

- **MQTT** is fire-and-forget: nothing is sent back. A command that fails is logged at `warn` or
  `error` level.
- **HTTP** waits for the outcome and answers with a status code (`POST /command` takes the same JSON;
  see [HTTP API](http-api.md#command-results)).
- Command names are case-sensitive. Parameters the command does not know are ignored, so a misspelled
  parameter name has no effect rather than causing an error.
- Commands run in the order they arrive, except those that read files (`play`, `precache`,
  `cache_clear`, `cache_invalidate`, `cache_reload`): these load in the background, at most 4 at a
  time and 32 in flight, and take effect when their load finishes. A slow download therefore never
  holds up a `stop` or a volume change. `stopall` and `fadeall` also cancel every load still in flight;
  other commands do not, so a `stop` sent while its sound is still loading finds nothing to stop.

| Command | Does | HTTP endpoint |
|---------|------|---------------|
| [`play`](#play) | Play a file or URL | `POST /play` |
| [`stop`](#stop) | Stop matching sounds | `POST /stop` |
| [`stopall`](#stopall) | Stop every sound | `POST /stopall` |
| [`fadeall`](#fadeall) | Fade out every sound | `POST /fadeall` |
| [`volume`](#volume) | Change matching sounds' volume | `POST /volume` |
| [`seek`](#seek) | Jump within matching sounds | `POST /seek` |
| [`speed`](#speed) | Change matching sounds' speed | `POST /speed` |
| [`voice_stop`](#voice_stop) | Stop a voice | `POST /voice/stop` |
| [`voice_fade_out`](#voice_fade_out) | Fade out a voice | `POST /voice/fade_out` |
| [`voice_volume`](#voice_volume) | Change a voice's volume | `POST /voice/volume` |
| [`input_volume`](#input_volume) | Change a live input's volume | `POST /input/volume` |
| [`input_mute`](#input_mute) | Mute or unmute a live input | `POST /input/mute` |
| [`precache`](#precache) | Load a file into the cache | `POST /precache` |
| [`cache_clear`](#cache_clear) | Empty the caches | `POST /cache/clear` |
| [`cache_invalidate`](#cache_invalidate) | Drop one file from the caches | `POST /cache/invalidate` |
| [`cache_reload`](#cache_reload) | Drop one file and load it again | `POST /cache/reload` |
| [`talkback_acquire`](#talkback) | Open the talkback microphone for a while | `POST /talkback/acquire` |
| [`talkback_release`](#talkback) | Close it | `POST /talkback/release` |
| [`talkback_hard_mute`](#talkback) | Close it whoever holds it | `POST /talkback/hard-mute` |

Volumes throughout are linear gains: `1.0` is unity, `0.5` is about -6 dB, and the maximum is `4.0`
(+12 dB). Values outside `0.0`–`4.0` are clamped.

## Sounds and voices

Each `play` starts one **sound**. A sound belongs to a **voice**, a named group used by the `voice_*`
commands and by [ducking rules](features/ducking.md). A play without a `voice` gets a voice of its
own. A voice exists while it has sounds playing: when the last one ends the voice is removed, and a
later play into the same name starts again at voice volume `1.0`.

`stop`, `volume`, `seek` and `speed` pick sounds with a **selector**:

| Selector | Matches |
|----------|---------|
| `internal_id` | The one sound with this id. Ids are assigned by the daemon and listed by `GET /status/samples`; pass it as a string of digits, such as `"42"` |
| `id` | Sounds started with this `id` |
| `file` | Sounds started with exactly this `file` string |
| `voice` | Sounds in this voice |

Give at least one. A sound matches if any given selector matches it. A command with no selector is
rejected (HTTP `400`), and one that matches no playing sound fails with HTTP `404`.

## Playback

### play

```json
{
  "command": "play",
  "file": "/opt/sounds/rain.wav",
  "id": "rain",
  "voice": "ambience",
  "volume": 0.6,
  "loop": true,
  "crossfade_ms": 200,
  "fade_in": 2000
}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `file` | string | *required* | Local path, or an `http://` / `https://` URL. With `security.allowed_directories` set, a local path must be inside one of them |
| `id` | string | none | Your label for the sound, for later selectors. Several sounds may share one |
| `voice` | string | a voice of its own | Voice to play in |
| `volume` | number | `1.0` | Sound volume, `0.0`–`4.0` |
| `loop` | boolean | `false` | Repeat until stopped |
| `crossfade_ms` | integer | `0` | Blend the end of the file into its start at each loop. Needs `loop: true` and a file longer than twice the crossfade; a warning is logged otherwise |
| `fade_in` | integer | none | Fade in over this many milliseconds |
| `start_position_ms` | integer | `0` | Start this far into the file (clamped to its end) |
| `channel_map` | array | channel *n* → output *n* | Where each source channel plays; see [Channel map](#channel-map) |
| `mode` | string | `cache.load_mode` | `auto`, `full` or `stream`; see [Full and windowed plays](#full-and-windowed-plays) |
| `window_ms` | integer | `cache.stream_window_ms` | Window length for a windowed play, `100`–`60000` |
| `prebuffer_ms` | integer | `cache.stream_prebuffer_ms` | Audio buffered before a windowed play starts, capped at the window. At most `window_ms` when the play sets both (HTTP `400`) |
| `freshness` | string | `cache.freshness` | `trusting`, `dev` or `pinned`: whether an edited local file already in memory is re-read (not with `pinned`), and when a cached URL is checked with its server in the background (`dev` on every play, `pinned` never); see [Caching](features/caching.md#freshness) |
| `cacheable` | boolean | `true` | For a URL that is not cached yet: `false` keeps the download out of the disk cache (for a live stream or a one-off file). A response without `Content-Length` is never saved |

Without a `channel_map`, source channel 0 plays on output 0, channel 1 on output 1, and so on: a mono
file plays on output 0 only. Source channels beyond the device's channel count are not heard.

At most `audio.max_sounds` (256) full plays and `audio.max_streamed_sounds` (64) windowed plays run
at once. At the full limit a new full play replaces the oldest one that is not looping, without a
fade, and a warning names it; when every full play loops, the new play fails (HTTP `500`). A windowed
play over its limit fails the same way.

#### Full and windowed plays

A **full** play decodes the whole file into memory. It supports everything above plus `seek`,
`speed`, reverse playback and pitch correction, and a finished decode stays in the memory cache for
instant replays. A **windowed** play streams through a short buffer (`window_ms`), so memory stays
small however long the file is, but it:

- always starts at the beginning (`start_position_ms` is ignored, with a warning);
- loops a local file without a crossfade (`crossfade_ms` is ignored, with a warning), and plays a URL
  only once (`loop` is ignored, with a warning);
- cannot `seek` or change `speed`.

`"auto"`, or leaving `mode` out, uses `cache.load_mode` (default `auto`). In `auto` a file plays in
full unless it is larger than `cache.full_load_max_bytes` decoded, longer than
`cache.full_load_max_seconds`, or too big for the memory still free in the cache budget; a URL of
unknown length is always windowed. `"full"` asks for a full play but still falls back to windowed when
the file would not fit in memory, and `"stream"` plays windowed. A file already in memory, or being
loaded in full, is shared unless the play itself says `"stream"`. A URL already on disk is decided
like a local file and, when windowed, streams from its disk copy.
[Caching](features/caching.md#how-a-play-is-loaded) has the details.

#### Channel map

Each entry routes one source channel to one output channel:

```json
{
  "command": "play",
  "file": "/opt/sounds/quad-ambience.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5},
    {"src": 2, "dest": "rear_left"},
    {"src": 3, "dest": "rear_right", "gain": 0.8}
  ]
}
```

- `src` is a channel of the file, `dest` an output channel. Either may be a number or a name from
  `audio.channel_aliases`; an unknown name fails the play (HTTP `400`).
- A source channel may appear in several entries to play on several outputs.
- `gain` (default `1.0`, range `0.0`–`8.0`) scales one route. Routes that land on the same output
  add together, so lower their gains to avoid clipping when folding channels down.

### stop

```json
{"command": "stop", "id": "rain", "fade_out_ms": 500}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id`, `id`, `file`, `voice` | string | | [Selector](#sounds-and-voices) |
| `fade_out_ms` | integer | `10` | Fade-out length. The short default avoids a click |

A sound that is already fading continues from its current level.

### stopall

```json
{"command": "stopall"}
```

Fades every sound out over 10 ms and cancels loads still in flight (their HTTP callers get `409`).
Live inputs keep running.

### fadeall

```json
{"command": "fadeall", "time": 3000}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `time` | integer | `1000` | Fade-out length in milliseconds. Also accepted as `fade_out_ms` |

Like `stopall` with a longer fade: every sound fades out and ends, loads in flight are cancelled, and
live inputs keep running.

### volume

```json
{"command": "volume", "voice": "ambience", "volume": 0.3}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id`, `id`, `file`, `voice` | string | | [Selector](#sounds-and-voices) |
| `volume` | number | *required* | New sound volume, `0.0`–`4.0` |

The change ramps over about 20 ms per unit of volume, so it does not click. This sets each sound's
own volume; `voice_volume` sets a separate voice level, and the two multiply.

### seek

```json
{"command": "seek", "id": "narration", "position_ms": 60000}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id`, `id`, `file`, `voice` | string | | [Selector](#sounds-and-voices) |
| `position_ms` | integer | *required* | New position, clamped to the end of the file |

Applies to the full plays the selector matches; windowed sounds it also matches are skipped, with a
warning. When it matches only windowed sounds, the command fails (HTTP `409`).

### speed

```json
{"command": "speed", "id": "narration", "speed": 0.8, "pitch_correction": true}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `internal_id`, `id`, `file`, `voice` | string | | [Selector](#sounds-and-voices) |
| `speed` | number | *required* | Playback rate: `1.0` normal, `2.0` double, negative plays backwards |
| `pitch_correction` | boolean | `false` | Keep the original pitch while changing speed |

- Without pitch correction, speed ranges from `-100` to `100`; values closer to zero than `0.01` become
  `±0.01`. With it, speed is clamped to `0.05`–`8.0` and cannot be negative (HTTP `400`).
- `speed: 0` is rejected (HTTP `400`); use `stop`.
- Each `speed` command sets pitch correction on or off, so one without `pitch_correction` turns it off.
- Reverse playback starts from the current position, so a sound still at its start ends at once (a
  looping one wraps to its end). `seek` first to play backwards from a point.
- A sound still being decoded for its first play changes speed at once without pitch correction, and a
  warning says correction is deferred. Correction starts once the decoded file is kept in the memory
  cache, and never if it is not kept; see [Caching](features/caching.md#full-and-windowed-plays).
- Applies to full plays only, with the same rule for windowed sounds as `seek`.

## Voices

### voice_stop

```json
{"command": "voice_stop", "voice": "ambience"}
```

Fades every sound in the voice out over 10 ms. Fails with HTTP `404` when the voice has no sounds.

### voice_fade_out

```json
{"command": "voice_fade_out", "voice": "ambience", "time": 5000}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `voice` | string | *required* | Voice to fade |
| `time` | integer | *required* | Fade length in milliseconds (the HTTP endpoint calls it `time_ms`) |

Fails with HTTP `404` when the voice has no sounds.

### voice_volume

```json
{"command": "voice_volume", "voice": "music", "volume": 0.4}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `voice` | string | *required* | Voice, or a live input's `voice_id` |
| `volume` | number | *required* | Voice level, `0.0`–`4.0` |

Ramps every sound in the voice (and any live input with that `voice_id`) to the new level. Sounds
started in the voice while it keeps playing inherit the level; once the voice's last sound ends the
level is forgotten. Fails with HTTP `404` when no sound or ready input has the voice.

## Live inputs

These address a live input from the config's `inputs` list, by its `voice_id` or by its position in
the list as a string (`"0"` is the first). The value must be a JSON string. An input that failed to
open answers HTTP `404`.

### input_volume

```json
{"command": "input_volume", "input": "gm_mic", "volume": 0.8}
```

Sets the input's volume (`0.0`–`4.0`) and unmutes it.

### input_mute

```json
{"command": "input_mute", "input": "gm_mic", "mute": true}
```

Muting silences the input and remembers its volume; unmuting restores that volume. While a
[talkback](#talkback) lease holds the input, unmuting it by its `voice_id` is refused (HTTP `403`);
selecting it by position, or raising it with `input_volume`, is not checked.

Both commands take an optional `fade_ms` (default `20`, up to `60000`): the change fades over that
many milliseconds, and `0` makes it instant. A muted input does not trigger ducking.

## Cache

### precache

```json
{"command": "precache", "file": "https://example.com/sounds/intro.mp3"}
```

Loads a file the way an `auto` play of it would, so a later play starts instantly: a file that would
play in full is decoded into the memory cache (and a URL saved to the disk cache when it is enabled);
one that would play windowed is not decoded, and a URL is downloaded into the disk cache instead. The
command completes once loading has started; the load continues in the background, a play that
arrives meanwhile shares it, and a failure is only logged. Directories are accepted only in the
config's `cache.precache`. See [Caching](features/caching.md#precaching).

### cache_clear

```json
{"command": "cache_clear"}
```

Empties the memory cache and deletes every file in the disk cache. Loads in progress still finish
into the memory cache, but downloads in progress are not saved to disk.

### cache_invalidate

```json
{"command": "cache_invalidate", "file": "https://example.com/sounds/intro.mp3"}
```

Drops one file (or URL) from both caches, so its next play reads it again.

### cache_reload

```json
{"command": "cache_reload", "file": "/opt/sounds/cue-12.wav"}
```

`cache_invalidate` followed by `precache`, so once the reload has finished the next play gets the
current file without waiting for it to load. Use it after replacing a file in place.

## Talkback

Talkback lets one client at a time open a microphone for a short, renewable lease, for example a
push-to-talk button in a control panel. When the lease runs out, is released or is hard-muted, the
microphone is muted again. See [Microphone Input](features/microphone-input.md#talkback) for setup
and current limitations.

```json
{"command": "talkback_acquire", "client_id": "panel-1", "destination": "GUEST_ALL", "gain": 0.0, "lease_ms": 1000}
{"command": "talkback_release", "client_id": "panel-1", "lease_id": "lease-0001"}
{"command": "talkback_hard_mute"}
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `client_id` | string | *required* | Who holds the lease |
| `source_id` | string | `"GM_MIC"` | Must be `GM_MIC`. It opens the input whose `voice_id` is `GM_MIC`, or else `mic` |
| `destination` | string | *required* | One of `GUEST_ALL`, `ROOM_1`, `ROOM_2A`, `ROOM_2B`, `ROOM_3`, `ROOM_4` |
| `gain` | number | *required* | `-60` to `12` (dB) |
| `lease_ms` | integer | *required* | Lease length, `250`–`2000` ms |
| `lease_id` | string | *required* (release) | The lease's id, from `GET /status/talkback` |

- Acquiring unmutes the input and starts a lease. The holder renews it by acquiring again before it
  expires; another client is refused (HTTP `403`) until then.
- Acquiring fails with HTTP `404` when no input with the `voice_id` `GM_MIC` (or `mic`) is open, and
  with `403` for a value outside its range or list.
- Releasing needs the holder's `client_id` and the current `lease_id`.
- `talkback_hard_mute` ends the current lease, whoever holds it, and mutes its input.

## Macros

A `macro` field merges presets from the config's [`macros`](configuration.md#macros) section into a
command:

```json
{"command": "play", "file": "/opt/sounds/theme.mp3", "macro": ["quiet", "wholeroom"], "voice": "music"}
```

The command's own parameters win, then earlier macros over later ones. Here `voice` comes from the
command, `volume` from `quiet`, and `channel_map` from `wholeroom` (given the config example linked
above). `macro` goes at the top level of the command. An unknown macro name is skipped with a warning.

## Compatibility with the original mqttaudio

Commands may put their parameters in a `message` object instead of at the top level:

```json
{"command": "play", "message": {"file": "/opt/sounds/rain.wav", "volume": 0.8}}
```

When `message` is present, only its contents are read.

These command names from the original app are also accepted:

| Name | Same as |
|------|---------|
| `soundPlay` | `play` |
| `soundStopAll` | `stopall` |
| `soundPrecache` | `precache` |
| `soundFadeAll`, `fadeout`, `soundFadeOut` | `fadeall` |

## Examples

```bash
TOPIC=audio/commands

# Load the cues for tonight so they start instantly
mosquitto_pub -t $TOPIC -m '{"command": "precache", "file": "/opt/sounds/theme.mp3"}'

# Background music on a loop, fading in
mosquitto_pub -t $TOPIC -m '{"command": "play", "file": "/opt/sounds/theme.mp3", "voice": "music",
  "volume": 0.3, "loop": true, "crossfade_ms": 100, "fade_in": 2000}'

# A narration; with a ducking rule for "narration" the music dips on its own
mosquitto_pub -t $TOPIC -m '{"command": "play", "file": "/opt/sounds/intro.wav", "id": "intro", "voice": "narration"}'

# Four-channel ambience on outputs 4-7
mosquitto_pub -t $TOPIC -m '{"command": "play", "file": "/opt/sounds/quad.wav",
  "channel_map": [{"src": 0, "dest": 4}, {"src": 1, "dest": 5}, {"src": 2, "dest": 6}, {"src": 3, "dest": 7}]}'

# Slow a sound down without lowering its pitch
mosquitto_pub -t $TOPIC -m '{"command": "speed", "id": "intro", "speed": 0.8, "pitch_correction": true}'

# End of the show
mosquitto_pub -t $TOPIC -m '{"command": "fadeall", "time": 5000}'
```
