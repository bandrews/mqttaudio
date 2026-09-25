# Known Issues

Open problems, limitations and accepted trade-offs, so a later change can pick them up deliberately.
Fixed issues are recorded in the [changelog](../CHANGELOG.md); the design decisions behind the
current behavior are in [docs/sprints/DECISIONS.md](sprints/DECISIONS.md), and the August 2026
quality review with its deferred backlog is in [docs/quality-review-2026-08/](quality-review-2026-08/).

## Open

### Playback and commands

- **Pitch correction never engages on a cold play whose file is not kept in memory.** The stretcher
  needs the complete buffer that the upgrade pass swaps in once the decode is promoted into the memory
  cache. A file larger than the free budget, or one changed or invalidated during its decode, is never
  promoted, so that play continues without pitch correction. A `speed` sent while the decode is still
  running warns that correction is deferred; one sent after promotion failed gets no warning.
- **A play can leave phantom state when the audio command queue is full.** The play paths register
  the voice, its activity count, ducking and the status entry before sending the sound to the audio
  thread, and do not undo them when the send fails (the HTTP caller gets `500`). The phantom stays in
  `/status/samples` and keeps its voice active, holding any ducking, until restart. The 1024-slot
  queue fills only when the audio thread stops draining it, for example while the output device is
  being rebuilt.
- **A cold play can drop a block under lock contention.** A progressive buffer is shared with the
  decoder through a `RwLock`; the audio thread only `try_read`s it, and when the decoder holds the
  write lock (including while its storage grows) that sound's block is silent while its position
  still advances.
- **Routes to a missing output are silent.** A `channel_map` or input route whose `dest` is beyond the
  device's channel count is dropped without a warning.
- **`/status/samples` reports the requested `speed`**, not the clamped value.

### Cache and loading

- **Headroom drops to zero during a load of unknown length.** `memory_headroom` reserves the whole
  free budget for such a load (and for a load whose lock is momentarily write-held), so plays decided
  meanwhile are windowed unless they have no size estimate or are already in memory.
- **`cache_clear` does not cancel loads in progress**, which then fill the cache again, and it
  deletes the temporary files of downloads still running, whose completion then fails with a
  warning. `cache_invalidate` racing a windowed download's completion can likewise leave the old
  entry registered.
- **Promotion copies the decoded buffer.** `cleanup_completed_loads` promotes with `to_vec()`, a
  transient second copy of the whole decode, made before the memory cache decides whether to keep it.
  Full loads are not all bounded by `full_load_max_bytes` (`mode: "full"` and files without a size
  estimate), and the copy runs on the control loop when the upgrade pass triggers it.
- **The disk cache has no size limit or eviction.**

### Live inputs

- **A lost input device is not reopened**, and `/ready` is decided once at startup, so it stays
  `200` after an input dies. Restart after reconnecting a USB microphone. Physical unplug/replug and
  multichannel USB duplex remain field checks for the release candidate.
- **The out-of-range source-route warning is unreachable.** `out_of_range_input_routes`
  (`src/main.rs`) can never fire, because an input whose routes need more channels than the device
  has fails to open first.

### Mixer capacity

- **The output callback's mix bus can allocate.** It starts empty and is resized inside the callback
  (`build_typed_output_stream` in `src/audio/engine.rs`), so the first block allocates, and so does any
  larger block when the device picks its own buffer size. The allocation harness does not cover it.
  Bass management's send is sized for 8192 frames and grows the same way for a longer block.

### HTTP API

- **Without a token, only browsers that mark their requests are kept from other sites.** The
  cross-site check reads `Sec-Fetch-Site`, which current Chrome, Edge, Firefox and Safari (16.4 and
  later) send; an older browser is not checked, and a page that resolves its own domain name to the
  daemon's address (DNS rebinding) counts as the same site. Setting a token closes both.
- **The 30-second command timeout starts after queuing**, so a request can wait longer while the
  command queue is full.

### Web UI

- **Error alerts show the raw response body** (`{"success":false,"error":"..."}`) rather than the
  error text, because the transport puts the body in the error.
- **Faders and sliders keep a refused value.** After the daemon refuses a change, the control stays
  where the operator left it. The speed slider can land on `0`, which the daemon refuses, and the
  volume faders stop at `1` although the daemon accepts up to `4`.
- **A non-numeric `internal_id` is flagged but not blocked** on the sample controls; the daemon's
  `400` is what the operator sees.
- **A missing token goes undetected when WebSockets are off.** The connect screen detects a daemon
  that wants a token through `GET /ws`; with `http.websocket_enabled` off and a token set without
  `require_auth`, it only learns from the first refused command.
- **The inputs list cannot show the level an unmute restores**; its input type lacks
  `unmuted_volume`.
- **The cue launcher's macro help text names "Sprint W8".**
- **The end-to-end tests do not stub `GET /ws`**, so the token probe falls back to `/version`
  there.

### Configuration editor

- **Saving validates the file alone.** A config that relies on `MQTTAUDIO_HTTP_AUTH_TOKEN` to satisfy
  `http.require_auth` cannot be saved.
- **Saving keeps permission bits but not the owner or group.** Editing a service's
  `root:mqttaudio 0640` config with `sudo` leaves it `root:root 0640`, which the service cannot read
  until the group is restored.
- **Formatting is not preserved.** Keys are written in alphabetical order, and `<file>.bak` keeps
  only the previous save.

### Packaging and tooling

- **A unit test failed once under full-suite load** (recorded when the binary still compiled its
  own copy of every module; name not captured) and passed on every rerun. The suites have
  timing-sensitive tests; capture the name if it recurs.

### Code health

- `handle_command` (`src/main.rs`) has empty-selector branches in `seek`, `speed`, `stop` and
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
  its delay (it hands over 1024-frame chunks, 21 ms at 48 kHz) for a buffer that never drifts to a
  click (D33). The steering is
  proportional, so the buffer settles near, not exactly at, its target; the trim ceiling allows for
  the offset.
- **`fadeall` and `stopall` leave live inputs running.** Use `input_mute`.
- **Content routed straight to the LFE channel is not low-passed.** Only the bass the sounds send from
  the source channels passes the crossover (D32, D63).
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
