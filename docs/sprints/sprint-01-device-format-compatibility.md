# Sprint 1 — Device & Format Compatibility

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A, B, C |
| Subagents | optional |

## Goal

Make the daemon start and play on the audio devices real installs actually use — not just macOS CoreAudio.
Today the output path assumes the device natively exposes `f32`, hard-codes an `f32` callback, and
`.expect()`s the stream build. On Windows WASAPI shared mode (typically i16/i32) and on many raw ALSA `hw:`
devices (S16/S24/S32 only) that assumption is false and the daemon **panics at startup**. This sprint
resolves the device's real sample format, builds a typed stream that matches it (keeping an `f32` internal
mix bus + a thin convert shim), and hardens the surrounding selection logic — channel fallback, discrete
rate choice, buffer-size honoring, and stream rebuild on device error. All selection logic is refactored
into **pure helpers** so Lane A can unit-test format/rate/channel choice with no device present.

## Why

The COMPATIBILITY audit found that the entire output path is f32-only and that the config-selection logic
never inspects sample format, so even devices that *do* support f32 somewhere can be handed a non-f32 config.
The headline item is a **guaranteed startup crash** on common Windows and Linux hardware; the rest are
correctness/robustness defects that make device handling brittle. This sprint depends on Sprint 0 because it
needs the render harness (rms/peak/band_energy/inter-sample-delta) to prove the convert shim is transparent,
the Docker pipeline to gate the pure-helper tests (Lane A), and the device-test gating to run the real
CoreAudio smoke (Lane B).

## Scope

**In scope**
- Resolve the device's real `SupportedStreamConfig` (channels + rate + **sample_format**) and build a typed
  output stream dispatched over `I16`/`U16`/`I32`/`F32`, with an `f32` mix bus and a per-sample convert shim.
- Replace the `.expect("Failed to build audio stream")` with a graceful error path + Linux `plughw:`/`default`
  fallback recommendation.
- `find_output_config`: filter candidate ranges by acceptable `sample_format` and return the chosen format
  alongside channels/rate.
- Channel-count fallback (next-larger range + zero-fill extras) instead of exact-match-or-exit.
- Discrete-rate selection: pick the nearest device-supported rate (reuse `NativeCapabilities.discrete_rates`)
  and validate the chosen config against `supported_output_configs` before building.
- Honor `audio.buffer_size` via `BufferSize::Fixed(n)` within the device's supported range, falling back to
  `Default` if unsupported.
- Rebuild the output (and input) stream on a device-error callback with backoff.
- ALSA name matching: include DEV/subdevice, broaden prefixes to match `classify_device_name`, cache the
  `/proc/asound/cards` read.
- Refactor the format/rate/channel selection into **pure helpers** over a `Vec` of
  `SupportedStreamConfigRange`-equivalent data, unit-testable without a device.

**Out of scope** (do not touch here)
- Input/output **clock drift / async SRC** between separate devices — that is the
  COMPATIBILITY/`input.rs` finding owned by **Sprint 8** (`needs_resampling` from nominal rates, static
  resample ratio). It appears in the same dump section; leave it. If you touch `input.rs` for the
  error-callback rebuild, do **not** alter the resampling logic.
- Any mixer/DSP behavior (Sprints 6–7).
- The `cache_manager`-across-`.await` and MQTT-resubscribe reliability items (Sprint 2).

## Findings addressed

> Each finding is summarized from the COMPATIBILITY section of the findings dump (plus one RELIABILITY item).
> Every `file:line` below was re-verified against the current source before writing this sprint.

### F1 — Output stream hard-codes an f32 callback and `.expect()`s the build (CRITICAL, confirmed)
- **Statement:** Non-f32 devices panic at startup because the output stream is built as `f32`-only.
- **Where:** `src/main.rs:553-587` (callback `move |data: &mut [f32], …|`, terminated by
  `.expect("Failed to build audio stream")` at `main.rs:587`); config built by `find_output_config`
  (`src/audio/engine.rs:266-356`).
