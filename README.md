# mqttaudio

**MQTT-controlled multichannel audio player for interactive entertainment**

mqttaudio is a high-performance, real-time audio engine that receives commands over MQTT to play sounds with precise multichannel routing, voice grouping, and smooth fading. Built in Rust for rock-solid stability and minimal latency.

## Features

- **Polyphonic Mixing**: Play multiple audio files simultaneously
- **Multichannel Routing**: Route audio to specific output channels (supports up to 16+ channels)
- **Voice Grouping**: Group sounds together for coordinated control
- **Sample Targeting**: Control individual sounds by ID, filename, or voice (seek, speed, stop, volume)
- **Variable Speed Playback**: Change playback speed (0.1x to 4.0x) with optional pitch correction (time-stretching)
- **Seek Control**: Jump to any position in a playing sample
- **Audio Ducking**: Automatically reduce background audio when foreground voices play
- **Bass Management**: Route low frequencies to subwoofer (LFE) channel with configurable crossover
- **Microphone Input**: Mix live audio inputs with matrix routing to output channels
- **Smooth Fading**: Fade in/out individual samples or entire voices
- **HTTP Caching**: Automatically cache remote audio files for instant playback
- **Format Support**: WAV, MP3, OGG, FLAC via symphonia decoder
- **Sample Rate Conversion**: Automatic resampling to match output device (inputs and files)
- **Low Latency**: Sub-millisecond mixing performance, handles 20+ simultaneous sounds
- **Zero-Copy Architecture**: Efficient memory usage with Arc-based buffer sharing

## Installation

### Prerequisites

- Rust 1.70+ (install from [rustup.rs](https://rustup.rs))
- Audio system: ALSA (Linux), CoreAudio (macOS), or WASAPI (Windows)
- MQTT broker (e.g., Mosquitto)

### From Source

```bash
git clone https://github.com/yourusername/mqttaudio.git
cd mqttaudio
cargo build --release
```

The binary will be at `target/release/mqttaudio`.

### System Dependencies

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install libasound2-dev
```

**macOS:**
No additional dependencies needed (CoreAudio is built-in).

**Windows:**
No additional dependencies needed (WASAPI is built-in).

## Quick Start

### 1. Start an MQTT Broker

```bash
# Using Mosquitto
mosquitto -v
```

### 2. Run mqttaudio

```bash
# Basic usage with defaults
mqttaudio --server localhost --topic audio/commands

# With custom audio device (first list available devices)
mqttaudio --list-devices
mqttaudio --server localhost --topic audio/commands --device "Your Device Name"

# With config file (recommended for production)
mqttaudio --config config.json
```

### 3. Send Your First Command

Open another terminal and send a test command:

```bash
# Play a simple audio file
mosquitto_pub -t audio/commands -m '{"command": "play", "message": {"file": "/path/to/sound.wav"}}'
```

You should hear the audio play immediately!

## Common Usage Examples

Here are ready-to-use examples for common tasks. Just copy and paste these commands (replacing file paths as needed).

### Playing Local Audio Files

**Simple playback:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/Users/you/Music/doorbell.wav"
  }
}'
```

**With volume control (50%):**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/Users/you/Music/notification.wav",
    "volume": 0.5
  }
}'
```

**Looping background music:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/Users/you/Music/ambient.ogg",
    "loop": true,
    "voice": "background"
  }
}'
```

### Playing Remote Audio (HTTP/HTTPS)

**Play from URL:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "https://example.com/sounds/welcome.mp3"
  }
}'
```

**Pre-cache a file for instant playback later:**
```bash
# First, cache it
mosquitto_pub -t audio/commands -m '{
  "command": "precache",
  "message": {
    "file": "https://example.com/sounds/large-file.wav"
  }
}'

# Later, play it instantly (served from cache)
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "https://example.com/sounds/large-file.wav"
  }
}'
```

### Fading and Volume Control

**Fade in over 2 seconds:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/path/to/music.mp3",
    "fade_in": 2000,
    "voice": "music"
  }
}'
```

**Fade out a voice:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "voice_fade_out",
  "message": {
    "voice": "music",
    "time": 3000
  }
}'
```

**Adjust voice volume on the fly:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "voice_volume",
  "message": {
    "voice": "background",
    "volume": 0.3
  }
}'
```

