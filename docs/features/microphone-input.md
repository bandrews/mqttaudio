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

## MQTT Commands

### Adjust Input Volume

```json
{
  "command": "input_volume",
  "message": {
    "input": "presenter_mic",
    "volume": 0.5
  }
}
```

The `input` field can be the `voice_id` or the input index (0, 1, 2...).

### Mute/Unmute

```json
{
  "command": "input_mute",
  "message": {
    "input": "presenter_mic",
    "mute": true
  }
}
```

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

Now when the gamemaster speaks, ambient audio and music automatically duck.

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
- Automatically ducks ambient audio when gamemaster speaks
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

## Sample Rate Handling

If the input device sample rate differs from the output, mqttaudio automatically resamples. You'll see a warning in the logs:

```
WARN Input device 'USB Microphone' sample rate (44100 Hz) differs from output (48000 Hz) - resampling will add latency
```

For lowest latency, use an input device that matches your output sample rate.

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