- **Severity:** Critical (guaranteed startup crash on common hardware).
- **Evidence:** `device.build_output_stream(&stream_config, move |data: &mut [f32], _| {…}, |err| {…}, None)`
  (`main.rs:553-582`) is unwrapped with `.expect(…)` (`main.rs:587`). `stream_config` comes from
  `find_output_config`, which returns a bare `cpal::StreamConfig{channels, sample_rate, buffer_size}`
  (`engine.rs:334-338, 349-353`) and never inspects `sample_format()`. The only `sample_format()` call in any
  output path is a log line in the dev-only `init_test_sine_wave` (`engine.rs:372`). cpal 0.15.3 requires
  native `f32` support for `build_output_stream::<f32>`; otherwise it returns `StreamConfigNotSupported`,
  which the `.expect` turns into a panic. The code's own ALSA probe already detects non-f32 `hw:` devices and
  prints "Requires {fmt} format (use plughw: instead)" (`src/audio/alsa_probe.rs:313-318`), so non-f32
  devices are known to exist. Affects Windows WASAPI shared (i16/i32) and raw ALSA `hw:` S16/S24/S32 devices.
- **Fix:** Resolve `SupportedStreamConfig` (default + `supported_output_configs()`), pick a supported format,
  and dispatch to a typed `build_output_stream::<i16|u16|i32|f32>` that converts the f32 mix bus to the
  target type via cpal `Sample`/`FromSample`. Replace `.expect` with a graceful error; on Linux recommend/
  fall back to `plughw:`/`default` (which do f32 conversion in the ALSA plug layer).

### F2 — `find_output_config` ignores `SupportedStreamConfigRange.sample_format` (HIGH, confirmed)
- **Statement:** Selection filters only on channel count and returns a format-less `StreamConfig`, so the
  chosen (channels, rate) range may belong to a non-f32-only range.
- **Where:** `src/audio/engine.rs:274-355` (channel filter `engine.rs:295-297`; rate selection
  `engine.rs:323-329` / `engine.rs:342`; returned `StreamConfig` `engine.rs:334-338, 349-353`).
- **Severity:** High (feeds F1; even f32-capable devices can land on a non-f32 range).
- **Evidence:** `supported_output_configs()?.collect()` yields `SupportedStreamConfigRange` entries, each
  specific to one `(channels, sample_format, buffer range, rate range)` tuple in cpal 0.15. The code filters
  only `c.channels() == target_channels` and never consults `sample_format()`; the returned `StreamConfig`
  carries no format. The "No configuration found…" error path (`engine.rs:307-310`) and the rate clamp
  likewise reason only about channels/rates.
- **Fix:** Filter ranges by an acceptable `sample_format()` (prefer F32, else the dispatch format chosen in
  F1) and return the format alongside channels/rate so the caller builds the matching typed stream.

### F3 — No fallback when the requested channel count is unavailable (MEDIUM)
- **Statement:** Exact channel-count match is required; otherwise the daemon errors and exits.
- **Where:** `src/audio/engine.rs:281-311` (target from `requested_channels` `engine.rs:281-283`; exact filter
  `engine.rs:295-297`; error `engine.rs:299-311`); process exit at `src/main.rs:380`.
- **Severity:** Medium.
- **Evidence:** `matching_configs` requires `c.channels() == target_channels`. If no range reports that exact
  count, `find_output_config` returns `"No configuration found for N channels. Available: …"`
  (`engine.rs:307-310`) and `main.rs:364-380` exits the process. So requesting 6 channels on a device
  enumerating only 2ch and 8ch ranges fails outright instead of opening 8 and using 6.
- **Fix:** If no exact match, fall back to the **smallest available channel count ≥ requested** (open more
  channels, zero-fill the extras in the callback). Only error if none can satisfy the request.

### F4 — Sample-rate clamp can pick a rate the device doesn't actually support (MEDIUM)
- **Statement:** When the requested rate isn't inside a matching range, the code arithmetic-clamps into
  `[min, max]` assuming a continuous range, which is wrong for discrete-only devices.
- **Where:** `src/audio/engine.rs:331-354` (`config_range = matching_configs[0]`, then
  `sample_rate = target.max(min_rate).min(capped_max)` with `capped_max = max_rate.min(384000)`).
- **Severity:** Medium.
- **Evidence:** The fallback assumes `[min_rate, max_rate]` is continuous. For devices exposing only discrete
  rates (e.g. 44100/48000, or 48000/96000), an in-between clamped value (e.g. 64000) is invalid; cpal's
  `with_sample_rate` just stores it, and the stream build then fails (→ exit at `main.rs:380`) or the driver
  silently retunes (pitch/speed error). 44.1k-native vs 48k-default is the common trigger.