### Audio Ducking

Audio ducking automatically reduces the volume of background voices when foreground voices are playing. This is essential for applications like museums, interactive installations, or any scenario where speech needs to be intelligible over background music or effects.

**How It Works:**
- Define ducking rules that specify which voices (primary) cause other voices (ducked) to reduce in volume
- When a primary voice starts playing, ducked voices smoothly fade down to the target volume
- When the primary voice stops, ducked voices restore to their original volume
- Multiple rules can apply simultaneously (the system uses the lowest volume and fastest fade)
- All fades are glitch-free, even when rules change mid-fade

**Configuration:**

Add ducking rules to your `config.json`:

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "My Audio Interface",
    "sample_rate": 48000
  },
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    },
    {
      "primary_voice": "dialog",
      "ducked_voices": ["music", "effects", "narration"],
      "target_volume": 0.05,
      "fade_duration_ms": 1000
    }
  ]
}
```

**Configuration Parameters:**
- `primary_voice`: Voice that triggers ducking
- `ducked_voices`: Array of voices that should be reduced in volume
- `target_volume`: Volume multiplier (0.0 to 1.0) to fade ducked voices to
- `fade_duration_ms`: Duration of fade in milliseconds

**Example Usage:**

```bash
# Start mqttaudio with ducking configuration
mqttaudio --config ducking_config.json

# In another terminal, start background music
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/background-music.mp3",
    "voice": "music",
    "volume": 0.7,
    "loop": true
  }
}'

# Play narration - music will automatically duck to 15% over 2 seconds
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/tour-narration.mp3",
    "voice": "narration",
    "volume": 0.9
  }
}'

# When narration finishes, music will restore to 70% over 2 seconds
```

**Testing Ducking:**

A comprehensive demo script is included to test ducking with tone files:

```bash
# Run the ducking demo (listens for smooth fading)
./demo_ducking.sh
```

The demo showcases:
1. Simple ducking (narration reduces music volume)
2. Multiple rules (dialog reduces music even more during narration)
3. Multiple ducked voices (both music and effects duck together)
4. Rapid transitions (stress test for smooth fading)

**Advanced Scenarios:**

Ducking priority hierarchy:
```json
{
  "ducking_rules": [
    {
      "primary_voice": "announcement",
      "ducked_voices": ["music", "effects", "narration"],
      "target_volume": 0.02,
      "fade_duration_ms": 500
    },
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ]
}
```

This creates a 3-tier priority system:
- **Announcements** (highest priority): Ducks everything to 2%
- **Narration** (medium priority): Ducks music and effects to 15%
- **Music/Effects** (lowest priority): Never trigger ducking

### Bass Management (LFE/Subwoofer)

Bass management extracts low frequencies from designated channels and routes them to a subwoofer/LFE channel. This is essential for installations with separate subwoofers or professional sound systems that require bass redirection.

**How It Works:**
- Low-pass and high-pass Butterworth filters split audio at the crossover frequency
- Bass (below crossover) is extracted and summed to the LFE channel
- Optionally, bass can be removed from source channels (true bass management)
- Uses biquad filters for efficient, real-time processing

**Configuration:**

Add bass management to your `config.json`:

```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "audio/commands"
  },
  "audio": {
    "device": "My 8-Channel Interface",
    "sample_rate": 48000
  },
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2],
    "remove_bass_from_sources": false
  }
}
```

**Configuration Parameters:**
- `enabled`: Enable/disable bass management
- `lfe_channel`: Output channel index for the subwoofer (0-indexed)
- `crossover_frequency_hz`: Frequency cutoff in Hz (typically 80-120 Hz)
- `source_channels`: Array of channel indices to extract bass from
- `remove_bass_from_sources`: If true, removes bass from source channels after extraction (full bass management); if false, bass is copied to LFE but left in source channels

**CLI Options:**

Override config file settings from the command line:

```bash
# Set LFE channel (0-indexed)
mqttaudio --lfe-channel 5 --crossover-frequency 100

