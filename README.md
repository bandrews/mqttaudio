# mqttaudio

mqttaudio is a small daemon that plays sound on a multichannel audio device when told to over MQTT
or HTTP. It was built for escape rooms, museum installations and other shows where a controller
sends cues and the audio has to start on time, every time.

The `v2.1` branch holds the **2.1.0-rc.1** release candidate; `main` stays on the 2.0 line until the
candidate is accepted. See the [release record](docs/releases/v2.1-rc1.md).

## Features

- **Commands over MQTT or HTTP.** JSON commands to play, stop, fade, seek, change speed and volume,
  with an optional [HTTP API](docs/http-api.md) that reports what is playing and streams the log.
- **Multichannel routing.** Send any channel of a file to any output channel, with per-route gain
  and named channels.
- **Voices.** Group sounds, then fade, stop or level the group as one.
- **Ducking.** Turn background voices down automatically while narration, or a live microphone,
  is active.
- **Live inputs.** Mix microphones and line inputs into the output with low latency, even when they
  run on a different clock from the output device.
- **Files and URLs.** WAV, AIFF, FLAC, MP3, Ogg Vorbis, AAC and ALAC, from disk or `http(s)://`.
  Decoded audio is cached in memory within a fixed budget, downloads are cached on disk, and long
  files stream through a small window so a two-hour cue never has to fit in memory.
- **Speed and pitch.** Variable speed, reverse playback, and speed changes that keep their pitch.
- **Bass management.** A crossover that sends the bass of chosen channels to a subwoofer.
- **A safe output.** A limiter keeps the mix below a ceiling; per-channel trims level the speakers.
- **An interactive config editor** (`mqttaudio --configure`) with test tones through each speaker.
- **A [web control app](docs/webui/README.md)** for monitoring and testing.

## Quick start

Build it (Rust 1.88 or newer, plus the platform packages listed in
[Getting Started](docs/getting-started.md#build-it)):

```bash
git clone -b v2.1 https://github.com/bandrews/mqttaudio.git
cd mqttaudio
cargo build --release
```

Find your output device and start the daemon:

```bash
./target/release/mqttaudio --list-devices
./target/release/mqttaudio --server localhost --topic audio/commands --device 'plughw:CARD=HD,DEV=0'
```

On Linux the device is an ALSA ID such as `plughw:CARD=HD,DEV=0`, copied from `--list-devices`;
leave `--device` out to use the system default. With a `plughw:` device, also pass the card's channel
count with `--channels`. Then send it a command:

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/doorbell.wav"}'
```

For a lasting setup, write a config file with `mqttaudio --configure` and run
`mqttaudio --config <file>`.

## A taste of the commands

```bash
# Loop some music in the "music" voice, fading in over two seconds
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/theme.mp3",
  "voice": "music", "volume": 0.5, "loop": true, "fade_in": 2000}'

# Play a stereo effect on outputs 4 and 5
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/door.wav",
  "channel_map": [{"src": 0, "dest": 4}, {"src": 1, "dest": 5}]}'

# Turn the music down, then fade it out
mosquitto_pub -t audio/commands -m '{"command": "voice_volume", "voice": "music", "volume": 0.2}'
mosquitto_pub -t audio/commands -m '{"command": "voice_fade_out", "voice": "music", "time": 3000}'

# Stop everything
mosquitto_pub -t audio/commands -m '{"command": "stopall"}'
```

## Documentation

| Guide | Covers |
|-------|--------|
| [Getting Started](docs/getting-started.md) | Building, a first sound, choosing a device |
| [Commands](docs/commands.md) | Every command and parameter |
| [Configuration](docs/configuration.md) | The config file, command line and environment |
| [HTTP API](docs/http-api.md) | REST routes, authentication, status, WebSockets |
| [Deployment](docs/deployment.md) | systemd, containers, locking it down, monitoring |
| [Troubleshooting](docs/troubleshooting.md) | Symptoms, log messages and fixes |
| [Known Issues](docs/bugs.md) | Open problems and limitations |
| [Changelog](CHANGELOG.md) | What changed in each release |

Feature guides: [Channel Routing](docs/features/channel-routing.md) ·
[Voice Management](docs/features/voice-management.md) · [Ducking](docs/features/ducking.md) ·
[Playback Control](docs/features/playback-control.md) · [Caching](docs/features/caching.md) ·
[Microphone Input](docs/features/microphone-input.md) ·
[Bass Management](docs/features/bass-management.md)

For contributors: [Architecture](docs/architecture.md) · [Contributing](CONTRIBUTING.md) ·
[Web UI](docs/webui/README.md)

## Platforms

Linux (ALSA), macOS (CoreAudio) and Windows (WASAPI).

## License

[MIT License](LICENSE)

## AI Statement

While this tool is human designed, reviewed, tested and maintained, the bulk of core development was performed by Claude Opus 4.5 or later.  

If you prefer a purely human developed alternative, the much simpler 1.0 version hand-built in C++ is still available.  Be aware the legacy version is end-of-life and all further development and maintenance will take place on the 2.0 branch.

## Copyright

Copyright (c) 2016-2026 Mo Fang Heavy Industries LLC. Released under the [MIT License](LICENSE).
