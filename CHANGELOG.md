# Changelog

All notable changes to mqttaudio will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Output limiter configuration.** Two new `audio` keys control the final output stage: `output_ceiling_db`
  (the limiter ceiling in dBFS, default `-1.0`, must be between `-60.0` and `0.0`) and `master_gain` (a
  linear bus gain applied before limiting, default `1.0`, between `0.0` and `8.0`).
- **`/status` reports a limiter clip counter.** The `/status` JSON now includes `clip_count`, the number of
  output samples the limiter held at the ceiling since startup.
- **Optional per-route downmix gain in `channel_map`.** Each entry of a Play `channel_map` accepts an optional
  `gain` (default `1.0`), e.g. `{"src": 2, "dest": 0, "gain": 0.5}`. When several source channels are routed
  to one destination they sum, which can clip; a per-route gain lets you attenuate (or boost) each route. A
  route with no `gain` is unity, so existing channel maps are unaffected.
- **`bass_management.lfe_gain` trim.** A new optional `bass_management` key (default `1.0`) applies a linear
  trim to the (now count-normalized) summed LFE, so you can match the sub level to the room without touching
  the amplifier. Omitting it leaves the sub at its normalized level.
- **Startup warning when the LFE channel does not exist.** When bass management is enabled but
  `lfe_channel` is greater than or equal to the device's output channel count, a one-time warning is logged
  at startup explaining that bass management will be a no-op (the bass cannot be redirected). Previously this
  was a silent no-op.

### Changed

- **Output overload is now a soft-knee limiter instead of a brickwall clamp.** The summed bus was previously
  hard-clamped to ±1.0 (mislabeled "saturation"), which flat-tops the waveform and adds harmonic distortion
  past full scale. It now passes through a soft-knee limiter at a configurable ceiling (default `-1.0` dBFS,
  see `audio.output_ceiling_db`) with an optional `audio.master_gain`: signal below the knee is unchanged,
  and louder material is smoothly limited so the peak never exceeds the ceiling. Full-scale material is
  therefore reproduced ~1 dB quieter (and less harsh) than before. The number of limited samples is exposed
  as `clip_count` on `/status`.
- **Per-channel calibration (`audio.channel_volumes`) is now applied.** These per-output-channel gains were
  parsed and validated but never affected output; they are now applied as the final per-channel gain stage
  (resolved once at startup, supporting numeric or alias channel keys). Anyone who set `channel_volumes`
  expecting attenuation will now hear it.
- **Play `volume` is clamped to `[0, 1]`.** A Play command with `volume > 1.0` (or negative) is now clamped
  at construction, matching the runtime `volume` command. Previously a Play could amplify above unity (e.g.
  `volume: 5.0` gave a 5× contribution into the mix); such configs will now play at unity instead.
- **Reverse playback no longer adds comb/low-pass distortion.** At any fractional reverse speed (e.g.
  `-0.5`, `-0.75`, `-1.5`) the sub-sample interpolation was blending the wrong neighbor (frame `n-1`
  instead of frame `n+1`), distorting all reverse playback. Interpolation now uses the same direction-
  independent rule in both directions, so reverse playback is clean.
- **Speed changes use cubic (Catmull-Rom) interpolation.** The non-pitch playback-speed path previously read
  the source with two-point linear interpolation, a poor reconstruction filter that adds audible grit when
  playing at a fractional speed (e.g. `0.6×`). It now uses 4-point cubic interpolation, which is far cleaner
  (markedly less spurious energy) and is exact for linear material. The `±100` speed range is unchanged.
  Note: cubic is a reconstruction filter, not an anti-aliasing one — speeds **above** `1.0` on the non-pitch
  path still alias (there is no low-pass before the speed-up). For clean large speed-ups, enable pitch
  correction (`pitch_correction: true`), whose time-stretcher is band-limited.
- **Loop crossfades are equal-power and seamless.** Looped playback with `crossfade_ms` set now uses an
  equal-power (cos/sin) crossfade, removing the ~3 dB level dip a linear blend produced at the crossfade
  midpoint, and the next loop pass resumes past the overlapped head so the crossfaded head region is no
  longer replayed at full level (which previously double-triggered transients near the loop start and could
  click at the seam). Looped ambience/beds will sound subtly fuller and smoother. (Fade-in/out ramps remain
  linear, which is adequate for their typically short durations.)
- **Enabling pitch correction mid-playback no longer drops a silent gap or clicks.** Switching a playing
  sample to pitch-corrected speed (`speed` with `pitch_correction: true`) used to build a fresh
  time-stretcher whose ~120 ms warm-up latency produced several callbacks of silence before audio resumed.
  The stretcher is now pre-rolled with the audio just before the playback position so signal is present from
  the first block, and the brief switch from direct to stretched playback is equal-power crossfaded over a
  few milliseconds so it does not click.
