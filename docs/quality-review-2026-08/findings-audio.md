# Quality Review Findings: Audio Subsystem

Scope: `src/audio/*` and `src/voice.rs`, with data flow traced through
main.rs, config.rs and the cache layer. Line numbers refer to the tree at the
time of writing.

## Major

### A1. Pitch correction silences any sample backed by a fully-loaded streaming buffer

`src/audio/mixer.rs:664-666, 835-838` with `src/audio/streaming.rs:213-220,
259-264`. The dispatch guard routes with `pitch_corrector.is_some() &&
buffer.is_complete()` — true for a `Streaming` buffer once `mark_complete()` is
called — but inside `mix_sample_with_pitch_correction`, `as_complete()` matches
on the *variant* and returns `None` for `Streaming`, so the function returns
without mixing and there is no fallback. Samples from `get_or_load_streaming`
keep the Streaming variant forever. Scenario: play an uncached file, send
`speed` with `pitch_correction: true`; while loading it plays varispeed
(wrong pitch), then goes completely silent the moment loading finishes.

### A2. Streaming samples can be killed by lock contention, and are truncated on loader stalls

`src/audio/streaming.rs:185-192`, `src/audio/mixer.rs:346-357`, loader at
`src/cache/mod.rs:237-238`. `SampleBuffer::frames()` for a streaming buffer is
`try_read().map(frames_available).unwrap_or(0)` — under contention it reports
0. `is_finished()` does `position >= frames()`, so if the callback's `retain`
runs while the loader holds the write lock, every non-looping streaming sample
is removed mid-playback. Separately, `advance_position` runs unconditionally,
so a decoder/network stall lets `position` overrun the loaded frontier and the
sample is dropped rather than buffering.

### A3. A sample created while the loader holds the write lock is permanently silent

`src/audio/mixer.rs:152-155, 190-193` with `streaming.rs:165-172`.
`ActiveSample::new_with_id` builds default routing from `buffer.channels()`,
which returns `unwrap_or(0)` on contention — the resulting empty channel map
plays silence for the sample's entire life, no error.

### A4. `audio.buffer_size` parsed, validated, documented — never used

`config.rs:54,158,777-780` vs `src/audio/engine.rs:235,250`:
`find_output_config` always returns `cpal::BufferSize::Default`. The startup
log prints the device default, compounding confusion.
(Also in findings-commands-config.md M3.)

### A5. Finished samples are never removed from `VoiceManager`

`src/voice.rs:100-109, 172-175` (`remove_sample`, `cleanup_empty_voices` have
no production callers). Sample IDs are added on every Play; removed only by
explicit `voice_stop`. A Play without `voice` generates a fresh
`_auto_<millis>` voice each time, so a long-running daemon accumulates one
Voice per playback, and `list_voices` reports phantom sample counts. Related:
`DuckingEngine.active_voices` (`ducking.rs:140-152`) also grows without bound.

### A6. Ducking fades run N× too fast when a voice has N samples; members get different multipliers

`src/audio/mixer.rs:606-629` with `src/audio/ducking.rs:77-93, 156-163`.
`get_multiplier(voice, frames)` advances the shared per-voice `DuckState` once
per active sample and once per live input, per callback. Two tracks on one
voice make a 2000 ms duck complete in 1000 ms, and the two get momentarily
different gains within the same callback.

### A7. Ducking release/restore time hardcoded to 2000 ms

`src/audio/ducking.rs:198-207`: `let fade_duration_ms = 2000;` — the comment
above ("Use the longest fade duration from previous rules") describes code
that does not exist. A rule with `fade_duration_ms: 100` ducks in 100 ms but
always recovers in 2 s; nothing in config can change it.

### A8. Reverse playback interpolates against the wrong neighbor

`src/audio/mixer.rs:739-755`. For reverse the code blends with the *lower*
frame using inverted weights, rendering position ~2.3 when asked for 3.7. At
non-integer reverse speeds the rendered position sequence is non-monotonic —
audibly garbled. The forward branch is correct for both directions.

### A9. Real-time violations in the audio callback path

