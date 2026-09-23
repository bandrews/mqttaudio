# Getting Started

This guide builds mqttaudio, plays a first sound, and introduces the ideas the rest of the
documentation uses.

## Build it

You need:

- **Rust 1.88 or newer**, from [rustup.rs](https://rustup.rs).
- **A C and C++ toolchain with libclang**, for the bundled time-stretching library:
  - Debian/Ubuntu: `sudo apt-get install build-essential clang libclang-dev libasound2-dev libssl-dev pkg-config`
  - Fedora: `sudo dnf install gcc-c++ clang clang-devel alsa-lib-devel openssl-devel pkgconf-pkg-config`
  - Arch: `sudo pacman -S base-devel clang alsa-lib`
  - macOS: the Xcode Command Line Tools (`xcode-select --install`)
  - Windows: the Visual Studio Build Tools (C++ workload) and LLVM; set `LIBCLANG_PATH` if the build
    cannot find `libclang.dll`
- **An MQTT broker** such as [Mosquitto](https://mosquitto.org/), unless you will only use the
  [HTTP API](http-api.md).

```bash
git clone -b v2.1 https://github.com/bandrews/mqttaudio.git
cd mqttaudio
cargo build --release
./target/release/mqttaudio --version
```

The default branch, `main`, still holds 2.0; this documentation describes the `v2.1` release
candidate.

The binary is `target/release/mqttaudio`. The examples below assume it is on your `PATH` or that you
run them from `target/release/`.

## Play a sound

1. Start a broker, if you do not have one running:

   ```bash
   mosquitto -v
   ```

2. Start mqttaudio on the default output device, taking commands from the topic `audio/commands`:

   ```bash
   mqttaudio --server localhost --topic audio/commands
   ```

   The log (on standard output) includes lines like these:

   ```text
   INFO mqttaudio: mqttaudio 2.1.0-rc.1 starting
   INFO mqttaudio: No configuration file found; using built-in defaults and command-line options
   INFO mqttaudio::mqtt::client: Connecting to MQTT broker: localhost:1883
   INFO mqttaudio: Audio device: Built-in Output
   INFO mqttaudio:   Sample rate: 48000 Hz
   INFO mqttaudio:   Channels: 2
   INFO mqttaudio: Ready to receive MQTT commands on topic: audio/commands
   INFO mqttaudio::mqtt::client: MQTT connected
   ```

3. From another terminal, play a file:

   ```bash
   mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/path/to/sound.wav"}'
   ```

4. Stop everything:

   ```bash
   mosquitto_pub -t audio/commands -m '{"command": "stopall"}'
   ```

Nothing comes back over MQTT. If a command does not work, the reason is in the daemon's log; see
[Troubleshooting](troubleshooting.md).

Without a broker, turn on the HTTP server instead and use `curl`:

```bash
mqttaudio --http-port 8080
curl -X POST http://127.0.0.1:8080/play -H "Content-Type: application/json" \
  -d '{"file": "/path/to/sound.wav"}'
```

## Choose the output device

```bash
mqttaudio --list-devices
```

Each usable device is listed with its **Device ID**, a ready-made `CLI` option and a
`Config (audio.device)` line. Copy the ID exactly into `--device` or `audio.device`; the command-line
option wins over the config file. Leave both out to use the system default.

### Linux (ALSA)

Find your card by its **Description**, then start with its `plughw:` entry under **Suggested
Devices**. For a GIGAPort HD+:

```text
=== Suggested Devices ===
  Device ID: plughw:CARD=HD,DEV=0
    Description: GIGAPort HD+, USB Audio
    CLI: --device 'plughw:CARD=HD,DEV=0'
    Config (audio.device): "device": "plughw:CARD=HD,DEV=0"
    Why: Hardware device with automatic format conversion
    Native: 8 ch, 44100 Hz, S16LE
```

The device value is `plughw:CARD=HD,DEV=0`, including the prefix, colon, equals signs and comma. The
description only helps you recognize the card. `Native` reports what the probe found; it is not a
setting, and a device without it may still work.

```bash
mqttaudio --topic audio/commands --device 'plughw:CARD=HD,DEV=0' --channels 8 --sample-rate 44100
```

or in the config:

```json
{"audio": {"device": "plughw:CARD=HD,DEV=0", "channels": 8, "sample_rate": 44100}}
```

ALSA offers several ways into the same card:

| Prefix | Meaning | Use it |
|--------|---------|--------|
| `plughw:` | The card, with automatic format, rate and channel conversion | First choice for a specific card |
| `hw:` | The card directly, without conversion | When the card supports the format mqttaudio opens |
| `default:`, `sysdefault:` | A default defined by the ALSA configuration | Depends on that configuration |
| `dmix:` | Software mixing, to share the card with other programs | When you need sharing |

If a `hw:` device fails with a format error such as `snd_pcm_hw_params_set_format` /
`Invalid argument`, use the `plughw:` form of the same card. A format error means the device was
found but could not be configured; that is different from **Output device not found**. The
[ALSA plugin reference](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm_plugins.html) has the
details.

### macOS and Windows

Device IDs are the devices' names:

```bash
mqttaudio --topic audio/commands --device "USB Audio Interface"
```

## Write a config file

Command-line options cover the basics; everything else lives in a JSON config file. The easiest
way to write one is the interactive editor, which explains each setting, offers pickers for devices,
channels and voices, plays test tones through each speaker, and checks the result before saving:

```bash
mqttaudio --configure --config mqttaudio.json
```

Or write it by hand:

```json
{
  "mqtt": {"server": "localhost", "topic": "audio/commands"},
  "audio": {"device": "plughw:CARD=HD,DEV=0", "channels": 8},
  "security": {"allowed_directories": ["/opt/sounds"]}
}
```

```bash
mqttaudio --config mqttaudio.json
```

A file named `mqttaudio.json` in the working directory is also found without `--config`.
[Configuration](configuration.md) lists every setting.

## Ideas to know

**Sounds and voices.** Each `play` starts a sound. Give it a `voice` to group it with others: the
`voice_stop`, `voice_fade_out` and `voice_volume` commands then act on the whole group, and
[ducking rules](features/ducking.md) turn voices down automatically while another voice plays.

```bash
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/rain.wav", "voice": "ambience", "loop": true}'
mosquitto_pub -t audio/commands -m '{"command": "play", "file": "/opt/sounds/wind.wav", "voice": "ambience", "loop": true}'
mosquitto_pub -t audio/commands -m '{"command": "voice_fade_out", "voice": "ambience", "time": 3000}'
```

**Channels.** A file's channel 0 plays on output 0, channel 1 on output 1, and so on. A
`channel_map` sends each channel wherever you like, and `audio.channel_aliases` lets you name the
outputs. See [Channel Routing](features/channel-routing.md).

```json
{"command": "play", "file": "/opt/sounds/alert.wav", "channel_map": [{"src": 0, "dest": 4}, {"src": 1, "dest": 5}]}
```

**Caching.** Decoded audio stays in memory, so the second play of a file starts at once, and files
from URLs are kept on disk. List files in `cache.precache` (or send `precache`) to have them ready
before their first cue. Long files play through a small window instead of being decoded whole. See
[Caching](features/caching.md).

## Next steps

- [Commands](commands.md): everything you can send
- [Configuration](configuration.md): every setting
- [Deployment](deployment.md): running it as a service
- Feature guides: [Channel Routing](features/channel-routing.md),
  [Voice Management](features/voice-management.md), [Ducking](features/ducking.md),
  [Playback Control](features/playback-control.md), [Caching](features/caching.md),
  [Microphone Input](features/microphone-input.md), [Bass Management](features/bass-management.md)
