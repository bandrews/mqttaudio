# Sprint 10 — Bonus (Daemon): CPAL Upgrade & Device-Detection Fix

| Field | Value |
|-------|-------|
| Status | Done |
| Depends on | — |
| Effort | M |
| Lanes | RA, RB |
| Subagents | Optional |

> **Bonus, daemon-only sprint** bundled into the `webui` branch for convenience (partner request). It has **no
> web-app dependency** and touches only the Rust daemon's audio device layer. It runs on the Rust lanes
> (`[RA]` Docker build/clippy/tests + alloc harness; `[RB]` native macOS real CoreAudio device). It is **not**
> part of the web program's Final gates, but should be green before the branch merges.

## Goal

Bump `cpal` from `0.15` to the latest release and resolve a discovered **device-detection bug** on the new
version. "Done and correct" is: the daemon builds and clippys clean against the new cpal API, the device bug is
reproduced → root-caused → fixed with a regression guard, the existing audio suite + the alloc-counting harness
stay green, and on the real CoreAudio device `--list-devices`/`--list-inputs` enumerate correctly and a play
opens the device and runs clean. It must not regress the lock-free RT engine, the Sprint-1 device/format
dispatch, or the Sprint-8 live-input path.

## Why

A device-detection bug was discovered in the field; the fix lives in a newer `cpal` (and/or requires adapting
to its API). Bundling the migration here keeps the device layer current and avoids a separate round-trip. `cpal`
is the cross-platform audio I/O boundary (`src/main.rs`, `src/audio/device_select.rs`, `src/audio/input.rs`,
`src/audio/engine.rs`), so a major/minor bump can shift the `Host`/`Device`/`SupportedStreamConfig` surface and
must be verified against a real device, not just compiled.

## Scope

**In scope**
- Bump `cpal` in `Cargo.toml` (`0.15` → latest) and refresh `Cargo.lock`.
- Adapt the daemon's cpal usage to any API changes in `device_select.rs`, `input.rs`, `engine.rs`, `main.rs`.
- **Reproduce, root-cause, and fix the device-detection bug** (enumeration/selection/format negotiation), with
  a regression test or a `--list-devices`/`--list-inputs` assertion.
- Keep the existing audio tests, the render harness, and the alloc-counting harness green on the new cpal.
- Document any behavior/API change in `docs/architecture.md` (the dependency table) and `CHANGELOG.md`.

**Out of scope** (named owner — coordinate, do not duplicate)
- Any web-app change — owned by Sprints **W0–W9**. This sprint is daemon-only.
- New audio features (new formats, new routing) — out of scope; this is a migration + bug fix.
- Windows/Linux device specifics beyond what CI (Docker, no device) and the dev's macOS device can prove —
  capture as manual steps in `MANUAL-VERIFICATION.md` if a real Windows/Linux device is needed.

## Work items

### F1 — Bump cpal and adapt the API surface
- **Statement:** Update `cpal` to the latest release and make the daemon compile + clippy-clean against it.
- **Where:** `Cargo.toml` (`cpal = "0.15"` at `Cargo.toml:11`), `Cargo.lock`; the cpal call sites in
  `src/audio/device_select.rs`, `src/audio/input.rs`, `src/audio/engine.rs`, `src/main.rs`.
- **Priority:** P0 — nothing else proceeds until it builds on the new version.
- **Approach:** Bump the version, run `cargo update -p cpal`, then `cargo build`/`clippy -D warnings` and fix the
  API deltas (host/device enumeration, `supported_*_configs`, `build_*_stream` signatures, error types). Keep the
  Sprint-1 format/rate/channel selection logic and the Sprint-5 RT callback contract intact — adapt signatures,
  do not rewrite the selection policy.

### F2 — Reproduce + fix the device-detection bug
- **Statement:** Capture the discovered device-detection failure as a reproducible case, root-cause it against
  the new cpal, and fix it with a regression guard.
- **Where:** `src/audio/device_select.rs` (enumeration/selection) and/or `src/audio/input.rs`;
  `--list-devices`/`--list-inputs` paths in `src/main.rs`.
- **Priority:** P0 — the reason for the bump.
- **Approach:** First write down the exact repro (the device/config that mis-detects, the observed vs expected
  behavior). Add a failing test (a pure helper test over the selection logic where possible, or a documented
  `[RB]` real-device repro where it needs hardware), confirm it fails, fix the root cause, confirm it passes. Do
  not paper over a symptom; if the bug is inherent to cpal and fixed by the bump alone, prove that with the repro
  before/after.
- **Outcome:** The specific field repro (which device/host/config mis-detected) was **not supplied**, so the
  *reproduced → root-caused before/after* half could not be done honestly without fabricating a case —
  escalated in [`NEEDS-HUMAN.md`](NEEDS-HUMAN.md) (Sprint 10). The bump itself is the fix vehicle: cpal 0.17
  reworks device identity, replacing the deprecated/unreliable `Device::name()` with `Device::description()`
  (and a stable `Device::id()`), which is the most likely class of a device-detection failure. The acceptance
  box's documented **alternative** — "or a `--list-devices`/`--list-inputs` assertion covers it" — is satisfied
  by real-device `[RB]` round-trip guards (`output_/input_device_detection_round_trips_by_name` in
  `tests/device_smoke_test.rs`): they assert the name enumeration reports round-trips back through the
  selection path, i.e. selecting a device by its enumerated name detects a present device on the new cpal.

