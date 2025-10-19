# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

mqttaudio is an MQTT-controlled command line audio player designed for interactive entertainment systems on a budget. It's a C++ application that listens to MQTT topics and plays audio files (local or remote) in response to JSON commands.

**Target Platform**: Linux (tested on Raspberry Pi/Raspbian)

## Building and Dependencies

### Install Dependencies
```bash
sudo make install-dependencies
```

This installs: SDL2, libSDL2-mixer, librt, libmosquitto (MQTT client), libasound (ALSA), libcurl

### Build
```bash
make mqttaudio
```

The build command compiles all source files together in a single g++ invocation, linking against SDL2, SDL2_mixer, mosquitto, ALSA, and curl libraries.

## Running

### List ALSA Devices
```bash
./mqttaudio --list-devices
```

### Basic Usage
```bash
./mqttaudio --server localhost --port 1883 --topic audio/commands --alsa-device yourdevicename
```

Only `--topic` is required; other switches are optional.

Additional options:
- `--preload <url>`: Preload audio file on startup
- `--uri-prefix <prefix>`: Prepend prefix to all file paths
- `--verbose`: Enable logging
- `--frequency <hz>`: Set audio frequency (default 44100)

## Architecture

### Core Components

1. **mqttaudio.cpp**: Main application logic
   - MQTT connection and message handling (mosquitto library)
   - JSON command parsing (RapidJSON)
   - SDL2 audio initialization and playback control
   - Command-line argument parsing (argp)
   - Signal handling for graceful shutdown

2. **Sample** (sample.h/cpp): Audio file wrapper
   - Loads audio files from local filesystem or HTTP/HTTPS URLs
   - Wraps SDL_mixer's Mix_Chunk
   - Handles both local files (Mix_LoadWAV) and remote files (via SDL_RWFromHttpSync)

3. **SampleManager** (samplemanager.h/cpp): Audio caching system
   - Manages cache of loaded audio samples in an unordered_map
   - Prevents re-loading the same file multiple times
   - **Note**: No cache eviction logic exists; cache grows unbounded

4. **SDL_rwhttp** (SDL_rwhttp.c/h): HTTP/HTTPS streaming
   - Third-party library for downloading remote audio files
   - Uses libcurl to fetch audio over HTTP
   - Creates SDL_RWops interface for SDL_mixer

5. **alsautil.h**: ALSA device enumeration
   - Header-only utility for listing available ALSA PCM devices
   - Used by `--list-devices` option

### MQTT Command Format

All commands are JSON messages published to the subscribed topic:

**Play audio**:
```json
{"command": "play", "message": {"file": "http://example.com/audio.wav", "loop": true, "volume": 0.75, "exclusive": false, "maxPlayLength": 60000}}
```

**Stop all audio**:
```json
{"command": "stopall"}
```

**Fade out**:
```json
{"command": "fadeout", "message": {"time": 10000}}
```

**Precache file**:
```json
{"command": "precache", "message": {"file": "http://example.com/file.wav"}}
```

### Audio Playback Flow

1. MQTT message received → `message_callback()` in mqttaudio.cpp:252
2. JSON parsed → `processCommand()` in mqttaudio.cpp:141
3. For "play" command → `playSample()` in mqttaudio.cpp:103
4. Sample loaded/cached → `SampleManager::GetSample()` in samplemanager.cpp:3
5. If not cached, create new `Sample` → loads via `Sample::initSample()` in sample.cpp:16
6. Mix_PlayChannelTimed() plays the audio on an available channel (up to 16 channels allocated)

### Key Details

- **SDL2 Audio**: Configured for 44.1kHz, 16-bit stereo, 512-byte buffer, 16 mixing channels
- **Supported Formats**: WAV, OGG (MP3 and MOD initialized but untested)
- **Remote Files**: HTTP/HTTPS URLs are downloaded synchronously on first play and cached in memory
- **Multi-channel Setup**: Can run multiple instances targeting different ALSA devices for multi-zone audio
- **Reconnection**: Automatically reconnects to MQTT broker with 10-second retry delay
- **Graceful Shutdown**: SIGINT/SIGTERM handlers stop MQTT loop and clean up resources

## Security Considerations

- No validation on local file paths; designed for closed-loop trusted networks only
- No MQTT authentication support currently
- Unbounded cache: playing many different files will exhaust memory
- Designed for pre-determined, limited set of audio files in entertainment/exhibition contexts

## Third-Party Dependencies

- **RapidJSON**: JSON parser (Tencent, MIT license) - vendored in rapidjson/
- **SDL_rwhttp**: HTTP streaming for SDL (mgerhardy, zlib license) - vendored as SDL_rwhttp.c/h
