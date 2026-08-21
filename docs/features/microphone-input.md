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
| `volume` | Input volume (0.0 to 4.0, unity is 1.0) |
| `voice_id` | Voice name for ducking integration |
| `routes` | Channel routing (source → destination) |
| `latency_ms` | Buffer latency (5-500ms) |
| `channels` | Capture channels to open. Omit to let the routes decide |
| `sample_rate` | Capture rate to request. Omit to match the output rate |

### Channel count

`channels` is normally left out. The stream is opened with the smallest count
the device supports that still covers every `source_channel` in `routes`, so
routing `source_channel: 8` on an 18-in interface opens all the channels needed
to reach it, while a single-mic route stays narrow.

Set it explicitly when a device misreports its capabilities, or when you want a
fixed layout regardless of routing.

### Volume

`volume` is a gain: 1.0 passes the microphone through untouched, below that
attenuates and above that boosts, up to 4.0 (+12 dB). Boosting is how you lift
a quiet lavalier or a preamp that will not go loud enough; the mixer saturates
its output, so too much gain clips rather than wrapping.

### Sample rate

`sample_rate` is normally left out. The capture stream is opened at the output
rate whenever the device supports it, which keeps the resampler out of the
signal path entirely.

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

### ALSA Device Names

On Linux, prefer the `plughw:` alias of a card over `hw:`:

```json
"device": "plughw:CARD=UMC1820,DEV=0"
```

Capture is read as 32-bit float. `plughw:` converts from the card's native
format; a bare `hw:` device that only offers integer formats is rejected at
startup with the format it does offer. `hw:` works where the card exposes a
float format natively.

Names are matched exactly first, then as a `--list-inputs` index, then by ALSA
card, so `"hw:CARD=UMC1820, DEV=0"`, `"hw:1,0"`, and `"0"` (the index from the
listing) can all resolve to the same enumerated device.

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

### Several Microphones on One Interface

A multichannel interface exposes each of its microphone preamps as a source
channel, so one `inputs` entry can carry several microphones at once. Routing
two source channels to the same destination sums them:

```json
{
  "device": "plughw:CARD=UMC1820,DEV=0",
  "voice_id": "room_mics",
  "routes": [
    {"source_channel": 0, "dest_channel": 4},
    {"source_channel": 1, "dest_channel": 4},
    {"source_channel": 2, "dest_channel": 5}
  ]
}
```

Microphones 1 and 2 are mixed together onto output 4; microphone 3 goes to
output 5 on its own. There is no limit on how many source channels an interface
can contribute.

### Several Microphones with Independent Control

Everything in one `inputs` entry shares a volume, a `voice_id` and therefore
one ducking behaviour. For per-microphone control, give each microphone its own
entry:

```json
"inputs": [
  {
    "device": "Gamemaster Headset",
    "voice_id": "gm_mic",
    "volume": 0.9,
    "routes": [{"source_channel": 0, "dest_channel": 4}]
  },
  {
    "device": "Handheld Mic",
    "voice_id": "handheld",
    "volume": 0.7,
    "routes": [{"source_channel": 0, "dest_channel": 4}]
  }
]
```

Both feed output 4 and are summed there, but each can be levelled, muted and
ducked on its own.

Separate entries need separate devices. Two entries naming the same ALSA `hw:`
device will not both open it; route the extra microphones as extra source
channels of a single entry instead.

## MQTT Commands

### Adjust Input Volume

```json
{
  "command": "input_volume",
  "input": "presenter_mic",
  "volume": 0.5
}
```

The `input` field can be the `voice_id` or the input index (0, 1, 2...).

### Mute/Unmute

```json
{
  "command": "input_mute",
  "input": "presenter_mic",
  "mute": true
}
```

## Ducking Integration

