# Microphone Input

mqttaudio can capture audio from microphones and other input devices, mixing them into the output with flexible routing. This is ideal for escape rooms, interactive installations, and live events.

## How It Works

1. Configure input devices in your config file
2. mqttaudio captures audio from each input
3. Input audio is routed to specified output channels
4. Input audio integrates with the voice system for ducking

## Configuration

Add inputs to your config file:

```json
{
  "inputs": [
    {
      "device": "USB Microphone",
      "volume": 0.8,
      "voice_id": "presenter_mic",
      "routes": [
        {"source_channel": 0, "dest_channel": 0},
        {"source_channel": 0, "dest_channel": 1}
      ],
      "latency_ms": 25
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `device` | Input device name (use `--list-inputs` to see options) |
| `volume` | Input volume (0.0 to 1.0) |
| `voice_id` | Voice name for ducking integration |
| `routes` | Channel routing (source → destination) |
| `latency_ms` | Buffer latency (5-500ms) |

## Finding Input Devices

List available input devices:

```bash
./mqttaudio --list-inputs
```

Example output:
```
Available audio input devices:
  0. USB Microphone
     Sample rate: 48000 Hz
     Channels: 1
  1. Built-in Microphone
     Sample rate: 44100 Hz
     Channels: 2
```

## Routing

### Mono to Stereo

Route a mono microphone to both left and right outputs:

```json
"routes": [
  {"source_channel": 0, "dest_channel": 0},
  {"source_channel": 0, "dest_channel": 1}
]
```

### Stereo Pass-Through

Route a stereo input directly:

```json
"routes": [
  {"source_channel": 0, "dest_channel": 0},
  {"source_channel": 1, "dest_channel": 1}
]
```

### Broadcast to Multiple Channels

Route a microphone to all player earpiece channels:

```json
"routes": [
  {"source_channel": 0, "dest_channel": 4},
  {"source_channel": 0, "dest_channel": 5},
  {"source_channel": 0, "dest_channel": 6},
  {"source_channel": 0, "dest_channel": 7}
]
```

A `source_channel` must exist on the device (channels are 0-based, so a 2-channel device has channels `0` and `1`). The device's channel count is only known once the input stream opens, so a route reading a non-existent source channel is logged as a warning at startup and that route is dropped:

```
WARN Input 0 routes read source channel(s) [2] but the device has only 2 channel(s) (0..1); those routes will be silently dropped — fix the input's routes config
```

## MQTT Commands

### Adjust Input Volume

```json
{
  "command": "input_volume",
  "input": "presenter_mic",
  "volume": 0.5
}
```

The `input` field can be the `voice_id` or the input index (0, 1, 2...). Setting an explicit volume also takes the input out of any muted state.

### Mute/Unmute

```json
{
  "command": "input_mute",
  "input": "presenter_mic",
  "mute": true
}
```

Muting stores the input's current volume and silences it; unmuting **restores that stored volume** (for example, a calibrated `0.5`), not a fixed `1.0`. So a mute/unmute round-trip leaves the calibrated level intact.

### Voice Volume

An input's `voice_id` is a first-class voice: a `voice_volume` command targeting it ramps the input's level, even when no sample is playing on that voice.

```json
{
  "command": "voice_volume",
  "voice": "presenter_mic",
  "volume": 0.4
}
```

This is separate from `input_volume` (a per-input gain): `voice_volume` is the shared voice-level gain that also scales any samples playing on the same voice.

## Ducking Integration

Microphone inputs work with audio ducking. Set a `voice_id` and use it in ducking rules:

```json
{
  "inputs": [
    {
      "device": "Gamemaster Headset",
      "voice_id": "gm_mic",
      "routes": [{"source_channel": 0, "dest_channel": 4}]
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "gm_mic",
      "ducked_voices": ["ambient", "music"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    }
  ]
}
```

Ambient audio and music duck while the gamemaster microphone's input stream is open. Activation is not yet gated on the microphone's signal level, so the duck holds while the input is running rather than only while the gamemaster is actually speaking; signal-level gating is planned.

## Example: Escape Room

Complete configuration for an escape room with gamemaster microphone:

```json
{
  "mqtt": {
    "topic": "escaperoom/audio"
  },
  "audio": {
    "device": "MOTU 8A",
    "channels": 8
  },
  "inputs": [
    {
      "device": "Gamemaster Headset",
      "volume": 0.9,
      "voice_id": "gm_mic",
      "routes": [
        {"source_channel": 0, "dest_channel": 4},
        {"source_channel": 0, "dest_channel": 5},
        {"source_channel": 0, "dest_channel": 6},
        {"source_channel": 0, "dest_channel": 7}
      ],
      "latency_ms": 25
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "gm_mic",
      "ducked_voices": ["ambient", "effects"],
      "target_volume": 0.1,
      "fade_duration_ms": 500
    }
  ]
}
```

This:
- Captures from the gamemaster's headset microphone
- Routes to player earpiece channels (4-7)
- Ducks ambient audio while the gamemaster microphone is active (its input stream is open)
- Uses low latency (25ms) for natural conversation feel

## Latency

The `latency_ms` setting controls the input buffer size:

| Setting | Latency | Stability |
|---------|---------|-----------|
| 5-15 ms | Very low | May have glitches on slower systems |
| 20-30 ms | Low | Good balance for most systems |
| 50-100 ms | Medium | Very stable, noticeable delay |
| 100-500 ms | High | Maximum stability, significant delay |

For live microphones, use the lowest stable setting (typically 20-30ms).

## Sample Rate and Clock-Drift Handling

An input device and the output device are clocked by independent oscillators. Even when their nominal sample
rates match (e.g. both `48000 Hz`), those clocks tick at very slightly different real-world rates, so over a
long session the input's ring buffer slowly fills or drains until it overflows (a click) or starves (a
dropout). To stay glitch-free over the long-running sessions this daemon is built for, **every input runs
through an asynchronous sample-rate converter**, not just inputs whose rate differs from the output.

The converter's resampling ratio is gently steered from the measured ring-buffer fill toward half-full: if
the ring is trending full the input is producing slightly faster than the output consumes, so the converter
emits marginally fewer frames, and vice versa. The correction authority is a small fraction of a percent —
far more than enough to track real oscillator drift (tens of parts per million) yet small enough to be
inaudible as pitch. The result is a ring-buffer fill that stays bounded indefinitely instead of drifting to a
boundary.

If the input device's nominal rate differs from the output you'll also see a warning that the rate conversion
adds latency:

```
WARN Input device 'USB Microphone' sample rate (44100 Hz) differs from output (48000 Hz) - resampling will add latency
```

For lowest latency, use an input device whose nominal rate matches your output rate; the drift-control
conversion still runs, but with no nominal rate change it only has to correct the small clock difference.

## Troubleshooting

**No audio from microphone:**
- Verify device name matches exactly (`--list-inputs`)
- Check routes are configured correctly
- Check volume is not 0
- Check the device isn't muted via `input_mute`

**Audio is delayed:**
- Reduce `latency_ms`
- Use a device with matching sample rate

**Audio glitches:**
- Increase `latency_ms`
- Check CPU usage
- Reduce number of simultaneous sources

Periodic dropouts caused by clock drift between two independent devices are handled by the always-on
drift-control converter (see *Sample Rate and Clock-Drift Handling*), so recurring glitches every few minutes
on a two-device setup should not occur. Glitches that remain are typically CPU- or latency-related.