# Example: Route bass from front L/R to channel 5 (subwoofer)
mqttaudio --config config.json --lfe-channel 5
```

**Use Cases:**

1. **Home Theater 5.1**: Route bass from channels 0-4 to channel 3 (LFE)
2. **Professional Installation**: Extract bass from all main speakers to dedicated subwoofer zone
3. **Multi-zone Audio**: Send bass content to specific zones with subwoofers

**Example Configuration for 5.1 System:**

```json
{
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2, 4, 5],
    "remove_bass_from_sources": true
  }
}
```

This routes bass from Front L (0), Front R (1), Center (2), Surround L (4), and Surround R (5) to the LFE channel (3), and removes bass from those channels (as most 5.1 receivers expect).

### Microphone Input Mixing

mqttaudio can capture audio from microphone inputs and mix them into the output with flexible routing. This is ideal for scenarios like escape rooms, interactive installations, or live event systems where microphone audio needs to be routed to specific output channels.

**How It Works:**
- Captures audio from one or more input devices
- Automatically resamples if input and output sample rates differ
- Routes input channels to output channels via configurable matrix routing
- Integrates with the voice system for ducking support
- Lock-free ring buffers ensure glitch-free, low-latency operation

**Configuration:**

Add inputs to your `config.json`:

```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "audio/commands"
  },
  "audio": {
    "device": "My 8-Channel Interface",
    "sample_rate": 48000
  },
  "inputs": [
    {
      "device": "USB Microphone",
      "volume": 0.8,
      "voice_id": "gamemaster_mic",
      "routes": [
        {"source_channel": 0, "dest_channel": 4},
        {"source_channel": 0, "dest_channel": 5}
      ],
      "latency_ms": 30
    },
    {
      "device": "Built-in Microphone",
      "volume": 1.0,
      "voice_id": "player_mic",
      "routes": [
        {"source_channel": 0, "dest_channel": 6}
      ],
      "latency_ms": 20
    }
  ]
}
```

**Configuration Parameters:**
- `device`: Input device name (use `--list-inputs` to see available devices)
- `volume`: Input volume multiplier (0.0 to 1.0)
- `voice_id`: Voice name for ducking integration
- `routes`: Array of source→destination channel mappings
- `latency_ms`: Buffer latency in milliseconds (5-500, lower = less latency, higher = more stability)

**CLI Options:**

```bash
# List available input devices
mqttaudio --list-inputs

# Output:
# Available audio input devices:
#   0. USB Microphone
#      Sample rate: 48000 Hz
#      Channels: 1
#   1. Built-in Microphone
#      Sample rate: 44100 Hz
#      Channels: 2
```

**MQTT Commands for Input Control:**

**Adjust input volume:**
```json
{
  "command": "input_volume",
  "message": {
    "input": "gamemaster_mic",
    "volume": 0.5
  }
}
```

The `input` field can be either the voice_id or the input index (0-based).

**Mute/unmute an input:**
```json
{
  "command": "input_mute",
  "message": {
    "input": "0",
    "mute": true
  }
}
```

**Use Cases:**

1. **Escape Room**: Route gamemaster microphone to player earpieces (channels 4-5)
2. **Interactive Installation**: Capture visitor microphones for processing
3. **Live Events**: Mix multiple microphones to specific output zones

**Example: Escape Room Setup**

```json
{
  "audio": {
    "device": "MOTU 8A"
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

This routes the gamemaster's voice to all player earpiece channels (4-7) and automatically ducks ambient audio when the gamemaster speaks.

**Sample Rate Handling:**

If the input device sample rate differs from the output, mqttaudio automatically resamples using high-quality interpolation. A warning is logged when resampling occurs:

```
WARN Input device 'USB Microphone' sample rate (44100 Hz) differs from output (48000 Hz) - resampling will add latency
```

### Voice Grouping (Controlling Multiple Sounds Together)

Voices let you group related sounds and control them together.

**Start background ambience:**
```bash
# Add rain sound to "ambience" voice
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/rain.wav",
    "voice": "ambience",
    "loop": true,
    "volume": 0.4
  }
}'

# Add wind sound to the same voice
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/wind.wav",
    "voice": "ambience",
    "loop": true,
    "volume": 0.3
  }
}'
```

**Stop all sounds in the "ambience" voice:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "voice_stop",
  "message": {
    "voice": "ambience"
  }
}'
```

### Multichannel Routing

Route audio to specific output channels (great for surround sound or multi-zone installations).

**Play stereo file on channels 2-3 instead of 0-1:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/stereo.wav",
    "channel_map": [
      {"src": 0, "dest": 2},
      {"src": 1, "dest": 3}
    ]
  }
}'
```

**Play quad audio to rear speakers (channels 4-7):**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/surround-quad.wav",
    "channel_map": [
      {"src": 0, "dest": 4},
      {"src": 1, "dest": 5},
      {"src": 2, "dest": 6},
      {"src": 3, "dest": 7}
    ]
  }
}'
```

**Play mono PA announcement to all speakers (one-to-many routing):**
```bash
# Route a mono file to channels 0, 1, 2, 3, 4, and 5 simultaneously
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "message": {
    "file": "/sounds/announcement.wav",
    "channel_map": [
      {"src": 0, "dest": 0},
      {"src": 0, "dest": 1},
      {"src": 0, "dest": 2},
      {"src": 0, "dest": 3},
      {"src": 0, "dest": 4},
      {"src": 0, "dest": 5}
    ],
    "voice": "announcements"
  }
}'
```

**Note:** You can map a single source channel to multiple output channels by specifying multiple entries with the same `src` value. This is useful for broadcasting announcements across multiple zones or speakers.

### Stopping Audio

**Stop all audio immediately:**
```bash
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'
```

**Stop a specific voice:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "voice_stop",
  "message": {
    "voice": "effects"
  }
}'
```