- **Fix:** Choose the **nearest device-supported discrete rate** rather than an arithmetic clamp. The probe
  logic already exists: `NativeCapabilities.discrete_rates` (`src/audio/device.rs:78-79`), populated by the
  ALSA probe (`src/audio/alsa_probe.rs:135-151`). Validate the chosen config against
  `supported_output_configs` before building.

### F5 — No device-disconnect / hotplug / default-change handling; error callback only logs (MEDIUM)
- **Statement:** The output stream is built once and kept for process lifetime; the error closure only logs,
  so a disconnect/default-switch silently stops audio forever.
- **Where:** `src/main.rs:553-590` (built once at `main.rs:553`; error closure `|err| { tracing::error!(…) }`
  at `main.rs:583-585`); device resolved once by `find_output_device` (`src/audio/engine.rs:115-159`).
  Same defect for **input** streams: `src/audio/input.rs:232-234` and `src/audio/input.rs:318-320`.
- **Severity:** Medium (acute for long-running unattended USB/Bluetooth/HDMI installs).
- **Evidence:** cpal delivers fatal stream errors (`DeviceNotAvailable` on USB unplug / ALSA fatal) through
  these callbacks; nothing rebuilds the stream. On macOS/Windows the default output device switches when
  headphones connect/disconnect and audio goes permanently silent with only a log line. This is also the
  RELIABILITY "device error callbacks only log — no stream rebuild or recovery" finding, for both output and
  input.
- **Fix:** On the stream-error callback, signal a non-RT supervisor to rebuild the stream (re-run
  `find_output_device`/`find_output_config`/`build_output_stream`) with backoff. Do **not** rebuild from
  inside the cpal error callback directly; hand off via a channel/flag and rebuild on a control thread.

### F6 — ALSA name matching ignores DEV/subdevice and unlisted prefixes; re-reads /proc per compare (LOW)
- **Statement:** `try_match_alsa_device` compares only the card id, ignores the subdevice, misses prefixes
  like `hdmi:`/`iec958:`, and re-reads `/proc/asound/cards` on every cross-compare.
- **Where:** `src/audio/engine.rs:163-261` (prefix list `engine.rs:166`/`engine.rs:189`; `try_match`
  `engine.rs:164-182`; `AlsaCardId` cross-compare reading `/proc/asound/cards` at `engine.rs:240-258`).
- **Severity:** Low (Linux-only, secondary fallback path; exact enumerated match at `engine.rs:121-129` is
  tried first).
- **Evidence:** `try_match_alsa_device` compares card identifiers only, so `hw:1,0` and `hw:1,3` (different
  subdevice, same card) compare equal. The `AlsaCardId::Index`↔`Name` cross-compare re-parses
  `/proc/asound/cards` per comparison and returns `false` on any read/parse failure. The prefix list
  (`engine.rs:166`) includes `surround`/`front:` but not `hdmi:`/`iec958:`/`dmix:`-with-card.
- **Fix:** Include the DEV/subdevice in the comparison, broaden the prefix list to match
  `classify_device_name` in `alsa_probe.rs`, and cache the `/proc/asound/cards` read instead of re-reading it
  per compare.

### F7 — `BufferSize::Default` ignores the validated `audio.buffer_size` (LOW, confirmed)
- **Statement:** The configured buffer size is validated but never applied; both return paths force
  `BufferSize::Default`.
- **Where:** `src/audio/engine.rs:337` and `src/audio/engine.rs:352` (`buffer_size: cpal::BufferSize::Default`);
  config field `audio.buffer_size` (`src/config.rs:46`, default 512), validated 64–8192
  (`src/config.rs:697-699`); `get_default_device_config` even hardcodes `buffer_size: 512` (`engine.rs:108`).
- **Severity:** Low (latency/quality knob, not a guaranteed glitch).
- **Evidence:** Both `Ok(cpal::StreamConfig{…})` returns set `BufferSize::Default`, so the user-facing
  `audio.buffer_size` is silently ignored; operators cannot tune latency/period size.
- **Fix:** Set `BufferSize::Fixed(n)` from `config.audio.buffer_size` when within the device's
  `supported_buffer_size` range; fall back to `Default` if out of range or unknown.

## Caveats (refuted / over-stated — don't chase ghosts)

- **Clock drift is NOT this sprint.** The COMPATIBILITY dump contains a HIGH "separate input and output
  devices have independent clocks with no drift compensation" item (`src/audio/input.rs:153-239,261`). It is
  **out of scope** here and owned by **Sprint 8**. Do not add async-SRC / fill-level feedback in this sprint.
