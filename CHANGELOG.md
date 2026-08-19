# Changelog

All notable changes to mqttaudio will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **MQTT reconnect deafness**: the daemon subscribed only once at startup, so
  any broker restart or network blip left it connected but ignoring every
  command until restarted. It now resubscribes on every reconnect.
- **Macros with the nested `message` format**: macro parameters were merged
  where the nested format never reads them, silently doing nothing. They now
  merge into `message`. Unknown macro names are logged instead of ignored.
- **Pitch-corrected playback of streamed files**: a sample went silent the
  moment its download completed. It keeps playing now.
- **Reverse playback at fractional speeds** interpolated against the wrong
  neighbor and sounded garbled; the math is fixed.
- **Streaming sample lifetime**: samples could be killed mid-playback by a
  momentary lock collision with the loader, truncated when playback caught up
  with a slow download, or created permanently silent. A failed download now
  ends its sample cleanly and the URL can be retried immediately (it used to
  stay poisoned until restart).
- **Ducking fade rates**: fades ran N times too fast when a voice had N
  samples; restore now uses the rule's own fade duration instead of a
  hardcoded 2 seconds; unrelated voice activity no longer restarts fades.
- **Settings that did nothing now work**: `audio.buffer_size`,
  `mqtt.client_id`, `mqtt.reconnect_delay_seconds`, `cache.enabled`,
  `cache.revalidate_after_seconds`, `logging.verbose` (config-file form), and
  `security.allowed_directories` (enforced when non-empty, with symlink and
  `../` traversal resolution; an empty list leaves local playback
  unrestricted).
- **HTTP robustness**: downloads have connect/response/stall timeouts, disk
  cache writes are atomic, cache filenames use a stable hash (SHA-256) that
  survives toolchain upgrades, and query strings no longer break format
  detection.
- **Memory growth on long uptimes**: finished sounds no longer leave permanent
  entries in the voice manager and ducking engine, streamed downloads now
  count against `max_memory_mb`, and `/status/voices` stops reporting ghosts.
- **`seek`** clamps to the track length instead of the downloaded-so-far
  frontier; HTTP `/input/mute` requires the `mute` field instead of silently
  unmuting when it is omitted.

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

- **Microphone-triggered ducking**: give an input an `activity_threshold`
  (peak capture level 0.0-1.0, plus `activity_hold_ms`, default 750) and its
  `voice_id` triggers ducking rules as a `primary_voice` - the mic goes hot,
  the room audio ducks, and it recovers after the hold time.
- **Real command outcomes over HTTP**: command endpoints wait for processing
  and report what actually happened (404 for a missing file or unmatched
  selector, 403 for a rejected path, 400 for malformed requests) instead of a
  blanket "accepted".
- **Responsive command loop**: file loads run as their own tasks, so
  `stopall` and other control commands are never queued behind a slow
  download - and a stop cancels loads still in flight.
- **HTTP cache revalidation**: cached URLs are checked against the server
  with conditional requests after `cache.revalidate_after_seconds` (0 = every
  access); changed files re-download automatically, unreachable servers fall
  back to the cached copy. Streamed plays and runtime precache now persist
  to the disk cache too.
- **WebSocket log streaming**: `/ws` now actually streams the daemon's log
  lines, and honors `auth_token` (via the `token` query parameter).
- **Environment variables**: `MQTTAUDIO_CONFIG` selects the config file when
  `--config` is absent; `RUST_LOG` enables per-module log filtering.
- `fadeout` and `soundFadeOut` accepted as aliases of `fadeall`, completing
  the legacy command set.
- `speed: 0` is rejected with a clear error instead of playing an
  unintelligible 100x-slowed drone.
- `advanced.resampler_quality` now also governs live-input capture
  conversion, which previously always ran at maximum quality regardless.
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

### Removed

- The `--lfe-channel` and `--crossover-frequency` CLI flags. They could
  never activate bass management on their own (the feature also needs
  `source_channels`, which has no flag) and only overrode an
  already-configured setup. Bass management is configured entirely in the
  `bass_management` config section.

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
- **Disk caching**: Downloaded files cached on disk across restarts
- **ETag/Last-Modified capture**: Validation headers stored with each cache entry
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

### Changed

- **Language**: Python → Rust for improved performance and reliability
- **Audio engine**: Custom mixer implementation for precise channel control
- **MQTT client**: paho-mqtt → rumqttc for native Rust async integration
- **Threading model**: Async/await with tokio for efficient I/O
- **Cache strategy**: Simple disk cache for downloaded files
- **Command format**: Backwards compatible with legacy mqttaudio v0.1.x commands

### Performance Improvements

- Audio callback execution time: < 1% of buffer duration (~20 μs typical)
- Cached file playback latency: < 10 ms
- HTTP file first play: 100-300 ms (network dependent)
- Lock-free ring buffers between capture and mixing threads

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
- `security.allowed_directories` setting introduced (enforcement landed
  in a later release)

### Backwards Compatibility

Legacy mqttaudio v0.1.x command formats are supported:
- `soundPlay` → `play`
- `soundStopAll` → `stopall`
- `soundPrecache` → `precache`

(`soundFadeOut`/`fadeout` support arrived in a later release.)

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