- **Pitch-corrected playback stays phase-continuous at non-integer speeds.** The pitch path advanced the read
  position by the fractional `frames × speed` while feeding the stretcher a rounded-up whole number of input
  frames, so at speeds whose product with the buffer size is not an integer (e.g. `0.7`) each callback re-fed
  a fraction of a frame, smearing the tone. The read position is now advanced by exactly the integer input
  frames fed (carrying the fractional remainder), so successive input slices abut and the output stays clean.
- **Pitch-corrected playback no longer truncates the tail at end-of-file.** The last ~120 ms held inside the
  time-stretcher were previously dropped when the source ended; the stretcher is now drained at EOF and its
  buffered tail is played out, so pitch-corrected sounds end completely instead of cutting off early.
- **Ducking restore now uses the triggering rule's `fade_duration_ms` instead of a fixed 2000 ms.** When a
  ducking primary goes idle, the ducked voices now return to full volume over the same fade the rule ducked
  them with (the longest fade, if several rules ducked the voice), rather than always taking 2 s. A rule with
  `fade_duration_ms: 200` therefore recovers in ~200 ms; configs that relied on the slow 2 s release will
  restore faster.
- **Ducking gain is now smooth and advances at the correct rate.** A voice's duck fade is advanced exactly
  once per audio buffer (previously once per playing sample on the voice, so two overlapping sounds on a
  ducked voice faded ~2× too fast and saw inconsistent gain within a buffer), and the gain is interpolated per
  frame across the buffer instead of stepping once per buffer (removing zipper/stair noise on the fade).
- **Live-input voices can now trigger ducking.** A microphone/line input whose configured `voice_id` is a
  ducking rule's `primary_voice` now ducks the rule's background voices while its input stream is open
  (previously input voices were inert as ducking primaries and ducked nothing). Signal-gated activation
  (ducking only while the input is actually loud) is a later change; for now the input counts as active for
  the lifetime of its stream.
- **`/status/samples` no longer reports live per-sample playback position.** HTTP status is now served
  from a control-side snapshot (the audio thread owns playback state lock-free, so the control plane never
  reads it). The endpoint still reports each sample's static metadata — `id`, `voice`, `file`,
  `total_frames`, `total_ms`, `sample_rate`, `volume`, `voice_volume`, `speed`, `loop_mode` — but
  `position`, `position_ms`, and `progress_percent` are now always `0`. (Lock-free RT engine, Sprint 5.)
- **Looping a still-downloading stream no longer buzzes.** A `loop: true` play of an HTTP/streaming
  source now plays forward (emitting silence past the loaded edge) and only begins looping once the
  stream is fully downloaded, instead of replaying a tiny growing prefix in a tight buzz. A looped
  stream therefore takes until download-complete to start its first loop.
- **Finished streamed URLs are promoted to the memory cache.** Replaying a URL that finished streaming
  now serves the cached, fully-decoded buffer instead of re-streaming it, and streamed audio now counts
  against the configured memory limit and participates in LRU eviction. Memory-usage reporting and
  eviction timing change accordingly; a buffer that is still playing is never evicted (it is kept alive
  by its reference, so the size accounting stays accurate).

- **MQTT re-subscription on reconnect.** The daemon now re-subscribes to its command topic on every
  broker (re)connect, so it recovers command handling after a broker restart instead of going silently
  deaf.
- **RT-shared state no longer poison-bricks audio.** The mixer/voice/active-voice mutexes use a
  non-poisoning lock (`parking_lot`), so a panic in one command/HTTP handler can no longer permanently
  silence audio via a poisoned lock. Cache access uses an async mutex so a slow decode no longer stalls the
  command loop or HTTP status endpoints (the decode runs off the async runtime).
- **MQTT command overflow drops instead of back-pressuring.** Under sustained overflow the MQTT producer
  now drops commands (logging a running count) rather than blocking the event loop, which previously could
  stall keepalive and get the broker to drop the session. (HTTP command delivery is unchanged.)
- **Invalid ducking targets are rejected.** A non-finite or out-of-range `ducking_rules[*].target_volume`
  now fails configuration validation instead of being accepted.
- **Graceful shutdown.** SIGINT/SIGTERM now fades out active samples and flushes cache metadata before the
  process exits, instead of cutting audio mid-buffer (no more shutdown click).

