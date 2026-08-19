# Changelog

All notable changes to mqttaudio will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Microphone capture on multichannel interfaces**: input devices with more
  than 16 channels lost part of every frame, which rotated the channel routing
  and grew a residue in the ring buffer until it overflowed continuously. All
  capture channels are now readable and routable.
- **Channel alignment under load**: an overrun could write a partial frame into
  the capture ring buffer, permanently shifting which microphone reached which
  speaker. Only whole frames are transferred now, so an overrun costs audio
  rather than correctness.
- **Capture latency drift**: a capture clock faster than the output clock built
  an unbounded backlog. Excess backlog is now trimmed in whole frames.
- **Realtime safety of capture callbacks**: the callbacks no longer allocate,
  resample into freshly allocated buffers, or write log lines, all of which
  stalled the capture thread and caused the overruns they reported.
- **Input device naming**: input devices are now resolved by ALSA card the same
  way output devices are, so `"hw:CARD=UMC1820, DEV=0"` matches the enumerated
  device.
- **Voice volume ramping on live inputs**: a fade no longer stalls while the
  input is starved.
- **`audio.channel_volumes` had no effect**: the per-channel calibration was
  parsed and validated but never applied to the output. It is now applied to
  the finished mix, after bass management.

### Added

- `fadeall` command, fading every playing sample out over a given time and
  stopping it, alongside the existing `stopall`. Available over MQTT
  (`{"command": "fadeall", "time": 2000}`, defaulting to 1000 ms) and as
  `POST /fadeall`.
- Gains above unity. Volume controls now accept up to 4.0 (+12 dB) instead of
  stopping at 1.0, so a quiet microphone, a voice, an individual sample or an
  underpowered subwoofer channel can be lifted rather than only attenuated.
  Applies to `inputs[].volume`, `audio.channel_volumes`, the `play`, `volume`,
  `voice_volume` and `input_volume` commands. The mixer still saturates its
  output, so a boost clips rather than wrapping.
- `audio.channel_volumes` keys may be a channel number, an
  `audio.channel_aliases` name, or an `audio.channel_names` label, and an
  unresolvable key is now a validation error rather than being ignored.
- `inputs[].channels` and `inputs[].sample_rate` to control how a capture
  stream is opened. By default the stream opens with the smallest channel count
  the routes need, at the output sample rate so no resampling is required.
- Input health counters (backlog, overruns, trims, starvation) reported through
  `GET /status/inputs` and logged every 10 seconds when non-zero.
- A clear startup error when a device offers no f32 capture format, naming the
  `plughw:` alias as the fix, and when routing references a channel the device
  cannot reach.

### Changed

- Unknown channel names in routing now explain the `audio.channel_names` /
  `audio.channel_aliases` split. A name defined only in `channel_names` is
  reported with the `channel_aliases` entry needed to fix it, instead of a bare
  "Unknown channel alias".
- `play` volume is clamped to the gain limit; previously it was passed through
  unbounded while every other volume control clamped at 1.0.

## [2.0.0] - 2025-10-19

### Overview

Complete rewrite of mqttaudio in Rust for improved stability, performance, and maintainability. This version is a ground-up reimplementation with backwards compatibility for legacy command formats.

### Added

#### Core Features
- **Polyphonic audio mixing**: Support for playing 20+ simultaneous audio samples
- **Multichannel routing**: Flexible channel mapping with support for 16+ output channels
- **Voice management**: Group samples into named voices for coordinated control
- **Sample rate conversion**: Automatic resampling using high-quality rubato library
- **Audio fading**: Smooth fade-in and fade-out support for samples and voices
- **Format support**: WAV, MP3, OGG/Vorbis, and FLAC via symphonia decoder

#### HTTP and Caching
- **HTTP download support**: Play audio files from http:// and https:// URLs
- **Intelligent disk caching**: Cache downloaded files with configurable revalidation
- **ETag/Last-Modified validation**: Smart cache validation using HTTP standards
- **Precaching command**: Pre-download files for instant playback

#### Configuration
- **JSON configuration files**: Flexible configuration with sensible defaults
- **Multiple config locations**: Support for system, user, and local config files
- **Command-line overrides**: Override any config setting via CLI arguments
- **Channel naming**: Map channel numbers to human-readable names
- **Per-channel volume calibration**: Adjust individual channel volumes

