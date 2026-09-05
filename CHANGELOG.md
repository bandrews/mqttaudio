# Changelog

All notable changes to mqttaudio will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.0-rc.1] - 2026-09-04

This release candidate consolidates the v2.1 development line, the August main
fixes, the CPAL 0.18.2 recovery update, and the local memoria-next microphone,
talkback, and HTTP integration work. See [release validation](docs/releases/v2.1-rc1.md).

- Load preparation runs outside command dispatch; stop/fade-all cancels pending
  playback and microphone controls and lease expiry remain responsive during I/O.
- Native-format capture, shared capture fanout, activity-triggered ducking, applied
  state, and input health reporting operate together with the real-time engine.
- ALSA identifiers round-trip through discovery and selection. Backend-recovered
  xruns keep their stream; permanent output errors use supervised rebuilds.
- Cache enablement, bounded memory/windowed streaming, progressive disk reads,
  cancellation, revalidation, corruption recovery, gain ramps, pitch loops, HTTP
  outcomes, authentication, and environment configuration are reconciled.


### Added

- **Config editor guidance and pickers.** The interactive editor now leads with a framed help pane at the
  top of the screen explaining the selected section or setting; every section carries an introduction and
  collections explain their feature in place (including what to do when they are empty). Settings that
  reference other parts of the config are chosen from pickers instead of typed: channel fields offer the
  defined `channel_aliases` plus plain channel numbers, voice fields offer the voice ids named elsewhere in
  the config (free text still allowed), enums list their options, and `channel_volumes` keys come from the
  channel picker. Ducking rules, inputs, and routes show readable per-item summaries
  (`"narration" ducks music → 20% over 500ms`) instead of bare indices, and macros are edited as guided
  parameter forms — pick from the Play command's parameters with typed validation, with raw JSON still
  available for custom keys.
- **Interactive config editor: `mqttaudio --configure`.** A full-screen terminal UI (ratatui) that creates
  or edits the JSON config file with per-field help, the daemon's own validation before saving, and live
  device testing: the output device picker plays per-channel test tones through the real playback pathway
  (channel volumes, master gain, limiter, and bass management all apply, with live peak meters and a
  channel sweep for speaker identification), and input devices get a live capture level meter. Every config
  field is editable — a coverage test fails the build if a future config field lacks an editor binding.
  Saves are sparse (only explicitly-set keys are written, so built-in defaults can evolve), atomic, preceded
  by a `.bak` backup, and preserve unknown keys/comments in existing files.

### Changed

- **`/ws` now actually streams the daemon's log lines (Sprint 14, D62).** The WebSocket log layer existed
  but was never installed in the tracing subscriber, so `/ws` clients got the welcome frame and then silence.
  It is now wired in `main` alongside the console/MQTT layers; every log line arrives as a
  `{"type":"log","message":…}` frame, as `docs/http-api.md` always claimed.
- **`/command` rejects non-JSON bodies with `CommandResponse` JSON (Sprint 14, D61).** A body the JSON
  extractor cannot parse now returns `400 {"success":false,"error":"Invalid JSON: …"}` instead of axum's
  plaintext rejection, so every `/command` error parses the same way. Clients that special-cased the
  plaintext body must read the JSON shape.
- **Removed the dead `audio.channel_names` config field (Sprint 14, D60).** It was deserialized and never
  read (channel routing uses `channel_aliases`). Old configs carrying the key still parse — unknown keys are
  tolerated.
- **Resampler interpolation stays `Linear` — R1 closed by measurement (Sprint 14, D59 overridden on
  evidence).** At the daemon's quality presets, Linear and Cubic sinc-table interpolation measure identical
  to ~0.015% of an already ≈-60 dB residual, so the planned switch was dropped rather than changing every
  rate-converted file's PCM for no measurable benefit. The measured quality floor is now pinned by a test.

- **The audio and capture threads no longer log or allocate on any steady-state path (Sprint 13).**
  Remaining real-time residuals are closed: ducking states are pre-populated at startup so the FIRST duck of
  a voice is allocation-free; pitch correction is built control-side and shipped to the audio thread inside
  the Speed command (`SetSpeedWithCorrector` + `PitchBundle`), with the displaced corrector dropped off the
  audio thread — toggling pitch mid-play is now Rust-side alloc/free-free; the capture path's
  resample-error/overflow/ratio-reject logging became relaxed atomic counters surfaced on `/metrics`
  (`input_capture.*`) and drained into off-thread log lines; conversion/pitch scratch buffers are pre-sized
  to the stream's maximum block with counted (`scratch_regrows`, `pitch_scratch_regrows`) fallbacks.