- **F4 nuance:** the dump notes that for many devices the clamped value (e.g. 48000) *happens* to be valid;
  the bug bites specifically on discrete-only hardware and the 44.1k-vs-48k mismatch. Fix the selection
  correctly (nearest supported + validate), but the test that proves it must use a discrete-rate fixture, not
  a continuous one.
- **F6 is a secondary path:** the exact enumerated-name match (`engine.rs:121-129`) is tried first, so the
  worst realistic case is right-card/wrong-subdevice or a missed `hdmi:` device — keep the change minimal.
- **F1 fallback to `plughw:` is Linux-only.** On Windows/macOS the fix is the typed-stream dispatch itself;
  there is no `plughw:` equivalent — don't invent one.

## Tasks (ordered, TDD)

> Build the pure helpers first so every selection decision is unit-testable in Lane A without a device. The
> typed-stream dispatch and convert shim are proven transparent via the Sprint 0 render harness.

1. **Extract pure selection helpers (no behavior change yet).** Introduce a small format-agnostic input type
   mirroring `SupportedStreamConfigRange` (channels, `sample_format`, min/max rate, optional
   `discrete_rates`) and move the decision logic out of `find_output_config` into pure functions in
   `src/audio/engine.rs` (or a new `src/audio/device_select.rs`):
   - `select_channels(ranges, requested) -> Result<u16, SelectError>`
   - `select_sample_format(ranges, channels) -> Option<SampleFormat>` (prefer F32, else first cpal-supported)
   - `select_sample_rate(ranges, channels, format, requested) -> Result<u32, SelectError>`
   - `select_buffer_size(range, requested) -> BufferSize`
   These take plain data, no `cpal::Device`. **TDD:** before writing them, add failing unit tests in
   `tests/device_select_test.rs` (Lane A) asserting: 32-channel cap when `requested=None`; `SelectError` (no
   panic) when no channel range satisfies the request; in-range exact selection; format filtering (an
   i16-only range is rejected when an f32 range exists, and an i16 range is *chosen* when forced i16);
   nearest discrete-rate choice (request 64000 against a `discrete_rates=[44100,48000,96000]` fixture →
   48000). Confirm they fail, then implement minimally.

2. **Channel fallback (F3).** Extend `select_channels` so that when no exact match exists it returns the
   smallest available count ≥ requested. **TDD first:** a test with ranges `{2, 8}` requesting 6 returns 8;
   requesting 9 returns `SelectError`. Then thread zero-fill of the extra channels through the callback
   (the typed callback writes the mix bus to channels `0..mix_channels` and zeros `mix_channels..device_channels`).

3. **Discrete-rate selection (F4).** Implement `select_sample_rate` to pick the nearest supported rate:
   continuous range → clamp into `[min,max]`; discrete-only → nearest entry of `discrete_rates`. Reuse
   `NativeCapabilities.discrete_rates` (`device.rs:78-79`, probed at `alsa_probe.rs:135-151`). **TDD first**
   per task 1's discrete-rate test; add a continuous-range test (request 96000 in `[44100,192000]` → 96000).

4. **Format filtering + typed `find_output_config` return (F2).** Change `find_output_config` to return the
   chosen `sample_format` alongside channels/rate (e.g. a small `OutputFormatChoice { channels, sample_rate,
   sample_format, buffer_size }`), built from the pure helpers and **validated against
   `supported_output_configs` before returning**. **TDD:** unit test on the helper level (Lane A); update
   existing callers. No `cpal::Device` in the unit tests — only the Lane B smoke opens a real device.

5. **Buffer-size honoring (F7).** Implement `select_buffer_size`: `BufferSize::Fixed(config.audio.buffer_size)`
   when within the range's `SupportedBufferSize::Range`, else `Default`. **TDD:** helper test — in-range value
   → `Fixed(n)`; out-of-range → `Default`; `Unknown` range → `Default`.