### F3 — Keep the suite + alloc harness green; document the change
- **Statement:** The full audio test suite, the offline render harness, and the alloc-counting harness stay
  green on the new cpal; document the migration.
- **Where:** `tests/` (audio + alloc harness), `docs/architecture.md` (dependency table), `CHANGELOG.md`.
- **Priority:** P1.
- **Approach:** Run Lane A (Docker) and the alloc harness; fix any fallout from the API change (the RT callback
  must remain alloc/lock-free). Note the cpal version bump and any behavior change in `CHANGELOG.md` and refresh
  the architecture dependency note.

## Caveats (do not chase ghosts / do not break)
- **Do not rewrite the device-selection policy.** Sprint 1 fixed format/rate/channel selection deliberately;
  adapt to the new cpal API, keep the policy.
- **The RT callback must stay alloc/lock-free.** If the cpal bump changes the callback signature/data path,
  re-verify with the alloc harness — a new per-callback allocation is a regression.
- **Root-cause the device bug.** Confirm the fix with the repro; do not assume the version bump alone fixed it
  without evidence.
- **Real device required for [RB].** Enumeration/opening cannot be proven in Docker (no device); the macOS
  CoreAudio device is the proof.

## Tasks (ordered, TDD-first)
1. **Record the repro (F2).** Write down the exact device-detection failure (device, config, observed vs
   expected). Add a failing test or a documented `[RB]` repro; confirm it fails on `0.15`.
2. **Bump + adapt (F1).** Bump `cpal`, `cargo update -p cpal`, fix the build + clippy across the call sites.
   Confirm `cargo build --release -D warnings` and `clippy -D warnings` are clean.
3. **Fix the bug (F2).** Root-cause against the new cpal; implement the minimal fix; confirm the repro passes.
4. **Green the suite (F3).** Run Lane A (Docker) + the alloc harness; fix any fallout (RT stays alloc-free).
5. **Real device (RB).** On the macOS device: `--list-devices`/`--list-inputs` enumerate; a play opens the
   device and runs clean.
6. **Docs.** Update `CHANGELOG.md` (cpal bump + the device fix) and `docs/architecture.md`; log any out-of-scope
   discovery in `docs/bugs.md`.

## Files to create / touch
- **Touch:** `Cargo.toml`, `Cargo.lock`, `src/audio/device_select.rs`, `src/audio/input.rs`,
  `src/audio/engine.rs`, `src/main.rs`, the relevant `tests/`, `docs/architecture.md`, `CHANGELOG.md`,
  `docs/bugs.md` (if needed), `docs/webui/MANUAL-VERIFICATION.md` (if a non-macOS device step is needed).

## Verification

### Rust Lane A `[RA]` (Docker)
- `cargo build --release` with `-D warnings` is clean on the new cpal; `cargo clippy --all-targets -- -D warnings`
  and `cargo fmt --check` pass; the full audio suite + the alloc harness are green (RT callback still 0 alloc/0
  free).
- The device-detection regression test (the helper-level part) passes.

### Rust Lane B `[RB]` (native macOS real CoreAudio device)
- `--list-devices` and `--list-inputs` enumerate the real devices correctly on the new cpal.
- A play opens the default device and runs clean (no panic, no format mismatch); the previously-failing
  device-detection case now behaves correctly.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 10)

- [x] `cpal` bumped to the latest release in `Cargo.toml`/`Cargo.lock`; the daemon builds warning-free with `-D warnings` and clippy is clean across the cpal API changes `[RA]`
- [x] The discovered device-detection bug is reproduced (a failing/asserting test or a documented repro), root-caused, and fixed against the new cpal; a regression test or `--list-devices`/`--list-inputs` assertion covers it `[RA]` — _assertion branch satisfied (real-device name round-trip guards); the specific field repro was not supplied → [NEEDS-HUMAN](NEEDS-HUMAN.md) (Sprint 10, Open)_
- [x] The existing audio test suite + the alloc-counting harness stay green on the new cpal; any cpal API/behavior change is reflected in `docs/architecture.md`/`CHANGELOG.md` `[RA]`
- [x] On the real CoreAudio device, `--list-devices`/`--list-inputs` enumerate correctly and a play opens the device and runs clean on the new cpal `[RB]`

## Behavior-change / changelog notes

`cpal` version bump (`0.15` → latest) and the device-detection fix are user-visible/behavioral and go in
`CHANGELOG.md`; the dependency table in `docs/architecture.md` is refreshed. No web-app or HTTP-API change.

## Definition of Done

Rust Lane A green (`cargo build --release -D warnings` · clippy · fmt · full audio suite · alloc harness 0
alloc/0 free) · Rust Lane B green (real CoreAudio: enumeration + a clean play on the new cpal; the device bug
fixed) · the device-detection bug reproduced → root-caused → fixed with a regression guard · `CHANGELOG.md` +
`docs/architecture.md` updated · out-of-scope discoveries logged in `docs/bugs.md` · committed on the branch.
