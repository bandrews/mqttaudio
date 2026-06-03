# Known bugs & out-of-scope discoveries

This file tracks bugs and oversights noticed while working that are **out of scope** for the change in
progress, so they aren't lost. Each entry names the owning sprint where known.

## Deferred to a later sprint

- **`await_holding_lock` in the command/precache paths (Sprint 2, D6).** `handle_command`'s `Play` and
  `Precache` arms, and the two startup precache loops in `main()`, hold the `cache_manager` `Mutex` guard
  across an `.await` (`src/main.rs`). This is the mutex-across-await reliability finding. It is currently
  suppressed with targeted `#[allow(clippy::await_holding_lock)]` (on `handle_command` and on `main`) so the
  Sprint 0 validation gate is green. Sprint 2 must drop the guard before awaiting (decode via
  `spawn_blocking`, internally-synchronized cache) and remove those allows.

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