6. **Typed output stream dispatch + convert shim (F1).** Add a `build_output_stream` dispatcher that matches
   on the chosen `SampleFormat` and builds `build_output_stream::<i16|u16|i32|f32>`, where each callback runs
   the existing f32 mix into a reusable f32 scratch bus and converts per sample via cpal `FromSample`. Keep
   `MixerState`/`mix_audio` untouched (still f32). Replace `.expect("Failed to build audio stream")`
   (`main.rs:587`) with a graceful error → `std::process::exit(1)` consistent with `main.rs:380`, plus a
   Linux-only log recommending `plughw:`/`default`. **TDD via the Sprint 0 render harness:** add a test that
   feeds a known sine through the convert shim for each target type and asserts, against the f32 reference,
   that `rms`/`peak` match within a tight tolerance, `band_energy` at the sine frequency dominates, and
   `max_inter_sample_delta` shows no discontinuity (quantization aside). The i16 path test is the
   "forced-i16 proves no panic" acceptance item. **Behavior-changing — see changelog.**

7. **Device-error rebuild with backoff (F5).** In the output error callback, set an atomic flag / send on a
   channel rather than only logging; add a non-RT supervisor that rebuilds the stream
   (`find_output_device`→`find_output_config`→dispatcher) with exponential backoff. Mirror the same flag→rebuild
   for the **input** error callbacks (`input.rs:232-234`, `input.rs:318-320`) **without** changing the
   resampling logic. **TDD:** a unit test on the backoff/state machine (e.g. `RebuildPolicy::next_delay()`
   sequence and reset-on-success) in Lane A; the actual hardware disconnect is a Lane C manual step.

8. **ALSA matching hardening (F6).** Include DEV/subdevice in `try_match_alsa_device`, broaden the prefix
   list to match `classify_device_name`, and cache the `/proc/asound/cards` read (parse once into a
   name→index map). **TDD:** pure unit tests for the matcher (`hw:1,0` vs `hw:1,3` → no match; `hw:1,0` vs
   `hw:CARD=UMC1820,DEV=0` with a stubbed cards map → match; `hdmi:1,0` recognized). Keep the change minimal —
   this is the secondary fallback path.

