# Getting Started

This guide walks you through installing mqttaudio, running your first commands, and understanding the core concepts.

## Prerequisites

- **Rust 1.70+** — Install from [rustup.rs](https://rustup.rs)
- **MQTT broker** — [Mosquitto](https://mosquitto.org/) is recommended
- **Audio device** — Built-in speakers work, but multichannel setups need appropriate hardware

### Platform-Specific Requirements

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install libasound2-dev libssl-dev pkg-config build-essential clang libclang-dev
```

**macOS and Windows:** No additional dependencies.

## Installation

### From Source

```bash
git clone https://github.com/yourusername/mqttaudio.git
cd mqttaudio
cargo build --release
```

The binary will be at `target/release/mqttaudio`.

### Verify Installation

```bash
./target/release/mqttaudio --version
./target/release/mqttaudio --help
```

## Your First Playback

### Step 1: Start the MQTT Broker

In one terminal:
```bash
mosquitto -v
```

### Step 2: Start mqttaudio

In another terminal:
```bash
./target/release/mqttaudio --server localhost --topic audio/commands
```

You should see:
```
INFO mqttaudio: Connected to MQTT broker at localhost:1883
INFO mqttaudio: Subscribed to topic: audio/commands
INFO mqttaudio: Audio device: Built-in Output (48000 Hz, 2 channels)
```

### Step 3: Play a Sound

In a third terminal:
```bash
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/path/to/your/audio.wav"
}'
```

Replace `/path/to/your/audio.wav` with an actual audio file on your system.

### Step 4: Stop All Audio

```bash
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'
```

## Core Concepts

### Commands

All interaction happens through JSON messages sent to an MQTT topic. Every command has this structure:

```json
{
  "command": "command_name",
  "param1": "value1",
  "param2": "value2"
}
```

Some commands (like `stopall`) don't require any parameters.

### Voices

A **voice** is a named group of sounds you can control together. When you play audio with a voice name, that sound joins that group:

```bash
# Add two sounds to the "ambience" voice
mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/rain.wav",
  "voice": "ambience",
  "loop": true
}'

mosquitto_pub -t audio/commands -m '{
  "command": "play",
  "file": "/sounds/wind.wav",
  "voice": "ambience",
  "loop": true
}'

# Stop both at once
mosquitto_pub -t audio/commands -m '{
  "command": "voice_stop",
  "voice": "ambience"
}'
```

Voices are useful for:
- Stopping all related sounds together
- Fading out a group of sounds
- Adjusting volume for a category of audio

### Channel Routing

By default, audio plays on the first available channels (stereo files play on channels 0 and 1). You can route audio to specific output channels:

```json
{
  "command": "play",
  "file": "/sounds/alert.wav",
  "channel_map": [
    {"src": 0, "dest": 4},
    {"src": 1, "dest": 5}
  ]
}
```

This plays a stereo file on output channels 4 and 5 instead of 0 and 1.

### Caching

When you play HTTP URLs, mqttaudio automatically:
1. Downloads the file
2. Decodes it
3. Caches it for instant playback next time

First play of an HTTP file: 100-300ms latency
Subsequent plays: < 10ms latency

## Example: Background Music with Effects

Here's a typical workflow for an interactive installation:

```bash
TOPIC="audio/commands"

# Start looping background music
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/background-music.mp3",
  "voice": "music",
  "volume": 0.4,
  "loop": true,
  "fade_in": 3000
}'

# Play a one-shot sound effect
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "file": "/sounds/doorbell.wav",
  "voice": "effects",
  "volume": 1.0
}'

# Lower music volume temporarily
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.1
}'

# Restore music volume
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_volume",
  "voice": "music",
  "volume": 0.4
}'

# Fade out music at the end
mosquitto_pub -t $TOPIC -m '{
  "command": "voice_fade_out",
  "voice": "music",
  "time": 5000
}'

# Stop all remaining audio
mosquitto_pub -t $TOPIC -m '{"command": "stopall"}'
```

## Using a Configuration File

For production deployments, use a config file instead of command-line arguments. The easiest way to
create one is the interactive editor:

```bash
./mqttaudio --configure
```

It explains every setting, offers pickers for devices, channels, and voices, validates before saving,
and can play test tones through each speaker. Alternatively, write the file by hand:

**config.json:**
```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": "USB Audio Interface",
    "sample_rate": 48000
  },
  "security": {
    "allowed_directories": ["/opt/sounds"]
  }
}
```

Run with:
```bash
./mqttaudio --config config.json
```

See [Configuration](configuration.md) for all available options.

## Finding Your Audio Device

List available devices:
```bash
./mqttaudio --list-devices
```

Example output:
```
Available audio output devices:
  0. Built-in Output
     Sample rate: 48000 Hz
     Channels: 2
  1. USB Audio Interface
     Sample rate: 48000 Hz
     Channels: 8
```

Use the device name in your config or command line:
```bash
./mqttaudio --device "USB Audio Interface" --topic audio/commands
```

## Next Steps

- [Commands Reference](commands.md) — All available commands
- [Configuration](configuration.md) — Config file options
- [Channel Routing](features/channel-routing.md) — Multichannel setups
- [Audio Ducking](features/ducking.md) — Automatic volume reduction