- **Hard voice cap (Sprint 13, D18, owner-approved).** Beyond 256 simultaneous voices a new play now steals
  the oldest non-looping voice (or is rejected if every voice loops) instead of growing the pool on the audio
  thread. Unreachable in normal use; changelogged because >256-voice behavior changes.
- **Speed-command validation moved to dispatch (Sprint 13, D57).** A negative speed with pitch correction now
  warns at the control plane and is not sent (it was previously ignored per-voice on the audio thread); pitch
  correction targeting a still-loading cold play warns that it engages when the load completes.

- **Cold plays start instantly (Sprint 12, D51).** Local files and disk-cached HTTP downloads in
  full-load mode now decode progressively, exactly as uncached HTTP always did: the play returns a
  playable buffer immediately (measured ~0.2 ms for a 5-minute WAV that previously waited ~210 ms —
  and on Pi-class hardware, seconds — for the whole decode) and audio begins with the first decoded
  chunk. Once the decode finishes, the playing voice is upgraded in place to the fully-loaded buffer,
  so seek/loop-crossfade/pitch behave exactly as before from that point on. Observable differences:
  pitch correction enabled in the first moments of a cold play (before its decode finishes) is
  deferred until the upgrade (it was previously available immediately, after the long blocking wait);
  a `start_position` deep into a cold file waits (bounded by `stream_prebuffer_deadline_ms`) for the
  decode to reach it; `/status/samples` reports the header's total-frames estimate for a still-loading
  play. The cold play's cached PCM comes from the chunked resampling path (an inaudible,
  tolerance-tested divergence from the one-shot path, already shipping for uncached HTTP).
- **One request per uncached HTTP play (Sprint 12, D55).** When the windowing probe decides a small
  uncached HTTP asset should full-load, the already-open response is decoded directly instead of being
  dropped and re-fetched — saving a full round-trip — and a cacheable download is teed to the disk
  cache during playback (previously the HTTP full-load path never persisted, so a restart re-downloaded).
- **The windowed prebuffer gate is event-driven (Sprint 12, D53).** The producer wakes the gate the
  moment the threshold is crossed; the ~5 ms polling quantum is gone (measured prebuffer-ready time
  fell from ~5.2 ms to ~0.2 ms plus actual fill). Deadline semantics are unchanged.
- **`cache_reload`/invalidation now abandons in-flight loads (Sprint 12, D52).** Previously a
  streaming load racing an invalidation could re-promote stale content into the cache and new plays
  could join the stale stream. Errored streaming loads are likewise dropped instead of lingering
  (replays of a failed URL retry instead of silently joining a dead buffer).

### Added

- **First-start play-latency telemetry (Sprint 11, D50).** Every play now measures the time from its command
  reaching the audio ring to the first block in which it mixes loaded audio, published from the audio thread
  via a pre-allocated atomic (one relaxed store on the publishing block — no allocation, no lock, proven by
  the allocation harness). `GET /metrics` gains `latency.play_to_first_mix_ns{last,max}` and
  `latency.plays_measured`, and each play emits one `latency`-target log event with its control-side stage
  durations (dispatch → decision → ready → enqueue). Purely additive — no play-path behavior changes.

- **Opt-in live-position telemetry (`GET`/`POST /telemetry`).** Off by default. When enabled, the audio thread
  publishes each playing sample's live frame position into a pre-allocated atomic once per block (a single
  relaxed store — no allocation, no lock, RT-safe), and `GET /status/samples` then reports real `position`,
  `position_ms`, and `progress_percent` instead of `0`. With telemetry off the callback does no new work and
  those fields stay `0` exactly as before, so nothing changes for existing clients. `POST /telemetry`
  (`{"enabled": true|false}`) flips it; `GET /telemetry` reads it. Added for the web control app's live
  progress bars; it is opt-in because it adds a little real-time work. `/status/samples` also gains a
  `windowed` boolean (a streamed/forward-only sample) so a UI can gate seek/speed/reverse reliably.