`src/audio/mixer.rs:592-599, 862` and `ducking.rs:140-152, 175-250`.
`mix_sample_with_pitch_correction` allocates a `vec!` per callback per
pitch-corrected sample; main.rs:645-661 (inside the callback) builds
`HashSet<String>`s with cloned strings and can emit `tracing::debug!`. Glitch
risk exactly when a cue ends. (Adjacent to, but distinct from, the mutex note
in docs/bugs.md.)

## Minor

- **Loop crossfade double-plays the loop head** (`mixer.rs:777-807, 401-416`):
  head frames `[0, cf)` are blended in under the tail, then played again at
  full level after the wrap — audible double-attack every loop. Mirrored in
  the reverse branch.
- **Loop + pitch correction don't compose** (`mixer.rs:844-858`): no
  wrap-around when feeding the stretcher — glitch/gap at every seam;
  `crossfade_samples` ignored on this path.
- **Pitch correction on a still-loading file silently degrades to varispeed**
  (`mixer.rs:663-666`), then goes silent per A1.
- **`begin_duck` restarts the fade on every notification** (`ducking.rs:228-249`)
  even with unchanged parameters — frequent unrelated voice activity converts
  a linear fade into an asymptotic crawl.
- **Ducking rules are never validated** (`config.rs` has no ducking block):
  `target_volume: 5.0`, negatives, and never-matching voice names load
  silently.
- **Mic voice as `primary_voice` is silently inert** — nothing calls
  `notify_voice_active` for live inputs (see findings-docs-consistency.md C3).
- **Seek rough edges** (`main.rs:1152-1154`, `mixer.rs:111`,
  `pitch_correction.rs:66-70`): clamps to the streaming load frontier
  (inconsistent with Play's `total_frames_or_estimate()`); `fractional_position`
  not reset; `PitchCorrector::reset()` exists "for seek operations" but is
  never called (pre-seek audio smears out of the stretcher).
- **Bass management silently no-ops when `lfe_channel` is out of range for the
  opened device** (`bass_management.rs:187-190`) — no startup warning.
  Duplicate `source_channels` entries run the same filter twice per frame,
  corrupting its state.
- **One-shot `resample()` never drains the sinc filter** (`resampler.rs:90-118`):
  output starts with latency-frames of near-silence and the file tail is
  dropped; `ChunkedResampler::flush()` has a milder version.
- **Input capture resampler ignores `advanced.resampler_quality`**
  (`input.rs:463-469` hardcodes the Maximum preset on the capture thread);
  `pending` vectors there can also reallocate in the capture callback.
- **One corrupt packet aborts the entire decode** (`decoder.rs:140`,
  `streaming_decoder.rs:252`): Symphonia `DecodeError` is documented as
  recoverable/skippable; a single bad MP3 frame makes the file unplayable.
- **`speed: 0` silently coerced to 0.01** (`mixer.rs:275-281`) — an extreme
  drone instead of pause/error; negative speed at position 0 is instantly
  "finished" (reverse-from-start needs a prior seek, undocumented).
- **Instant gain changes pop** — `input_volume`, `input_mute`, per-sample
  `volume` (main.rs:1071, 1109, 1122, 1248) write directly while
  `voice_volume` ramps click-free.
- **`--test-mixer` hardcodes developer-machine paths**
  (`engine.rs:419-422`: `/Users/bandrews/...`) — fails on any other machine.

## Verified fine

- `advanced.resampler_quality` is genuinely used for playback decode paths
  (only input capture bypasses it).
- `audio.channel_volumes` works end-to-end (index/alias/name), applied
  post-bass-management.
- Bass management options wired and functional; filters are textbook
  Butterworth biquads (note: LP2+HP2 at same cutoff has small ripple —
  Linkwitz-Riley would sum flat).
- `inputs[].channels` / `sample_rate` wired, with good error messages.
- Ring-buffer frame alignment careful on both ends; no interleave rotation.
- Channel-map bounds checks in all mix paths prevent panics.
- Ducking mid-fade restart-from-current math is correct when parameters
  legitimately change.
- Backlog trimming bounds capture latency correctly.
- Decoder sample-format conversions correct; NaN not reachable via JSON.
