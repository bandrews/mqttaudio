# Known bugs & out-of-scope discoveries

This file tracks bugs and oversights noticed while working that are **out of scope** for the change in
progress, so they aren't lost. Each entry names the owning sprint where known.

## Deferred to a later sprint

- **`xruns` counter is incremented but not surfaced on `/status` (Sprint 5).** The lock-free RT engine
  creates an `xruns: AtomicU64` (`src/main.rs:553`), passes it to the output supervisor, and increments it
  in the cpal error callback (`src/audio/engine.rs`), but it is not yet plumbed into the control-side
  `StatusSnapshot` or the `/status` JSON. Sprint 5's acceptance ("expose it on the status snapshot for the
  soak test") and Sprint 9's `/metrics` work still need to wire it through. The counter is live and usable
  by a soak test that holds the `Arc<AtomicU64>` directly; only the HTTP exposure is missing.

- **`/status/samples` live position is gone by design (Sprint 5 / D20 / D22a).** Because the control plane
  never reads `MixerState` (D22a), the status snapshot is control-side and cannot see audio-thread-owned
  playback position. `/status/samples` therefore reports `position`/`position_ms`/`progress_percent` as 0
  (static metadata and `total_ms` are still accurate). If live position is wanted back, it must be fed to
  the control thread over a dedicated channel (the audio thread publishing per-voice positions), not by
  locking the callback state. Recorded as a deliberate behavior change, not a regression; changelog'd.

- **`DiskCache` uses `DefaultHasher` for cache keys (Sprint 3).** `src/cache/disk.rs:~147` derives the
  on-disk filename from `std::collections::hash_map::DefaultHasher`, which is not guaranteed stable across
  releases/platforms. Sprint 3 should switch to a stable content hash (truncated SHA-256 / xxhash).

- **`CacheError` variant names (`IoError`/`JsonError`/`HttpError`) (Sprint 9).** clippy's
  `enum_variant_names` fires on the shared `Error` suffix. Renaming ripples through many match arms, so it
  is suppressed with a targeted `#[allow(clippy::enum_variant_names)]` for now (`src/cache/disk.rs`).

- **Stale `#![allow(dead_code)]` "Phase 10" banner (Sprint 9, D39).** `src/audio/streaming_decoder.rs`
  still carries a crate-style `#![allow(dead_code)]`. Sprint 9 removes dev scaffolding and must make the
  build warning-free without this blanket allow.

- **Hardcoded `/Users/bandrews/...` paths (Sprint 9, D39).** `audio::engine::test_mixer` embeds absolute
  developer paths for its test files. Sprint 9 removes `test_mixer`/`--test-mixer` and the dev scaffolding.

- **ALSA name matcher hardening deferred (Sprint 1 F6 — LOW, Linux-only).** `try_match_alsa_device` /
  `extract_alsa_card_from_name` in `src/audio/engine.rs` compare only the card identifier (ignoring the
  DEV/subdevice), miss some prefixes (`hdmi:`/`iec958:`), and re-read `/proc/asound/cards` per comparison.
  This is the *secondary* device-resolution path — the exact enumerated-name match is tried first and
  handles the common case — so the realistic worst case is right-card/wrong-subdevice or a missed `hdmi:`
  device when selecting by a non-enumerated name. The fix (subdevice comparison + a cached `/proc` map) is
  Linux-only and can only be verified in Lane A; it was deferred to keep Sprint 1 focused on the critical
  non-f32 crash and the gating acceptance criteria. Pick it up when a real device exposes the mismatch
  (also a fit for Sprint 9's device cleanup). Prefix broadening is the cheap first step.

- **Chunked-vs-one-shot resampler divergence (Sprint 4 caveat — LOW, consistency-only).** The streaming
  decode path resamples in chunks (`StreamingDecoder` + `ChunkedResampler`), zero-padding the final
  partial chunk and truncating to the expected frame count without compensating rubato's startup delay,
  while disk-cache hits and local files use the one-shot `decoder::decode_file`. The two produce slightly
  different PCM for the same source (the `matches_full_decode` test tolerates the diff); it is not an
  audible glitch. Sprint 4's promotion (F2) means a streamed URL resolves to the one-shot path on replay,
  so the divergence only affects the first, still-streaming play. Left for Sprint 9 cleanup — do not
  rewrite the resampler for this.

## Architectural notes

- **Binary re-declares modules instead of using the library crate.** `src/main.rs` declares its own
  `mod audio; mod cache; mod config; mod http; mod mqtt; mod voice;` rather than depending on the
  `mqttaudio` lib, so the modules are compiled twice and the two trees have drifted (the lib's `mqtt`
  module omits `logger`, which the binary uses). Because of this, `handle_command`/`CommandCtx` live in the
  binary crate and are covered by binary unit tests. Unifying the binary onto the library crate would halve
  compile time and make the command path testable from integration tests, but it is a larger refactor than
  the validation-harness sprint should take on. Candidate for Sprint 9.

## Fixed in passing (Sprint 0, to make the gate meaningful)

- `benches/mixer_benchmark.rs` no longer compiled: `ActiveSample::new_with_mapping` had gained
  `loop_mode`/`crossfade_samples` parameters that the bench call didn't pass. Updated the call.
- Removed two unused `#[cfg(test)]` methods (`BassManagement::reset`, `BassManagement::sample_rate`) that
  had no callers and tripped `dead_code` under `-D warnings`.
- Test reads in `src/cache/http_stream.rs` used `Read::read` and ignored the returned count
  (`clippy::unused_io_amount`, deny-by-default); switched to `read_exact`, which is what the assertions
  already assumed.