**Fade out everything over 5 seconds:**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "fadeout",
  "message": {
    "time": 5000
  }
}'
```

### Cache Management

**Clear entire cache:**
```bash
mosquitto_pub -t audio/commands -m '{"command": "cache_clear"}'
```

**Invalidate a specific cached file (force re-download):**
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "cache_invalidate",
  "message": {
    "file": "https://example.com/sounds/updated-file.mp3"
  }
}'
```

## Complete Usage Scenario

Here's a complete example showing how you might use mqttaudio for an interactive installation:

```bash
# Terminal 1: Start mqttaudio
mqttaudio --server localhost --topic gallery/audio

# Terminal 2: Control the installation
TOPIC="gallery/audio"

# Pre-cache all assets for instant playback
mosquitto_pub -t $TOPIC -m '{"command": "precache", "message": {"file": "/sounds/ambient-forest.ogg"}}'
mosquitto_pub -t $TOPIC -m '{"command": "precache", "message": {"file": "/sounds/bird-chirp.wav"}}'
mosquitto_pub -t $TOPIC -m '{"command": "precache", "message": {"file": "/sounds/narration.mp3"}}'

# Start ambient background (looping, low volume)
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "message": {
    "file": "/sounds/ambient-forest.ogg",
    "voice": "ambience",
    "loop": true,
    "volume": 0.2,
    "fade_in": 3000
  }
}'

# Play a one-shot sound effect when visitor approaches
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "message": {
    "file": "/sounds/bird-chirp.wav",
    "voice": "effects",
    "volume": 0.8
  }
}'

# Duck the ambience and play narration
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "message": {
    "voice": "ambience",
    "volume": 0.05
  }
}'

mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "message": {
    "file": "/sounds/narration.mp3",
    "voice": "narration",
    "volume": 0.9
  }
}'

# Wait for narration to finish, then restore ambience volume
sleep 30
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "message": {
    "voice": "ambience",
    "volume": 0.2
  }
}'

# When closing: fade out everything
mosquitto_pub -t $TOPIC -m '{
  "command": "fadeout",
  "message": {
    "time": 5000
  }
}'
```

## Configuration

### Command Line Options

```
mqttaudio [OPTIONS]

OPTIONS:
  --server <HOST>              MQTT server hostname [default: localhost]
  --port <PORT>                MQTT server port [default: 1883]
  --topic <TOPIC>              MQTT topic to subscribe to [default: audio/commands]
  --device <NAME>              Audio output device name
  --list-devices               List available audio output devices and exit
  --list-inputs                List available audio input devices and exit
  --sample-rate <RATE>         Output sample rate [default: 48000]
  --buffer-size <SIZE>         Audio buffer size in frames [default: 512]
  --channels <COUNT>           Number of output channels [default: auto-detect]
  --lfe-channel <INDEX>        LFE (subwoofer) channel index for bass management
  --crossover-frequency <HZ>   Crossover frequency for bass management [default: 80]
  --cache-dir <PATH>           HTTP cache directory [default: ~/.cache/mqttaudio]
  --config <FILE>              Load configuration from JSON file
  --verbose                    Enable verbose logging (debug level)
  --help                       Print help information
  --version                    Print version information
```