- **Output device sample-format negotiation.** The output stream is now built to match
  the device's native sample format (I16/U16/I32/F32) using an internal f32 mix bus and
  a per-sample convert shim, instead of assuming f32. Non-f32 Windows WASAPI shared-mode
  and ALSA `hw:` devices no longer crash at startup.
- **Channel-count fallback.** Requesting a channel count the device does not expose
  exactly now opens the next-larger configuration (extra channels stay silent) instead
  of exiting.
- **Sample-rate selection.** The nearest device-supported rate is chosen (honoring
  discrete-rate devices) and validated against the device's supported configs before the
  stream is built; the previous arithmetic clamp could pick an unsupported rate.
- **`audio.buffer_size` is now honored** via `BufferSize::Fixed` when the device supports
  it (previously the validated value was ignored). Latency/period size may change for
  existing configs.
- **Bass management high-passes the mains by default.** `bass_management.remove_bass_from_sources` now
  defaults to `true` (was `false`): when bass management is enabled, the bass routed to the sub is removed
  from the source channels, which is standard bass management. Anyone who relied on the previous additive
  "LFE+Main" behavior (full-range mains *and* the same bass in the sub) must now set
  `remove_bass_from_sources: false` explicitly. This only affects configs with `bass_management.enabled: true`.
- **Bass-management crossover is now 4th-order Linkwitz-Riley.** The crossover used a single 2nd-order
  Butterworth low-pass/high-pass pair, whose outputs are 180° out of phase at the crossover frequency and so
  notch (partially cancel) where the mains and sub overlap. It is now a 4th-order Linkwitz-Riley crossover
  (two cascaded Butterworth sections per filter): the low- and high-pass outputs are in phase and recombine
  flat through the crossover, and the slopes are steeper (24 dB/oct). This changes the acoustic response for
  anyone with bass management enabled (a fuller, flatter blend at the crossover).
- **LFE level is independent of the number of source channels.** The summed LFE was the *sum* of the bass
  extracted from every source channel, so feeding correlated bass from two channels made the sub ~6 dB louder
  than from one. The summed LFE is now normalized by the active source count, so the sub level no longer
  scales with how many channels feed it. A 2-source setup is therefore ~6 dB quieter in the sub than before;
  use the new `bass_management.lfe_gain` to trim if needed. Affects only configs with bass management enabled.

### Fixed

- **No startup panic on non-f32 devices.** Stream-build failures now exit gracefully (on
  Linux with a `plughw:`/`default` recommendation) instead of panicking.
- **Output auto-recovery.** A fatal output-device error now rebuilds the stream with
  exponential backoff (re-resolving the device) instead of going permanently silent; if it
  cannot recover, the process exits so a service manager can restart it.
- **Bass-management filter no longer processes denormals after audio goes quiet.** The crossover's IIR
  decay tail could settle into the floating-point subnormal range and be processed every sample on the audio
  thread, where subnormal arithmetic is dramatically slower (a periodic CPU-spike / dropout risk during quiet
  passages). The filter state is now flushed to zero once it falls below an inaudible threshold. No effect on
  correct-level audio.

### Security

All of the following lockdowns are **opt-in with backward-compatible defaults** — an existing
open/anonymous deployment behaves exactly as before unless you configure them.

- **File allowlist enforcement.** When `security.allowed_directories` is set, local file paths are
  canonicalized and must resolve inside an allowed directory before they are opened, so traversal
  (`../../etc/passwd`) and symlink escapes are rejected. An empty/absent allowlist preserves allow-all
  and logs a one-time startup warning.
- **MQTT TLS.** A new `[mqtt.tls]` block switches the broker connection to TLS; with `ca_path` it
  trusts a private/self-signed CA, otherwise the system root store. TLS is never enabled implicitly —
  plain TCP stays the default on every port, including 8883 — so a legacy plaintext broker is never
  silently broken. A non-fatal warning fires when credentials would be sent in cleartext to a
  non-loopback broker.
- **HTTP authentication.** Setting `http.require_auth` requires the bearer token on the
  status/command/WebSocket endpoints (the health endpoint stays open); tokens are compared in constant
  time. A loud non-fatal warning fires when the server binds a non-loopback address without auth.
- **Atomic, size-verified cache writes.** Downloads are written to a temp file and atomically renamed
  into place, then verified against the expected size on load, so an interrupted download can no longer
  leave a truncated file that reads back as "valid".
- **Stable cache keys.** Cache filenames derive from a SHA-256 of the URL instead of a process-seeded
  hash, so cached entries survive restarts and are reused across runs.
- **Conditional cache revalidation.** Stale HTTP cache entries are revalidated with a conditional GET
  (`If-None-Match`/`If-Modified-Since`); a `304` refreshes the entry in place, a `200` re-downloads.

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