- **Live output meters + state-event channel (`/ws/state`, `GET /status/meters`).** Also opt-in (the same
  `/telemetry` gate). When enabled, the audio thread publishes per-output-channel peak levels into atomics (a
  relaxed store per block in the limiter pass — no allocation, no lock), and a ~15 Hz control-side timer
  broadcasts a compact tick frame (`{type:"tick", samples:[{internal_id,position_ms,progress_percent}],
  meters:{output:[…]}}`) over a new `/ws/state` WebSocket — **only while telemetry is on and at least one client
  is connected**, so it costs nothing otherwise. `GET /status/meters` is a poll fallback. Added for the web
  app's live meters and smoother position updates.
- **Read-only `GET /config`.** Returns the running configuration as JSON with secrets redacted
  (`http.auth_token` and `mqtt.password` are nulled out). Config is read once at startup, so this is a startup
  snapshot. Added so the web app can show current values and generate restart-required config snippets for
  tuning (the daemon has no hot-reload).
- **Windowed streaming for big files (`mode=stream`, and via the default `mode=auto`).** A `play` of a large
  or long file — local **or** `http(s)://` — is now played through a bounded ring (a fixed window, default
  1.5 s) fed by a background decoder, instead of being fully decoded into memory, so a multi-hour cue costs
  `O(window)` memory with a low time-to-first-sample. The Play command accepts an optional `mode`
  (`auto`|`full`|`stream`, default `auto`) plus `window_ms`/`prebuffer_ms` overrides;
  `cache.stream_window_ms` / `stream_prebuffer_ms` / `stream_prebuffer_deadline_ms` set the defaults.
  Windowed (streamed) voices play forward only — seek, loop-crossfade, reverse, variable speed, and pitch
  correction do not apply to them. An uncached HTTP URL is routed by the same size/budget decision (probing
  `Content-Length`); a live stream with no `Content-Length` always windows. A windowed HTTP play streams
  through a bounded, back-pressured reader, so even a multi-hour remote WAV cannot OOM the daemon.
- **Cacheable HTTP windowed plays persist to disk.** A windowed play of a cacheable HTTP URL (one with a
  `Content-Length`) tees its download to the disk cache as it plays, so the next play of that URL hits disk
  with no extra request. A live source (no `Content-Length`) or an explicit per-play `"cacheable": false`
  windows without persisting. This is incremental (the bytes are written as they stream, not after a full
  download), so it does not delay time-to-first-sample.
- **Auto memory budget + hard cache cap (never OOM).** The decoded-audio cache now has a hard cap. By default
  it auto-detects a bounded size from available system memory (≈40 %, clamped to [128 MiB, 1 GiB]) so the
  daemon never camps all RAM, and `mode=auto` automatically windows any local asset whose estimated decoded
  size would not fit the budget — so a 2-hour 5.1 cue can never OOM the box under the default mode. New
  `cache.memory_budget` (`{"mode":"auto"|"explicit"|"unlimited", ...}`), `cache.load_mode`,
  `cache.full_load_max_bytes`, and `cache.full_load_max_seconds` knobs tune it. `/metrics` now reports the
  cache's resident bytes, entries, headroom, and on-disk bytes.
- **Cache freshness: local edits picked up, remote entries refreshed in the background.** Editing a local
  asset between plays now takes effect — on a warm cache hit the file's mtime+size are checked and it is
  re-decoded if it changed. Remote (HTTP) entries are refreshed out-of-band by a periodic freshness tick
  (never blocking a play on the network). `cache.freshness` (`trusting`|`dev`|`pinned`, default `trusting`)
  and a per-play `freshness` override control it. A new `cache_reload` command (MQTT, and `POST /cache/reload`)
  invalidates an entry and re-precaches it, so a content pipeline can force fresh+instant on republish.
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
- **Optional `schema_version` config field.** The config may now carry a top-level `schema_version` integer
  (absent means "current"). If it is *newer* than the running build understands, a one-time startup warning is
  logged that newer fields may be ignored; the config still loads. This lets an operator running an old binary
  against a newer config get a heads-up instead of silent surprises.
- **Bass source-channel sanity checks.** Config validation now rejects a `bass_management.source_channels`
  list that contains a duplicate channel, or a source channel equal to the resolved `lfe_channel`, with an
  error message naming the offending channel (previously both were silently accepted).
- **`/version` and `/metrics` HTTP endpoints.** `GET /version` returns the build identity
  (`name`, `version`, and `git_sha` when the build injected `MQTTAUDIO_GIT_SHA`). `GET /metrics` returns real
  operational telemetry — `uptime_seconds`, `clips` (limiter holds), `xruns` (audio stream-error/dropout
  count), the active voice/sample/input counts, `output_channels`, and a per-voice `ducking` map of resolved
  multipliers. Both are open in open mode and gated alongside the status routes under `http.require_auth`.
