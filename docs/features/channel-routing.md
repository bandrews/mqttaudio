# Channel Routing

mqttaudio treats the output device as a row of numbered channels and lets each play put each of its
file's channels on any of them.

## Channels

Outputs are numbered from 0. What each number feeds depends on your interface and wiring; a common
layout is:

| Channel | 5.1 | 7.1 |
|---------|-----|-----|
| 0 | Front left | Front left |
| 1 | Front right | Front right |
| 2 | Center | Center |
| 3 | LFE (subwoofer) | LFE (subwoofer) |
| 4 | Rear left | Side left |
| 5 | Rear right | Side right |
| 6 | | Rear left |
| 7 | | Rear right |

`--list-devices` shows each device's channel count (`Native: 8 ch, ...`), and `GET /status` shows
the count the daemon opened (`output_channels`).

## Default routing

Without a `channel_map`, file channel 0 plays on output 0, channel 1 on output 1, and so on. A mono
file therefore plays on output 0 only, and a stereo file on outputs 0 and 1.

## Channel maps

A play's `channel_map` lists routes, each from a file channel (`src`) to an output (`dest`):

```json
{
  "command": "play",
  "file": "/opt/sounds/stereo.wav",
  "channel_map": [{"src": 0, "dest": 4}, {"src": 1, "dest": 5}]
}
```

- **One to many.** Repeat a `src` to send it to several outputs, for example a mono announcement to
  every speaker.
- **Some channels only.** File channels without a route are not heard: `[{"src": 0, "dest": 0}]`
  plays only the left channel of a stereo file.
- **Many to one.** Routes to the same output add together, which can clip; lower their gains.
- **Missing outputs.** A route to an output the device does not have is silently dropped.

### Per-route gain

Each route may carry a `gain` (default `1.0`, range `0.0`–`8.0`) that scales only that route:

```json
"channel_map": [
  {"src": 0, "dest": 0, "gain": 0.5},
  {"src": 2, "dest": 0, "gain": 0.5},
  {"src": 1, "dest": 1, "gain": 0.5},
  {"src": 3, "dest": 1, "gain": 0.5}
]
```

This folds a four-channel file down to stereo without the sums clipping.

## Naming channels

`audio.channel_aliases` gives outputs names, which then work anywhere a channel number does: in
`channel_map`s, input routes, bass management and `channel_volumes`.

```json
"audio": {
  "channel_aliases": {"front_left": 0, "front_right": 1, "lfe": 3, "booth_left": 6, "booth_right": 7}
}
```

```json
{"command": "play", "file": "/opt/sounds/hint.wav", "channel_map": [{"src": 0, "dest": "booth_left"}, {"src": 1, "dest": "booth_right"}]}
```

An unknown name fails the play with HTTP `400`. (`audio.channel_names` holds display labels and is
not used for routing; the error message says so if you mix them up.)

## Reusing maps

Put maps you use often in [macros](../configuration.md#macros):

```json
"macros": {
  "everywhere": {"channel_map": [{"src": 0, "dest": 0}, {"src": 0, "dest": 1}, {"src": 0, "dest": 4}, {"src": 0, "dest": 5}]},
  "booth": {"channel_map": [{"src": 0, "dest": "booth_left"}, {"src": 1, "dest": "booth_right"}]}
}
```

```json
{"command": "play", "file": "/opt/sounds/announcement.wav", "macro": "everywhere"}
```

## Levelling speakers

To trim a speaker for every play, set `audio.channel_volumes` rather than route gains; it applies to
the finished mix of each output. See [Configuration](../configuration.md#audio).

## The subwoofer channel

With [bass management](bass-management.md) on, a route straight to the LFE channel reaches the
subwoofer unfiltered, and the extracted bass is added on top. Route to it only for content made for
the subwoofer.

## Examples

Four-channel ambience on the front and rear pairs of a 5.1 system:

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/forest-quad.wav", "loop": true,
  "channel_map": [{"src": 0, "dest": 0}, {"src": 1, "dest": 1}, {"src": 2, "dest": 4}, {"src": 3, "dest": 5}]}'
```

The same mono announcement in three stereo zones:

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/closing.wav",
  "channel_map": [{"src": 0, "dest": 0}, {"src": 0, "dest": 1}, {"src": 0, "dest": 2},
                  {"src": 0, "dest": 3}, {"src": 0, "dest": 4}, {"src": 0, "dest": 5}]}'
```

Music on a headphone output wired to channels 6 and 7:

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/music.mp3", "voice": "headphones",
  "loop": true, "channel_map": [{"src": 0, "dest": 6}, {"src": 1, "dest": 7}]}'
```

Test a new installation with a mono tone on one output at a time before playing real content; the
config editor's device picker can do this without a running daemon.
