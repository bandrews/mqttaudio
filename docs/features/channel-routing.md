# Channel Routing

mqttaudio supports flexible multichannel audio routing, allowing you to play audio to any combination of output channels.

## Concepts

### Channels

Audio channels are numbered starting from 0:

| Channel | Typical 5.1 | Typical 7.1 |
|---------|-------------|-------------|
| 0 | Front Left | Front Left |
| 1 | Front Right | Front Right |
| 2 | Center | Center |
| 3 | LFE (Sub) | LFE (Sub) |
| 4 | Rear Left | Side Left |
| 5 | Rear Right | Side Right |
| 6 | — | Rear Left |
| 7 | — | Rear Right |

Your actual channel layout depends on your audio interface and system configuration.

### Default Routing

Without explicit channel mapping:
- **Mono files** play on channel 0
- **Stereo files** play on channels 0 and 1
- **Multichannel files** play on sequential channels (0, 1, 2, 3...)

## Channel Mapping

Use `channel_map` in the play command to route audio to specific outputs:

```json
{
  "command": "play",
  "file": "/sounds/stereo.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5}
  ]
}
```

This plays a stereo file on channels 4 and 5 instead of 0 and 1.

### One-to-Many Routing

Route a single source channel to multiple outputs (useful for PA announcements):

```json
{
  "command": "play",
  "file": "/sounds/announcement.wav",
  "channel_map": [
    {"src": 0, "dest": 0},
    {"src": 0, "dest": 1},
    {"src": 0, "dest": 2},
    {"src": 0, "dest": 3},
    {"src": 0, "dest": 4},
    {"src": 0, "dest": 5}
  ]
}
```

### Partial Routing

You don't need to map all source channels. To play only the left channel of a stereo file:

```json
{
  "command": "play",
  "file": "/sounds/stereo.wav",
  "channel_map": [
    {"src": 0, "dest": 0}
  ]
}
```

### Per-Route Gain

Each route accepts an optional `gain` (default `1.0`, unity). It scales only that route, so one source
channel can reach several outputs at different levels instead of being sent at full level to each:

```json
{
  "command": "play",
  "file": "/sounds/mono.wav",
  "channel_map": [
    {"src": 0, "dest": 0},
    {"src": 0, "dest": 1, "gain": 0.4}
  ]
}
```

This plays the mono source at full level on channel 0 with a quieter copy on channel 1. Values above
`1.0` boost; gains are clamped to `0.0`–`8.0`. A route without `gain` is unity, so existing maps are
unchanged. The same gain tames a downmix that sums several source channels into one destination so the
sum does not clip (see [commands](../commands.md)).

To level-match a speaker for every play rather than per command, set a per-output-channel gain in
`audio.channel_volumes` instead (see [configuration](../configuration.md)).

## Examples

### Surround Sound Installation

Route a 4-channel ambient file to specific speakers:

```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/forest-quad.wav",
  "channel_map": [
    {"src": 0, "dest": 0},
    {"src": 1, "dest": 1},
    {"src": 2, "dest": 4},
    {"src": 3, "dest": 5}
  ],
  "loop": true
}'
```

### Multi-Zone Audio

Play the same mono file to multiple zones:

```bash
# Zone 1: channels 0-1
# Zone 2: channels 2-3
# Zone 3: channels 4-5

mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/announcement.wav",
  "channel_map": [
    {"src": 0, "dest": 0},
    {"src": 0, "dest": 1},
    {"src": 0, "dest": 2},
    {"src": 0, "dest": 3},
    {"src": 0, "dest": 4},
    {"src": 0, "dest": 5}
  ]
}'
```

### Headphone Zone

Route stereo music to a headphone output on channels 6-7:

```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/music.mp3",
  "channel_map": [
    {"src": 0, "dest": 6},
    {"src": 1, "dest": 7}
  ],
  "voice": "headphones",
  "loop": true
}'
```

## Listing Available Channels

To see how many channels your audio device supports:

```bash
./mqttaudio --list-devices
```

Example output:
```
Available audio output devices:
  0. Built-in Output
     Sample rate: 48000 Hz
     Channels: 2
  1. MOTU 8A
     Sample rate: 48000 Hz
     Channels: 8
```

## Tips

1. **Check your device** — Make sure you have enough output channels for your routing plan
2. **Test with simple files first** — Verify routing with mono test tones before complex multichannel content
3. **Use voices for organization** — Group related channel routings by voice for easier control
4. **Document your setup** — Keep notes on which physical speakers are connected to which channels