- **`/status` and `/status/voices` enriched.** `/status` now also reports `xruns` (the audio stream-error
  count, alongside the existing `clip_count`), and each entry of `/status/voices` carries its current
  `ducking_multiplier` (`1.0` when the voice is not ducked).
- **Structured (JSON) logging.** A new `logging.format` option (`"text"` default, or `"json"`) emits each log
  record as one JSON object per line for log aggregation (journalctl/Loki/ELK). The MQTT log topic, when
  configured, publishes alongside whichever console format is selected.
- **systemd service unit.** `packaging/mqttaudio.service` ships a hardened `Type=simple` unit
  (dedicated unprivileged user, `Restart=on-failure`, read-only filesystem with a writable cache state
  directory, ALSA-only device access) plus install/enable instructions in the README.

### Changed

- **`cpal` upgraded from `0.15` to `0.17`.** Brings the current cross-platform audio I/O layer (including the
  CoreAudio device backend) and adopts cpal's reworked device-identity API: device names are now read through
  `Device::description()` (the `Device::name()` call used through `0.15` is deprecated), and `SupportedStreamConfig`
  exposes the sample rate as a plain `u32`. No change to the device-selection policy, the lock-free RT callback,
  or the audio output: the bump is API adaptation plus the newer backend. Device enumeration and name-based
  selection (`audio.device`) are covered by real-device round-trip tests (`tests/device_smoke_test.rs`,
  `[RB]`-gated), and the RT callback remains 0-allocation / 0-free (alloc harness green).
- **`cache.max_memory_mb: 0` now means auto-detect a bounded cap, not unlimited.** Previously `0` (and the
  former `512` default) meant an unlimited cache, which could OOM the box on a big file. `0` is the new
  default and resolves to the auto memory budget. A positive `max_memory_mb` is still an explicit hard cap
  and always wins; set `cache.memory_budget: {"mode":"unlimited"}` to opt back into no cap.
- **Large local assets played with the default `mode=auto` now stream (windowed) instead of fully loading.**
  This trades seek/loop/pitch on those large assets for bounded memory and a low time-to-first-sample; set
  `mode=full` per play (or raise `cache.full_load_max_bytes` / `cache.full_load_max_seconds`) to keep
  full-loading them.
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
- **`input_mute` now restores the prior volume on unmute instead of forcing `1.0`.** Unmuting an input
  returns it to the level it had when muted (e.g. a calibrated `0.7`), rather than jumping to full scale.
  Anyone relying on unmute bumping a calibrated input up to unity will see a difference. (Setting an explicit
  `input_volume` clears the muted state, so a later unmute does not revert that change.)
- **`voice_volume` now affects input-only voices.** A `voice_volume` command targeting a voice that has only
  a live input (no sample playing on that voice) now ramps the input's level. Previously it was a silent
  no-op unless a sample happened to share the voice id; scripts that sent `voice_volume` to a mic voice
  expecting nothing will now change the input level.
- **Live inputs fade to silence on underrun instead of cutting hard.** When an input's ring buffer briefly
  starves (e.g. clock drift between two devices), the mixer now holds the last frame and fades it to silence
  over a few samples rather than dropping straight to zero, removing the click an abrupt cut produced. The
  input's `voice_volume` ramp also keeps advancing through the starved frames, so it stays time-accurate.
- **Live inputs always run through async sample-rate conversion (drift control), even at equal nominal rates.**
  A capture device and the output device are clocked by independent oscillators, so even a "48000 → 48000"
  input slowly drifts against the output and would eventually overflow or starve its ring buffer (a periodic
  click or dropout on a long-running daemon). Every input now feeds an async resampler whose ratio is gently
  steered from the measured ring-buffer fill toward half-full, so the ring stays bounded indefinitely. The
  equal-rate raw passthrough path is gone. This is inaudible in steady state (the steering authority is a
  fraction of a percent); if any pitch wobble is ever observed on an input, the steering gain is too high.
- **Out-of-range input routes now log a warning (previously silent).** Once an input device opens and its
  channel count is known, any route whose source channel is `>=` that count is logged as a warning (the mixer
  silently drops such routes). No audio change — only a new diagnostic so the misconfiguration is visible.
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
- **Auto-generated voice ids are now unique.** A Play with no explicit `voice` is assigned an auto id of the
  form `_auto_<millis>_<n>` (a monotonic counter is appended), where it was previously just `_auto_<millis>`.
  Two Plays without a voice in the same millisecond used to receive the *same* id and so merged into one voice
  for `voice_stop`/`voice_fade_out`/`voice_volume`/ducking purposes; they now get distinct voices. Anyone
  relying on the exact `_auto_<millis>` string, or on same-millisecond no-voice Plays sharing a voice, is
  affected.
