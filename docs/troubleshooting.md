# Troubleshooting

Start with the log: most failures name their cause there.

- The log goes to **standard output**. Under systemd, read it with `journalctl -u mqttaudio`.
- Configuration errors are printed to **standard error** before the daemon exits.
- `--verbose` (or `"logging": {"level": "debug"}`) adds detail; `RUST_LOG=mqttaudio::cache=debug`
  narrows it to one area.
- Over HTTP, a failed command answers with the reason; MQTT commands only log it.

## The daemon will not start

| Message | Cause and fix |
|---------|---------------|
| `Failed to load configuration: Parse error: ...` | The config file is not valid JSON. The message gives the line and column |
| `Configuration validation failed:` followed by a list | Each line names a setting and its allowed values. Fix them all; see [Configuration](configuration.md) |
| `Failed to find output device: Output device not found: ...` | `audio.device` or `--device` does not match any device. Copy the **Device ID** from `--list-devices`; on Linux that is the ALSA ID (`plughw:CARD=...,DEV=0`), not the description. A `--device` option overrides the config |
| `Failed to configure output device '...'` | The device exists but none of its configurations fit. Remove `audio.channels` and `audio.sample_rate` to let the daemon choose, or pick values the device lists |
| `Audio output unavailable: ...` | The stream could not be opened. On Linux, a `hw:` device may need a format the daemon does not produce: use the `plughw:` ID for the same card |
| `Failed to initialize cache: ...` | `cache.directory` cannot be created or written. Point it at a writable directory, or set `cache.enabled` to `false` |
| `No command interface is available ...` | Neither MQTT nor the HTTP server is running. Check `mqtt.topic`, and whether the HTTP port is already in use |
| `Failed to connect to MQTT broker: MQTT TLS configuration error: ...` | The CA file in `mqtt.tls.ca_path` cannot be read. With the HTTP server on, the daemon continues without MQTT (`Continuing with HTTP-only mode`). A file that can be read but is not a valid certificate shows up only when connecting, as repeated `MQTT error: ...` lines |

An unreachable broker does not stop startup: the daemon logs `MQTT error: ...` and retries every
`mqtt.reconnect_delay_seconds`.

## Commands have no effect

1. **Is the daemon receiving them?** At `debug` level each MQTT message is logged as
   `Received MQTT message on topic ...`. If nothing appears, check that the daemon's topic
   (`Ready to receive MQTT commands on topic: ...` at startup) matches the one you publish to, that
   it logged `MQTT connected`, and that both use the same broker.
2. **Did the command parse?** `Macro expansion error: JSON parse error: ...` means the MQTT payload
   is not valid JSON (HTTP `/command` answers `400 Invalid JSON`). `Failed to parse command: ...`
   means the command name is unknown (names are case-sensitive), a required parameter is missing, or
   a parameter has the wrong type. `internal_id` and `input` must be JSON strings, such as `"3"`.
3. **Did it find its target?** `No sample matches the selector ...` means nothing playing matched.
   A `stop` sent while its sound is still loading arrives first and finds nothing; `stopall` and
   `fadeall` do cancel pending loads.