9. **Lane C doc (refine, don't duplicate).** `docs/sprints/MANUAL-VERIFICATION.md` already has **W-1 — Output
   starts on a WASAPI shared-mode (i16/i32) device _(added by Sprint 1)_**. Refine it so it explicitly
   verifies the **format dispatch** (logs show the negotiated format, e.g. `I16`/`I32`) and add the i32 case
   and the `--list-devices` + play smoke; do **not** add a second W-1 entry. Leave W-2 (Sprint 8) alone.

10. **Green the gate.** `cargo build --release` warning-free, clippy `-D warnings`, `fmt --check`; Lane A and
    Lane B both green; commit atomically; log any out-of-scope discoveries in `docs/bugs.md`.

## Files to create / touch

- **Create:** `tests/device_select_test.rs` (Lane A pure-helper tests). Optionally
  `src/audio/device_select.rs` if the helpers are cleaner in their own module (otherwise keep them in
  `engine.rs`). Convert-shim render-harness test under `tests/` (reusing Sprint 0's harness).
- **Touch:**
  - `src/audio/engine.rs` — extract pure helpers; `find_output_config` returns format + validated config;
    channel fallback; discrete-rate selection; `BufferSize::Fixed`; ALSA matcher (`165-261`) DEV/subdevice +
    cached `/proc` read.
  - `src/main.rs` — typed `build_output_stream` dispatcher + f32→target convert shim (`553-587`); replace the
    `.expect` (`587`) with graceful error; zero-fill extra channels in the callback; device-error
    flag→rebuild supervisor with backoff.
  - `src/audio/input.rs` — input error callbacks (`232-234`, `318-320`) signal rebuild (no resampling change).
  - `src/audio/device.rs` / `src/audio/alsa_probe.rs` — only if the discrete-rate helper needs a small
    accessor; prefer reusing `NativeCapabilities.discrete_rates` as-is.
  - `docs/sprints/MANUAL-VERIFICATION.md` — refine the existing **W-1** entry (Lane C).
  - `docs/sprints/SPRINT-TRACKER.md` — set Sprint 1 status; tick boxes only when genuinely verified.
  - `README.md`/`CHANGELOG.md` (behavior-change notes), `docs/bugs.md` as needed.

## Verification

**Lane A (Docker / Linux)** — `scripts/validate.sh`
- Pure-helper unit tests pass: 32-channel cap, no-match → `SelectError` (no panic), in-range selection,
  format filtering (f32 preferred; forced-i16 selectable), nearest-discrete-rate, channel fallback
  (next-larger), buffer-size `Fixed`/`Default` boundaries, ALSA matcher subdevice/prefix cases, rebuild
  backoff sequence.
- Render-harness convert-shim test: i16/u16/i32/f32 paths match the f32 reference within tolerance
  (`rms`/`peak`/`band_energy`/`max_inter_sample_delta`); the **forced-i16 path proves no panic**.
- `cargo build --release` (`-D warnings`), clippy `-D warnings`, `fmt --check` all clean.

**Lane B (native macOS, real device)** — `scripts/validate.sh --native`
- `--list-devices` lists the default CoreAudio device with channels/rate.
- A play smoke on the real default CoreAudio device goes through the new format dispatch and produces audio
  (device-test gated per Sprint 0); no panic, no regression vs the prior f32 path.

**Lane C (manual Windows, partner)** — `docs/sprints/MANUAL-VERIFICATION.md` **W-1** (refined)
- WASAPI shared-mode device: `--list-devices`, then play a known WAV; process does **not** panic; logs show
  the negotiated format (e.g. `I16`/`I32`) and rate; audio plays cleanly. Repeat with `--sample-rate 44100`
  and `--channels 2`.

## Acceptance criteria

> Copied verbatim from `docs/sprints/SPRINT-TRACKER.md` → "Sprint 1 — Device & format compatibility".

- [ ] Pure-helper unit tests for sample-format / rate / channel selection pass; a forced-i16 path proves no panic `[A]`
- [ ] Output builds a typed stream matching the device's native format (I16/U16/I32/F32) with an f32 mix bus + convert shim; `.expect` replaced with graceful error/fallback `[A]`
- [ ] `find_output_config` filters by `sample_format` and returns it; nearest-supported discrete rate chosen; channel-count fallback (next-larger + zero-fill) `[A]`
- [ ] `audio.buffer_size` honored via `BufferSize::Fixed` within device range `[A]`
- [ ] Device-error callback rebuilds the stream with backoff `[A]`
- [ ] `--list-devices` + play smoke runs on the real CoreAudio device via the format dispatch `[B]`
- [ ] WASAPI shared (i16/i32) `--list-devices` + play smoke documented and run `[C]`

## Behavior-change / changelog notes

These are user-visible / behavior-changing and belong in `CHANGELOG.md` / `README.md`:

- **Non-f32 output devices now work.** The output stream is built to match the device's native sample format
  (I16/U16/I32/F32) instead of assuming f32; non-f32 Windows WASAPI shared and ALSA `hw:` devices no longer
  panic at startup. (F1)
- **Graceful failure instead of panic** when a stream can't be built; Linux logs a `plughw:`/`default`
  recommendation. (F1)
- **Channel-count fallback:** requesting a channel count the device doesn't expose exactly now opens the
  next-larger configuration and zero-fills the extras instead of exiting. (F3)
- **`audio.buffer_size` is now honored** (`BufferSize::Fixed`) where the device supports it; previously the
  value was silently ignored. Latency/period size may change for existing configs. (F7)
- **Output (and input) streams now auto-recover** by rebuilding on device error with backoff instead of going
  permanently silent. (F5)
- **Sample-rate selection now picks the nearest device-supported (discrete) rate** and validates the config
  before building; some devices may negotiate a different rate than the previous arithmetic clamp produced. (F4)

> **Decided upfront (DECISIONS.md D1–D4):** Tasks 6 (typed-stream dispatch / convert shim) and 3+2 (channel
> fallback semantics) change runtime audio behavior and the negotiated device format. Record the
> behavior-change list above in `CHANGELOG.md` before marking the sprint Done. The zero-fill-extra-channels
> choice (F3) is a routing decision — implement it per DECISIONS.md (D1) rather than, e.g., refusing the open.

## Definition of Done

Lane A green · Lane B green (incl. the real CoreAudio `--list-devices` + play smoke through the format
dispatch) · pure-helper unit tests + the render-harness convert-shim test landed · `.expect` replaced with a
graceful path · F1–F7 fixes implemented at root cause (no tests disabled/`#[ignore]`d to pass, no symptom
patches) · clock-drift left untouched for Sprint 8 · **W-1** refined in `MANUAL-VERIFICATION.md` (not
duplicated) · behavior-change list recorded in the changelog · `CHANGELOG.md`/`README.md` updated ·
out-of-scope discoveries logged in `docs/bugs.md` · committed atomically to the branch as units complete ·
`cargo build --release` warning-free.