- **`seek` on a still-loading stream now lands at the requested time.** A forward `seek` into a region of a
  streaming buffer that has not yet downloaded previously snapped back to the last loaded frame; it now clamps
  against the total (or the streaming estimate) — the same rule `start_position_ms` already used — so the seek
  lands at the requested frame and plays silence until that region loads. Seeks within a fully loaded or
  complete buffer are unchanged. (Affects streaming/HTTP sources only.)
- **`crossfade_ms` without `loop: true` now logs a warning.** The loop crossfade only runs at a loop boundary
  of a buffer long enough to hold the crossfade at both ends. A Play that sets `crossfade_ms` without
  `loop: true`, or whose crossfade is longer than half the clip, silently did nothing; dispatch now logs a
  warning so the ignored crossfade is visible. No change to the blend math.
- **Routing input content to the bass-management LFE channel now logs a warning.** When bass management is
  enabled and a configured input route's destination is the LFE channel, that content reaches the sub
  full-range (the crossover is bypassed) and the extracted bass is summed on top. A one-time startup warning
  now flags this so it is not a silent surprise; routing to the LFE deliberately is still allowed.

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
  device. An input `device` may also be the index number printed by
  `--list-inputs`.
- **The daemon blocked its own microphone input**: resolving the output
  device opens handles for both directions, and the daemon kept them for the
  life of the process - so its own idle capture handle made the output card's
  input side busy, invisible to input enumeration, and unusable by any other
  capture application. Device handles are now released before inputs open,
  and again as soon as the output stream is built. Full-duplex on a single
  interface (play out of and capture into the same card) works now.
- **Audio streams died permanently on the first unrecovered xrun**: the
  audio backend's ALSA loop only recovered underruns detected at one call
  site; an xrun or error state surfacing anywhere else (observed in the
  field as endless `snd_pcm_poll_descriptors ... (-32)` and `POLLERR`
  spam with no audio) spun forever without recovery. The backend is
  upgraded (cpal 0.15 to 0.18) to one that recovers xruns and suspend
  events at every call site and exits the stream worker cleanly when a
  device disconnects. The ALSA `null` device, which the upgraded backend
  enumerates, stays excluded from device listing and selection.
- **Capture ignored the preferred sample rate on `plughw:` devices**: ALSA
  plug devices report one continuous rate range with an implausible maximum,
  and the capability sanity filter discarded the whole configuration for it,
  falling back to the device default (often 44100 Hz stereo) regardless of
  the output rate or an explicit `sample_rate`. That forced the capture
  resampler into the path and, on shared-clock interfaces, made the output
  stream impossible to open at its own configured rate. The implausible
  ceiling is now clamped instead of disqualifying the configuration, so
  capture follows the output rate whenever the hardware allows it.
- **Full-duplex on shared-clock USB interfaces**: with capture already
  running, opening the output stream on the same card could fail with
  `Invalid argument` and crash the daemon, because the two directions ran
  with different buffer parameters. Capture streams now open with the same
  `audio.buffer_size` as the output (falling back to the device default if
  rejected), and an output stream that still cannot open reports the
  requested parameters and the running capture configuration instead of
  panicking.
- **Input device resolution hid sibling aliases of a card**: resolving an
  input collected every enumerated device at once, and since enumeration
  opens each device for capture, the first alias of a card (`hw:`) claimed
  its only capture substream and made every other alias of the same card
  (`plughw:`, `dsnoop:`) unenumerable - so the recommended `plughw:` name
  could never resolve. Devices are now enumerated one at a time, which also
  keeps device numbering identical to `--list-inputs` for selection by
  index.
- **"Input device not found" is now diagnosable**: the error lists which
  devices could be opened for capture at that moment, and on Linux probes the
  requested name directly through ALSA to say *why* it is unavailable - held
  by another process (a sound server such as PipeWire, or a second daemon),
  permission denied (service user not in the `audio` group), or nonexistent.
  Capture devices are only enumerable while they can actually be opened, so a
  device shown by an interactive `--list-inputs` could previously vanish into
  an unexplained "not found" when the daemon ran as a service.
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
