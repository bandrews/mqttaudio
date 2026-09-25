# Microphone Input

mqttaudio can mix live inputs (microphones, line inputs, the outputs of other equipment) into its
output with low latency: a game master talking into the room, a presenter over the music, a feed from
another system. Inputs take part in voice levels and ducking like any other sound.

## Setting up an input

Find the capture device:

```bash
mqttaudio --list-inputs
```

```text
Available audio input devices:
  0. plughw:CARD=Headset,DEV=0
     Sample rate: 8000-192000 Hz
     Max channels: 2
  1. plughw:CARD=UMC1820,DEV=0
     Sample rate: 44100-96000 Hz
     Max channels: 18
```

Then add it to the config's `inputs` list:

```json
"inputs": [
  {
    "device": "plughw:CARD=Headset,DEV=0",
    "voice_id": "gm_mic",
    "volume": 0.9,
    "routes": [
      {"source_channel": 0, "dest_channel": 0},
      {"source_channel": 0, "dest_channel": 1}
    ]
  }
]
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `device` | system default input | Name from `--list-inputs`, or its number there as a string (`"0"`) |
| `routes` | *required* | Which capture channel (from 0) plays on which output channel (number or alias) |
| `voice_id` | `"mic"` | The input's voice, for `voice_volume`, the `input_*` commands and ducking |
| `volume` | `1.0` | Gain, `0.0`–`4.0` |
| `latency_ms` | `20` | Capture buffering, `5`–`500`; see [Latency and clock drift](#latency-and-clock-drift) |
| `channels` | smallest count covering the routes | Capture channels to open |
| `sample_rate` | the output rate | Capture rate to ask for |
| `activity_threshold` | none | Level that makes the input count as active for ducking |
| `activity_hold_ms` | `750` | How long it stays active after the level drops |

On Linux a device name that matches no listed device by name or number is also compared by ALSA
card, so `hw:1,0`, `hw:CARD=UMC1820,DEV=0` and `plughw:CARD=UMC1820,DEV=0` can find the same card.
Prefer the `plughw:` form: ALSA then converts formats the card does not offer natively.

An input that fails to open is logged, reported by `GET /ready` (`503`) and `/status/inputs`
(`"ready": false` with the error), and left out; the rest of the daemon runs normally. Both reflect
startup only: an input that stops later, such as an unplugged USB microphone, logs
`Input stream error: ...` and is not reopened, while `/ready` stays `200`. Restart the daemon after
reconnecting it.

## Routes

Each route sends one capture channel to one output channel, so a microphone can feed several
speakers:

```json
"routes": [
  {"source_channel": 0, "dest_channel": 4},
  {"source_channel": 0, "dest_channel": 5},
  {"source_channel": 0, "dest_channel": 6},
  {"source_channel": 0, "dest_channel": 7}
]
```

Capture channels count from 0, so the first input on an interface's front panel is
`source_channel: 0`. The input opens the smallest channel count the device offers that includes every
routed capture channel; if the device has too few, the input fails to open with
`routing needs N capture channels but device supports at most [...]`. Set `channels` to open a
specific count instead; it must be one the device offers. A route to an output channel the device
does not have is not heard.

## Levels

`volume` is the input's gain: `1.0` passes it through, up to `4.0` (+12 dB) lifts a quiet lavalier.
The output limiter catches peaks, so a large boost is compressed rather than clipped.

At runtime:

```json
{"command": "input_volume", "input": "gm_mic", "volume": 0.6}
{"command": "input_mute", "input": "gm_mic", "mute": true}
{"command": "voice_volume", "voice": "gm_mic", "volume": 0.5}
```

- `input` is the `voice_id` or the input's position in `inputs` (`"0"`), always as a string.
- `input_volume` and `input_mute` fade over 20 ms, or over their `fade_ms` (`0` for instant).
  Unmuting restores the volume the input had when muted, and setting a volume also unmutes.
- `voice_volume` sets a separate voice level that ramps smoothly and multiplies with the input's
  volume, the same as for sounds.

`GET /status/inputs` reports the volume and mute state the audio thread has applied.

## Ducking

An input can duck other voices while someone speaks, and be ducked itself. With an
`activity_threshold` the input counts as active from the moment its level reaches the threshold until
it has stayed below it for `activity_hold_ms`:

```json
"inputs": [
  {
    "device": "plughw:CARD=Headset,DEV=0",
    "voice_id": "gm_mic",
    "activity_threshold": 0.05,
    "routes": [{"source_channel": 0, "dest_channel": 4}, {"source_channel": 0, "dest_channel": 5}]
  }
],
"ducking_rules": [
  {"primary_voice": "gm_mic", "ducked_voices": ["ambience", "music"], "target_volume": 0.1, "fade_duration_ms": 300}
]
```

The level is the peak across the input's routed channels as captured, before its volume, so set the
threshold above the room noise (the config editor's input picker shows live levels). A muted input is
inactive however loud the room is. Without a threshold the input counts as active whenever it is open
and not muted. See [Ducking](ducking.md).

## Latency and clock drift

The input and output devices run on their own clocks, which never agree exactly. Every input
therefore passes through a sample-rate converter whose ratio is steered, within ±2%, by how full the
buffer between capture and output is. That keeps the buffer, and so the delay, steady over sessions of
any length, even when both devices nominally run at the same rate.

`latency_ms` sets the capture buffering. The converter hands over audio in blocks of 1024 frames, so
the buffer must hold at least two: a smaller `latency_ms` is raised to the minimum, with a warning
naming the value used. The minimum depends on both rates: 11 ms with capture and output at 48 kHz,
12 ms with 44.1 kHz capture, 6 ms with 96 kHz capture into a 48 kHz output. The real delay from
microphone to speaker is roughly twice `latency_ms` plus about 11 ms of conversion.

| `latency_ms` | Suits |
|--------------|-------|
| 11–20 | Live speech on a well-behaved machine |
| 20–50 | Most setups |
| 50 and up | Busy or slow machines, where dropouts matter more than delay |

Capture opens at the output's rate when the device offers it; otherwise at the nearest rate it has,
with the warning
`Input device '...' sample rate (44100 Hz) differs from output (48000 Hz) - resampling will add latency`.
The capture buffer size follows `audio.buffer_size`, falling back to the device's default if it is
refused. Capture accepts 32-bit float, 16- and 32-bit integer and unsigned 16-bit samples.

Inputs that name the same device with the same `latency_ms`, `channels` and `sample_rate` share one
capture stream, so several microphones on one interface do not compete to open it.

## Monitoring

`GET /status/inputs` reports running totals for each input:

| Field | Meaning |
|-------|---------|
| `backlog_frames` | Captured audio waiting to be mixed; its steady value is the input's delay |
| `max_backlog_frames` | Backlog above which old audio is discarded |
| `dropped_frames` | Captured audio lost because the buffer was full |
| `trimmed_frames` | Audio discarded to bring the delay back down |
| `underrun_frames` | Output frames that found no captured audio |

A few underrun frames while the buffer fills at startup are normal. Errors inside the capture path are
logged as `Input '...' capture counters moved: ...` and counted in `/metrics` under `input_capture`.

## Talkback

Talkback gives one client at a time a short, renewable lease on a microphone, for a push-to-talk
button in a control panel. The daemon, not the client, closes the microphone again when the lease is
released, expires or is hard-muted, so a crashed panel cannot leave a microphone open.

Name the microphone and the destinations a lease may choose in the
[`talkback`](../configuration.md#talkback) section. Each destination lists output channels, and the
microphone needs a route to every one of them:

```json
"inputs": [
  {
    "voice_id": "gm_mic",
    "routes": [
      {"source_channel": 0, "dest_channel": 4},
      {"source_channel": 0, "dest_channel": 5},
      {"source_channel": 0, "dest_channel": 6},
      {"source_channel": 0, "dest_channel": 7}
    ]
  }
],
"talkback": {
  "input": "gm_mic",
  "destinations": [
    {"name": "GUEST_ALL", "channels": [4, 5, 6, 7]},
    {"name": "ROOM_1", "channels": [4, 5]}
  ]
}
```

A panel then holds the button with:

```json
{"command": "talkback_acquire", "client_id": "panel-1", "destination": "ROOM_1", "gain": 0.0, "lease_ms": 1000}
```

It repeats the acquire before `lease_ms` runs out for as long as the button is held, which renews the
lease and may change its destination and gain, then sends `talkback_release` with its `client_id`.
Another client is refused until the lease ends. Parameters are in
[Commands](../commands.md#talkback).

How the microphone behaves:

- It is muted from startup and opens only during a lease, with the 20 ms default fade. `input_mute`
  and `input_volume` cannot open it without a lease (HTTP `403`); muting it is always allowed.
- During a lease it plays only on the routes to the destination's channels, at the lease's `gain`
  on top of the input's `volume`.
- When the lease ends, the daemon queues the mute at once. If the audio command queue is full, it
  tries again every 20 ms, and `/status/talkback` reports `applied_live: true` until the mute is
  queued.
- A muted input never triggers ducking, so the microphone ducks other voices only during a lease.

## Troubleshooting

See [Troubleshooting: Live inputs](../troubleshooting.md#live-inputs) for the device errors, and:

- **Silence:** check `/status/inputs` for `"ready": true`, a volume above zero and `"muted": false`;
  check the routes point at outputs the device has.
- **`device offers no supported capture format ... use the 'plughw:' alias for this card`:** use the
  `plughw:` name of the same card.
- **Too quiet:** raise `volume` above `1.0`. If `underrun_frames` climbs, the problem is dropouts,
  not level.
- **Choppy:** rising `dropped_frames` or `underrun_frames`. Raise `latency_ms`; check CPU load; and if
  ALSA also logs `underrun occurred`, fix the output side first (a direct `plughw:` device rather
  than a `dmix` chain), since a stalling output backs up capture.
- **`trimmed_frames` rising steadily:** the clocks differ by more than the 2% the converter absorbs,
  which normally means capture runs at a different rate than it reports; see the shared-clock note in
  [Troubleshooting](../troubleshooting.md#live-inputs). Putting capture and playback on one
  interface removes drift entirely.
- **Delay:** lower `latency_ms`, and use a device that offers the output's rate.