A microphone's `voice_id` can be listed in a rule's `ducked_voices`, so
playback (say, a narration voice) can duck the microphone:

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
      "primary_voice": "narration",
      "ducked_voices": ["gm_mic"],
      "target_volume": 0.2,
      "fade_duration_ms": 500
    }
  ]
}
```

A microphone can also be a rule's `primary_voice` when it has activity
detection configured. Set `activity_threshold` on the input (peak level
0.0-1.0 that counts as speaking; try 0.02-0.1) and optionally
`activity_hold_ms` (default 750 - how long activity persists through
pauses):

```json
{
  "inputs": [
    {
      "device": "Gamemaster Headset",
      "voice_id": "gm_mic",
      "activity_threshold": 0.05,
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

Now when the gamemaster speaks, ambient audio and music duck automatically.
Without an `activity_threshold`, an input never triggers rules (it can
still be ducked by them).

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

## Monitoring Input Health

Capture problems are counted per input and reported two ways.

Every 10 seconds, any input that lost or starved audio logs a warning:

```
WARN Input 0 ('gm_mic'): 480 frames overran, 0 trimmed, 0 starved in 10s (backlog 960 frames)
```

`GET /status/inputs` reports the same counters as running totals:

```json
{
  "inputs": [
    {
      "index": 0,
      "voice_id": "gm_mic",
      "volume": 0.9,
      "channels": 2,
      "muted": false,
      "backlog_frames": 480,
      "max_backlog_frames": 1920,
      "dropped_frames": 0,
      "trimmed_frames": 0,
      "underrun_frames": 0
    }
  ]
}
```

| Counter | Meaning |
|---------|---------|
| `backlog_frames` | Captured audio waiting to be mixed. Its steady-state value is your actual input latency |
| `max_backlog_frames` | Ceiling before the backlog is trimmed |
| `dropped_frames` | Capture could not hand audio over because the buffer was full |
| `trimmed_frames` | Backlog discarded to keep latency bounded |
| `underrun_frames` | Output frames with no captured audio available |

A handful of `underrun_frames` at startup is normal while the buffer primes.

## Troubleshooting

**No audio from microphone:**
- Verify device name matches exactly (`--list-inputs`), or use the listing's
  index number as the device name
- Check routes are configured correctly
- Check volume is not 0
- Check the device isn't muted via `input_mute`

**"Input device not found" for a device `--list-inputs` shows:**
- Only devices that can be opened for capture *at that moment* are enumerable,
  so a device can appear in an interactive listing yet be missing when the
  daemon starts. The error lists what was available and, on Linux, reports why
  a direct ALSA capture open of the requested name fails:
  - *busy*: another process holds the capture side - a sound server (PipeWire,
    PulseAudio) or another capture application, including a second copy of the
    daemon. `fuser -v /dev/snd/*` shows the holder
  - *permission denied*: when running as a systemd service, the service user
    must be in the `audio` group (`sudo usermod -aG audio USER`, then restart)
  - *no such device*: the name does not exist; compare against `arecord -L`
- Run the listing in the same context the daemon runs in
  (`sudo -u SERVICE_USER ./mqttaudio --list-inputs`) to see what the daemon
  sees

**Microphone is too quiet:**
- Raise `volume` above 1.0, or send `input_volume` with a value above 1.0
- Check `underrun_frames` is not climbing, which sounds like dropouts rather
  than low level

**Startup fails with "device offers no f32 capture format":**
- Use the `plughw:` alias of the same card instead of `hw:`

**Startup fails with "routing needs N capture channels":**
- The device cannot be opened wide enough to reach a `source_channel` in your
  routes. Check the channel count from `--list-inputs`, and remember source
  channels are 0-indexed: input 1 on the front panel is `source_channel: 0`

**Choppy mic audio, health log shows both overruns and starvation:**
- Check the startup log for "sample rate differs from output - resampling
  will add latency": on a shared-clock interface, capture cannot be forced
  to a rate the card is not running at. When capture opens at the wrong
  rate despite `sample_rate`, something else holds the card at that rate -
  a `dmix`/`dsnoop` definition from asound.conf, a sound server, or another
  application. Free the card and capture follows the output rate
- ALSA underrun messages (`snd_pcm_recover underrun occurred`) mean the
  *output* is stalling; that stalls mixing, which then backs up capture.
  Fix the output side (direct `plughw:`, not a dmix chain) first

**Overruns (`dropped_frames` climbing):**
- The capture thread is producing faster than the mixer consumes. Raise
  `latency_ms` to give the buffer more headroom
- Check CPU usage - a stalled audio callback shows up here first

**Trims (`trimmed_frames` climbing steadily):**
- Input and output clocks are drifting apart, which happens when the microphone
  and the speakers are on different devices. Trimming keeps latency bounded, at
  the cost of an occasional discontinuity. Putting capture and playback on the
  same interface removes the drift

**Audio is delayed:**
- Reduce `latency_ms`
- Use a device with matching sample rate
- Check `backlog_frames` - a steadily large backlog is added latency

**Audio glitches:**
- Increase `latency_ms`
- Check CPU usage
- Reduce number of simultaneous sources
