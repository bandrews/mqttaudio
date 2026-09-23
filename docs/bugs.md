# Known Issues

Open problems, limitations and accepted trade-offs, so a later change can pick them up deliberately.
Fixed issues are recorded in the [changelog](../CHANGELOG.md); the design decisions behind the
current behavior are in [docs/sprints/DECISIONS.md](sprints/DECISIONS.md), and the August 2026
quality review with its deferred backlog is in [docs/quality-review-2026-08/](quality-review-2026-08/).

## Open

### Talkback

- **The talkback microphone is live at startup.** Inputs open unmuted, and nothing mutes the talkback
  source (`GM_MIC`, or `mic`) until the first lease is released, expires or is hard-muted, while
  `/status/talkback` reports `muted` the whole time. Workaround: send `input_mute` for it at startup.
  Muting it at startup would also mute any input left at the default `voice_id` of `mic`, so this
  needs a decision about how talkback inputs are marked.
- **`destination` and `gain` are validated but never applied.** The microphone plays through its
  configured routes at its configured volume. The destination allowlist (`GUEST_ALL`, `ROOM_1`,
  `ROOM_2A`, `ROOM_2B`, `ROOM_3`, `ROOM_4`) is hard-coded in `src/talkback.rs`.
- **Ordinary commands can reopen the microphone.** Without an active lease, `input_mute` unmutes it.
  With a lease, the guard compares the `input` string with the source's voice id, so selecting the
  input by number (`"0"`) bypasses it.
- **A lease can fail open.** If the audio command queue is full when a lease expires, is released
  or is hard-muted, the mute is dropped (logged at `error` on expiry, silently otherwise) and the
  microphone stays live.
- **Rough edges:** the acquire reply does not carry the `lease_id` (read `/status/talkback`), so an
  MQTT-only client cannot release; every refusal, including a malformed `lease_ms`, answers `403`
  rather than `400`; an expired lease keeps refusing other clients for up to one 20 ms tick.

### Playback and commands

- **Ignored commands report success.** `seek` and `speed` on a windowed sound, and a `play` dropped
  because all 256 sounds loop, answer HTTP `200` without doing anything.
- **The seek/speed gate for windowed voices is too broad.** `selector_targets_streamed_voice`
  (`src/main.rs`) skips the whole command when the selector's `voice` has had a windowed sound since
  the voice was last idle, including fully loaded sounds that could seek.
- **Reverse loops with a crossfade may jump at the wrap.** The per-frame mix loop in
  `mix_sample_into_output` wraps a reverse loop to the buffer's end, while the position update
  (`wrap_loop_position`) wraps to just before the crossfaded tail, so the two disagree for one block.
  No test covers reverse looping with `crossfade_ms`.
- **Pitch correction never engages on a cold play whose file is not kept in memory.** The upgrade to
  a complete buffer happens only when the decode is promoted into the memory cache; a file larger
  than the free budget stays on its progressive buffer, and the warning that correction "engages when
  the load completes" is then wrong.
- **Routes to a missing output are silent.** A `channel_map` or input route whose `dest` is beyond the
  device's channel count is dropped without a warning.
- **`/status/samples` reports the requested `speed`**, not the clamped value.

### Cache and loading

- **A URL in the disk cache is always decoded in full.** `loading::prepare` skips the windowing
  decision for cached URLs, so a long remote file that was cached by a windowed play is decoded
  whole on its next play (a two-hour 5.1 file is about 8 GB). The same applies to a local file
  re-decoded after an edit, which bypasses the decision in
  `get_or_load_streaming_with_freshness`.
- **Precache and `cache_reload` ignore the budget and windowing.** They always decode in full, and
  log success when the result is then too big to keep.
- **Freshness does not match decision D46.** D46 says a play never waits on the network, `dev`
  checks on every play, and `pinned` checks nothing. In the code, a play of a URL that is only on
  disk revalidates in the foreground (up to 5 s) in every mode, `dev` only shortens the background
  pass, and the background pass holds the cache lock across its network requests, which stalls
  `/metrics`, `/status`, `/status/cache` and new loads while a server is slow.
- **Headroom drops to zero during a load of unknown length.** `memory_headroom` reserves the whole
  free budget for such a load, so every play decided meanwhile is windowed.
- **`cache_clear` does not cancel loads in progress**, which then fill the cache again, and it
  deletes the temporary files of downloads still running, whose completion then fails with a
  warning. `cache_invalidate` racing a windowed download's completion can likewise leave the old
  entry registered.
- **Promotion copies the decoded buffer.** `cleanup_completed_loads` promotes with `to_vec()`, a
  transient second copy bounded by `full_load_max_bytes`.
- **The disk cache has no size limit or eviction.**

### Live inputs

- **Input mute and volume changes are instant.** `input_mute` and `input_volume` set the level
  without a ramp, which can click (`LiveInput::set_muted` / `set_volume`); `voice_volume` ramps.