4. **Was part of it ignored?** Parameters with misspelled names are ignored without a message.
   A windowed (streamed) sound ignores some `play` options, with a warning, and `seek` and `speed`,
   with a warning only when the command selects by `voice`; see
   [Commands: Full and windowed plays](commands.md#full-and-windowed-plays).
5. **Is the daemon overloaded?** `Command queue full; dropped MQTT command` means commands arrive
   faster than they are processed. `Too many concurrent loads; command rejected` means 32 plays
   or cache commands were already loading.

## A sound does not play

Look for `Load failed: ...` or `Failed to load ...` in the log:

| Message contains | Cause |
|------------------|-------|
| `is outside security.allowed_directories` | The file is not under a directory in `security.allowed_directories` |
| `Cannot resolve path` | The file does not exist (with an allowlist configured) |
| `I/O error` | The file cannot be read: missing, or no permission for the daemon's user |
| `unsupported feature`, `Unsupported audio format`, `Unsupported format`, `No default audio track found`, `No audio track found` | The file is not in a format mqttaudio decodes: WAV, AIFF, CAF, FLAC, MP3, MP1/MP2, Ogg Vorbis, AAC and ALAC (in MP4/M4A), or Matroska/WebM with one of those codecs. Opus is not supported |
| Any other `Decode error` | The file is damaged or unusual. `ffprobe <file>` shows what it contains |
| `Audio decoding failed` | The decode failed before the play could start. An earlier `Failed to create decoder for ...` or `Decode error for ...` line gives the reason |
| `HTTP 404 Not Found from ...` (or another status), `HTTP request error` | The URL failed. Try it with `curl -I <url>` from the same machine |
| `Download stalled: no data for 60 seconds` | The server stopped sending partway through |
| `No audio decoded before the prebuffer deadline` | The first audio took longer than `cache.stream_prebuffer_deadline_ms` to arrive |

If the log says the sound is playing but you hear nothing:

- **Volume.** The sound's `volume`, its voice's `voice_volume`, and any active ducking rule
  multiply. `GET /status/samples` and `/status/voices` show all three.
- **Channels.** A mono file plays on output 0 only unless it has a `channel_map`. Routes to an output
  the device does not have are dropped without a message. `GET /status` shows the output channel
  count.
- **The system mixer.** Check that the device is not muted or turned down outside mqttaudio
  (`alsamixer` on Linux).

## Clicks, dropouts and stutter

- **Check `xruns`** on `GET /status`, and `Audio stream error: ...` lines in the log. They mean the
  output device ran out of audio. Raise `audio.buffer_size` (try `1024`), close other programs using
  the device, and make sure you run a release build (`cargo build --release`); debug builds are much
  slower.
- **Pitch correction** costs noticeably more CPU than plain speed changes. Many simultaneous
  pitch-corrected sounds can overload a small machine.
- **Sounds that stutter at the start** of a windowed play point to slow storage or network: raise
  `cache.stream_prebuffer_ms` (and the deadline with it), or `precache` the file.
- **The device disappearing** (a USB interface unplugged) logs
  `Audio stream error after a stable run; rebuilding output...`. The daemon retries with backoff
  and, if the device does not come back, exits with
  `Output device could not be (re)built after several attempts` so a service manager can restart it.

## Distortion

`clip_count` on `GET /status` counts samples that reached the output limiter's ceiling. If it keeps
rising, the mix is too hot: lower sound or voice volumes, `audio.master_gain`, or boosts in
`audio.channel_volumes`. Several sources routed to one output add together; lower their route
`gain`s.

## A changed file still plays the old version

- **Local files** already in memory are re-read when their size or modification time changes,
  unless `cache.freshness` (or the play's `freshness`) is `pinned`.
- **Downloaded files** are re-checked with the server once they are older than
  `cache.revalidate_after_seconds` (300 by default), so a change can take a few minutes to show.
- To pick up a change at once, send `cache_reload` with the file or URL.

See [Caching](features/caching.md#freshness) for the details. `cache_clear` empties both caches; the
disk cache lives in `cache.directory` and can also be deleted by hand while the daemon is stopped.

## Live inputs

**`Failed to open input device '...': Input device not found: ...`**: the message lists the devices
that could be opened for capture at that moment and, on Linux, what ALSA reported for the name you
gave:

- **Device or resource busy.** Another program holds the capture side: PipeWire or PulseAudio,
  another recorder, or a second copy of the daemon. `fuser -v /dev/snd/*` shows who.
- **Permission denied.** The daemon's user cannot open the device. For a systemd service, add the
  service user to the `audio` group (`sudo usermod -aG audio mqttaudio`) and restart it.
- **No such device.** The name does not exist; compare it with `arecord -L`.

A device listed by an interactive `--list-inputs` can still be missing at service start, because
only devices that can be opened at that moment are listed. Run the listing as the service user
(`sudo -u mqttaudio mqttaudio --list-inputs`) to see what the service sees.

The daemon keeps running without an input that failed; `GET /ready` answers `503` and names it. An
input that stops after startup, such as an unplugged USB microphone, logs `Input stream error: ...`
and is not reopened, and `/ready` still answers `200`. Restart the daemon after reconnecting it.

**`Input device '...' sample rate (44100 Hz) differs from output (48000 Hz) - resampling will add latency`**:
capture opened at a different rate from the output, because the device does not offer the output's
rate or is held at another one. It works, with a little more delay. On an interface that uses one
clock for both directions, capture can only run at the rate the card is already running at; if
something else (a `dmix` or `dsnoop` device, a sound server, another program) holds the card at
another rate, free it and capture follows the output rate.

**`Input device '...' latency_ms 5 is below the 11 ms its ring needs to hold one resampler burst; using 11 ms`**:
the configured latency was too small to work and was raised. Set `latency_ms` to the value shown to
silence the warning.

**Choppy microphone audio.** Watch `/status/inputs`: rising `dropped_frames` means capture fills the
buffer faster than the output drains it, rising `underrun_frames` means the output finds it empty,
and rising `trimmed_frames` means latency is being cut back. Both dropped and underrun frames rising
together, alongside ALSA `underrun occurred` messages, usually means the output side is stalling
(often through a `dmix` chain) rather than the microphone. A log line
`Input '...' capture counters moved: ...` reports errors in the capture path itself.

## Ducking does nothing

- Voice names are case-sensitive: `"narration"` and `"Narration"` are different voices.
- A rule applies while its `primary_voice` has a sound playing, or is a live input that is active.
  Check `GET /status/voices`: a primary with sounds must be listed there, and ducked voices show a
  `ducking_multiplier` below `1.0`. A voice used only by a live input is not listed there;
  `GET /metrics` (`ducking`) lists every ducked voice, inputs included.
- A live input as `primary_voice` needs an `activity_threshold` to duck only while someone speaks.
  Without one it counts as active whenever it is open, so the ducked voices stay down.

See [Ducking](features/ducking.md).

## Reporting a problem

Include:

1. `mqttaudio --version` and the operating system
2. The output of `--list-devices` (and `--list-inputs` for input problems)
3. The config file, with passwords and tokens removed
4. The log with `--verbose`, from startup until the problem
5. The commands you sent, and what you expected to happen

Report issues at <https://github.com/bandrews/mqttaudio/issues>.