#### MQTT Commands
- `play`: Play audio with full control (volume, routing, fading, looping)
- `stopall`: Stop all audio immediately
- `voice_stop`: Stop all samples in a specific voice
- `voice_fade_out`: Fade out a voice over specified duration
- `voice_volume`: Adjust volume for all samples in a voice
- `fadeout`: Global fade out (legacy compatibility)
- `precache`: Pre-download and decode files
- `cache_clear`: Clear entire cache
- `cache_invalidate`: Invalidate specific cached file

#### Developer Tools
- **Comprehensive test suite**: 104+ unit and integration tests
- **Performance benchmarks**: Criterion-based benchmarks for mixer performance
- **Detailed logging**: Structured logging with configurable levels
- **Device listing**: `--list-devices` to enumerate audio devices

#### Documentation
- Complete architecture documentation in `docs/`
- Command reference with examples
- Configuration guide
- Quick reference cheat sheet
- Implementation roadmap
- Performance tuning guide
- Testing strategy documentation

### Changed

- **Language**: Python → Rust for improved performance and reliability
- **Audio engine**: Custom mixer implementation for precise channel control
- **MQTT client**: paho-mqtt → rumqttc for native Rust async integration
- **Threading model**: Async/await with tokio for efficient I/O
- **Cache strategy**: Simple disk cache with HTTP validation
- **Command format**: Backwards compatible with legacy mqttaudio v0.1.x commands

### Performance Improvements

- Audio callback execution time: < 1% of buffer duration (~20 μs typical)
- Cached file playback latency: < 10 ms
- HTTP file first play: 100-300 ms (network dependent)
- Zero allocations in audio callback (real-time safe)
- Lock-free communication between threads

### Technical Details

#### Dependencies
- `cpal`: Cross-platform audio I/O
- `symphonia`: Professional-grade audio decoding
- `rubato`: High-quality sample rate conversion
- `rumqttc`: Async MQTT client
- `reqwest`: HTTP client with async streaming
- `tokio`: Async runtime
- `serde`/`serde_json`: Configuration and command parsing
- `clap`: CLI argument parsing
- `tracing`: Structured logging

#### Platform Support
- macOS (CoreAudio)
- Linux (ALSA/PulseAudio)
- Windows (WASAPI)

#### Security
- Path traversal protection for local files
- Directory whitelist for file access
- Canonical path resolution with symlink handling

### Backwards Compatibility

Legacy mqttaudio v0.1.x command formats are fully supported:
- `soundPlay` → `play`
- `soundStopAll` → `stopall`
- `soundFadeOut` → `fadeout`
- `soundPrecache` → `precache`

### Known Limitations

- MQTT authentication not yet implemented
- No cache size limits or LRU eviction (manual cache management required)
- Configuration hot-reload not supported (requires restart)
- Single MQTT topic subscription per instance

### Migration from v0.1.x

1. Install Rust toolchain (see README.md)
2. Build with `cargo build --release`
3. Create configuration file (see `config.example.json`)
4. Legacy commands continue to work without changes
5. Consider migrating to new command format for additional features

---

## [0.1.1] - 2021-03-28

### Added
- Initial Python implementation
- Basic MQTT control
- WAV and OGG playback
- Simple stereo mixing
- HTTP download support

### Changed
- JSON formatting improvements
- Dependency installation instructions

---

## [0.1.0] - 2021-03-28

### Added
- Initial release
- Basic MQTT-controlled audio playback
- Support for local audio files
- Simple command structure

---

## Release Notes

### Version 2.0.0 Release Highlights

mqttaudio 2.0.0 represents a complete reimagining of the project in Rust. Key improvements include:

1. **Reliability**: Memory-safe Rust implementation eliminates entire classes of bugs
2. **Performance**: 10-100x faster audio processing with zero-copy architecture
3. **Scalability**: Support for 20+ simultaneous samples vs. 2-3 in v0.1.x
4. **Features**: Multichannel routing, voice grouping, and smooth fading
5. **Maintainability**: Comprehensive test suite and documentation

This version maintains backwards compatibility while adding powerful new features for interactive installations, museums, exhibitions, and entertainment venues.

### Upgrade Considerations

- Requires Rust toolchain for compilation
- Configuration file format is new (but optional)
- Performance characteristics differ significantly
- Local file access requires explicit directory whitelisting

For questions or issues, please refer to the documentation in `docs/` or the README.md file.