- **Activity detection reads the captured level before volume and mute**, so a muted microphone
  still triggers its ducking rules.
- **A lost input device is not reopened**, and `/ready` is decided once at startup, so it stays
  `200` after an input dies. Restart after reconnecting a USB microphone. Physical unplug/replug and
  multichannel USB duplex remain field checks for the release candidate.
- **The out-of-range source-route warning is unreachable.** `out_of_range_input_routes`
  (`src/main.rs`) can never fire, because an input whose routes need more channels than the device
  has fails to open first.

### Mixer capacity

- **Streamed sources and live inputs have no hard cap.** `MixerState` reserves 64 streamed sources
  and 16 live inputs, but only full-load samples are capped (256, `MAX_VOICES`). Past the reserve the
  audio thread grows the vector, allocating in the callback.

### HTTP API

- **CORS does not cover the `Authorization` header.** `cors_permissive` sends
  `Access-Control-Allow-Headers: *`, which browsers do not apply to `Authorization`, so cross-origin
  pages must use `?token=`. Mirroring the requested headers would fix it.
- **Cross-site requests in open mode.** With no `auth_token`, any web page the operator opens can
  send the body-less `POST /stopall`, `/cache/clear` and `/talkback/hard-mute` to a daemon on
  `localhost`. Setting a token closes this.
- **The 30-second command timeout starts after queuing**, so a request can wait longer while the
  command queue is full.

### Bass management

- **The LFE sum is divided by the configured source count**, not by how many sources carry signal:
  bass on one of five source channels reaches the subwoofer 14 dB down, and with
  `remove_bass_from_sources` on it is also removed from that channel.

### Configuration editor

- **Saving validates the file alone.** A config that relies on `MQTTAUDIO_HTTP_AUTH_TOKEN` to satisfy
  `http.require_auth` cannot be saved.
- **Formatting is not preserved.** Keys are written in alphabetical order, and `<file>.bak` keeps
  only the previous save.

### Packaging and tooling

- **The container health check needs the token in the environment.** With
  `MQTTAUDIO_HTTP_REQUIRE_AUTH=true` it requires `MQTTAUDIO_HTTP_AUTH_TOKEN`, although `/ready`
  never needs a token, and it assumes port 8080 while `http.port` defaults to `0`.
- **No declared minimum Rust version.** Dependencies need Rust 1.88 (ratatui 0.30, time 0.3.47), but
  `Cargo.toml` has no `rust-version` and no build checks it.
- **A binary unit test failed once under full-suite load** (`605 passed; 1 failed`, name not
  captured) and passed on every rerun. The suite has timing-sensitive tests; capture the name if it
  recurs.

### Code health

- `handle_command` (`src/main.rs`) still has arms for `precache` and the cache commands, which
  `loading::prepare` completes first, and empty-selector branches in `seek`, `speed`, `stop` and
  `volume` that the selector pre-check makes unreachable.
- `CacheError`'s variant names carry a targeted `#[allow(clippy::enum_variant_names)]`
  (`src/cache/disk.rs`).
- The ALSA name matcher (`try_match_alsa_device` in `src/audio/device.rs`) compares only the card, not
  the device or subdevice, and misses the `hdmi:` and `iec958:` prefixes. It is a fallback after the
  exact-name match, so the worst case is the right card with the wrong subdevice.

## By design

These are deliberate trade-offs, recorded so they are not mistaken for bugs.

- **Speeds above 1.0 without pitch correction alias.** The fast speed path interpolates (Catmull-Rom)
  but does not low-pass before skipping samples (D27). Pitch correction is band-limited.
- **Every live input goes through the drift-controlling resampler**, even at matching rates, adding
  its chunking delay (about 11 ms) for a buffer that never drifts to a click (D33). The steering is
  proportional, so the buffer settles near, not exactly at, its target; the trim ceiling allows for
  the offset.
- **`fadeall` and `stopall` leave live inputs running.** Use `input_mute`.
- **No low-pass on the summed LFE bus.** Each contribution is already low-passed; only content routed
  straight to the LFE channel reaches it unfiltered (D32).
- **Per-route gain exists for play `channel_map`s only**, not for input routes (D29).
- **A cold play of a rate-converted file uses the chunked resampler**, whose output differs very
  slightly from the one-shot decoder's (tolerance-tested, inaudible).
- **Pitch correction enabled at a sound's first frame** has the stretcher's short warm-up; enabling it
  mid-sound pre-rolls to avoid a gap.
- **The allocation harness cannot see C++ allocations** in the time-stretcher. The stretcher is built
  on the control thread and designed not to allocate while processing, but that is not proven by
  `tests/alloc_harness.rs`.
- **Not surfaced yet:** per-windowed-sound buffer fill and underrun counts, per-input level meters,
  RMS meters, and discrete state-change events on `/ws/state`.