### Configuration File

Create `config.json`:

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "My Audio Interface",
    "sample_rate": 48000,
    "buffer_size": 512,
    "channels": 8
  },
  "cache": {
    "directory": "~/.cache/mqttaudio",
    "max_memory_mb": 500
  },
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2, 4, 5],
    "remove_bass_from_sources": false
  },
  "inputs": [
    {
      "device": "USB Microphone",
      "volume": 0.8,
      "voice_id": "mic_1",
      "routes": [
        {"source_channel": 0, "dest_channel": 4},
        {"source_channel": 0, "dest_channel": 5}
      ],
      "latency_ms": 25
    }
  ],
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 2000
    }
  ],
  "security": {
    "allowed_directories": [
      "/opt/sounds",
      "/home/user/audio"
    ]
  },
  "logging": {
    "level": "info"
  }
}
```

Then run:
```bash
mqttaudio --config config.json
```

## MQTT Commands

All commands are JSON objects sent to the configured MQTT topic.

### Play Audio

```json
{
  "command": "play",
  "message": {
    "file": "http://example.com/audio.wav",
    "id": "background-track-1",
    "voice": "effects",
    "channel_map": [
      {"src": 0, "dest": 2},
      {"src": 1, "dest": 3}
    ],
    "volume": 0.8,
    "loop": false,
    "fade_in": 1000,
    "start_position_ms": 30000,
    "max_play_length": 30000
  }
}
```

**Parameters:**
- `file` (required): URL (http/https) or local file path
- `id` (optional): Unique identifier for this playback instance (for later targeting with seek/speed/stop/volume commands)
- `voice` (optional): Voice name for grouping
- `channel_map` (optional): Array of `{"src": N, "dest": M}` channel routes
- `volume` (optional): 0.0 to 1.0, default 1.0
- `loop` (optional): Loop playback, default false
- `fade_in` (optional): Fade in duration in milliseconds
- `start_position_ms` (optional): Start playback at this offset in milliseconds
- `max_play_length` (optional): Maximum playback duration in milliseconds

### Sample Targeting Commands

These commands let you control specific playing samples by id, file, or voice.

**Seek to position:**
```json
{
  "command": "seek",
  "message": {
    "id": "background-track-1",
    "position_ms": 60000
  }
}
```

**Change playback speed:**
```json
{
  "command": "speed",
  "message": {
    "id": "background-track-1",
    "speed": 1.5,
    "pitch_correction": false
  }
}
```

Speed range: 0.1 to 4.0. When `pitch_correction` is false (default), faster playback = higher pitch ("chipmunk effect"). When `pitch_correction` is true, the audio is time-stretched so pitch remains constant regardless of speed. This uses the signalsmith-stretch library for high-quality real-time time-stretching.

**Stop specific samples:**
```json
{
  "command": "stop",
  "message": {
    "file": "music.mp3",
    "fade_out_ms": 500
  }
}
```

**Adjust sample volume:**
```json
{
  "command": "volume",
  "message": {
    "voice": "effects",
    "volume": 0.3
  }
}
```

**Targeting options** (use any combination):
- `id`: Target specific sample by its user-provided ID
- `file`: Target all samples playing this file
- `voice`: Target all samples in this voice

Multiple selectors use OR logic (matches if ANY selector matches).

### Stop All Audio

```json
{
  "command": "stopall"
}
```

### Voice Commands

**Stop a voice:**
```json
{
  "command": "voice_stop",
  "message": {
    "voice": "background"
  }
}
```

**Fade out a voice:**
```json
{
  "command": "voice_fade_out",
  "message": {
    "voice": "background",
    "time": 3000
  }
}
```

**Adjust voice volume:**
```json
{
  "command": "voice_volume",
  "message": {
    "voice": "music",
    "volume": 0.5
  }
}
```

### Cache Commands

**Precache a file:**
```json
{
  "command": "precache",
  "message": {
    "file": "http://example.com/bigfile.wav"
  }
}
```

**Clear entire cache:**
```json
{
  "command": "cache_clear"
}
```

**Invalidate specific file:**
```json
{
  "command": "cache_invalidate",
  "message": {
    "file": "http://example.com/updated.wav"
  }
}
```

### Input Commands

**Adjust input volume:**
```json
{
  "command": "input_volume",
  "message": {
    "input": "gamemaster_mic",
    "volume": 0.5
  }
}
```

The `input` field can be the voice_id or input index (0-based).

**Mute/unmute an input:**
```json
{
  "command": "input_mute",
  "message": {
    "input": "0",
    "mute": true
  }
}
```

### Global Fade Out

```json
{
  "command": "fadeout",
  "message": {
    "time": 5000
  }
}
```

## Performance

mqttaudio is designed for real-time audio with strict latency requirements:

- **Audio callback time**: < 1% of buffer duration (typically ~20 μs for 10 samples)
- **Max simultaneous samples**: 20+ without glitches
- **Latency (cached files)**: < 10 ms
- **Latency (HTTP files, first play)**: 100-300 ms (network dependent)
- **Memory**: Efficient Arc-based sharing, minimal allocations in audio thread

Run benchmarks:
```bash
cargo bench
```

## Development

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test mixer

# With output
cargo test -- --nocapture
```

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# With verbose output
cargo build --verbose
```

### Project Structure

```
mqttaudio/
├── src/
│   ├── main.rs              # Entry point, CLI handling
│   ├── config.rs            # Configuration loading
│   ├── audio/
│   │   ├── engine.rs        # Audio engine coordinator
│   │   ├── mixer.rs         # Real-time mixer (audio callback)
│   │   ├── ducking.rs       # Audio ducking engine
│   │   ├── bass_management.rs # LFE/subwoofer crossover filtering
│   │   ├── pitch_correction.rs # Time-stretching for pitch-corrected speed change
│   │   ├── input.rs         # Microphone/input device handling
│   │   ├── decoder.rs       # Audio file decoding
│   │   ├── resampler.rs     # Sample rate conversion
│   │   └── types.rs         # Audio data types
│   ├── mqtt/
│   │   ├── client.rs        # MQTT connection
│   │   └── commands.rs      # Command parsing
│   ├── cache/
│   │   ├── disk.rs          # Disk cache implementation
│   │   └── memory.rs        # Memory cache
│   └── voice.rs             # Voice management
├── benches/
│   └── mixer_benchmark.rs   # Performance benchmarks
├── docs/                    # Architecture documentation
├── tests/
│   └── audio/               # Test audio files
└── legacy/                  # Original Python implementation
```

## Troubleshooting

### No Audio Output

1. List available devices:
   ```bash
   mqttaudio --list-devices
   ```

2. Specify device explicitly:
   ```bash
   mqttaudio --device "Your Device Name"
   ```

3. Check sample rate compatibility:
   ```bash
   mqttaudio --sample-rate 44100
   ```

### MQTT Connection Issues

1. Verify broker is running:
   ```bash
   mosquitto_sub -t audio/commands -v
   ```

2. Check server/port:
   ```bash
   mqttaudio --server mqtt.example.com --port 1883
   ```

3. Enable verbose logging:
   ```bash
   mqttaudio --verbose
   ```

### HTTP Download Failures

1. Check network connectivity
2. Verify URL is accessible
3. Check cache directory permissions:
   ```bash
   ls -la ~/.cache/mqttaudio
   ```

### Audio Glitches

1. Increase buffer size (trades latency for stability):
   ```bash
   mqttaudio --buffer-size 1024
   ```

2. Check CPU usage and reduce simultaneous samples
3. Run benchmarks to verify performance

## License

MIT

## Credits

Built with:
- [cpal](https://github.com/RustAudio/cpal) - Cross-platform audio I/O
- [symphonia](https://github.com/pdeljanov/Symphonia) - Audio decoding
- [rubato](https://github.com/HEnquist/rubato) - Sample rate conversion
- [signalsmith-stretch](https://signalsmith-audio.co.uk/code/stretch/) - Time-stretching for pitch correction
- [rumqttc](https://github.com/bytebeamio/rumqtt) - MQTT client

## Contributing

Contributions welcome! Please:
1. Run tests: `cargo test`
2. Run benchmarks: `cargo bench`
3. Format code: `cargo fmt`
4. Check lints: `cargo clippy`

See `docs/` for architecture details and development guidelines.
