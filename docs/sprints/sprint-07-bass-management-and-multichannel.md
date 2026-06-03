# Sprint 7 — Bass Management & Multichannel

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 5 |
| Effort | M |
| Lanes | A |
| Subagents | Optional |

## Goal

Make the bass-management / crossover path **acoustically correct and level-stable** for the multichannel
installation use case, and **exercise it through the real `mix_audio` path** for the first time. Replace the
phase-cancelling 2nd-order Butterworth split with a 4th-order Linkwitz-Riley crossover so the mains+sub
recombine flat and in-phase at the crossover; make LFE level independent of how many source channels feed it;
make the silent failure modes (no LFE channel, double-counted bass, denormal CPU spikes) visible and
configurable. Every audio-output change is proven with the Sprint-0 render harness
(`rms` / `peak` / `band_energy` / `max_inter_sample_delta`).

## Why

All of the bass-management findings come from a code path that, today, has **zero integration coverage through
`mix_audio`** — Sprint 0's audit flagged that all 39 `MixerState::new(2)` test constructions set
`bass_management: None` (`mixer.rs:526-536`), so `bm.process()` is only ever tested in isolation
(`bass_management.rs` unit tests), never as wired into the production mix loop at `mixer.rs:580-581`. The
crossover defects below are real DSP/level-calibration bugs for anyone who turns the feature on, and they are
currently invisible to the test suite.

Two scoping facts that shape this sprint:

- **The feature is off by default.** `BassManagementConfig::enabled` defaults to `false`
  (`config.rs:237`, `bass_management.rs:125`), and `mix_audio` only calls `bm.process()` when
  `state.bass_management` is `Some` (`mixer.rs:580`). So every finding here affects **only users who opt in** by
  setting `bass_management.enabled = true`. That is why effort is **M**, not L.
- **One item is a design decision, not a forced fix.** The "double bass" finding describes a recognized
  *LFE+Main* mode, so it needs a **default + docs decision per DECISIONS.md**, not a unilateral behavior
  change (see Caveats and the changelog section).

This sprint depends on Sprint 5 (lock-free RT engine) because the denormal-flush change touches the per-sample
biquad on the audio thread, and the integration test drives `mix_audio` — both must sit on top of the
post-Sprint-5 callback/ownership model rather than the current `Arc<Mutex<MixerState>>`.

## Scope

**In scope**
- 4th-order Linkwitz-Riley crossover (cascade two identical Butterworth biquads for both LP and HP) replacing
  the single 2nd-order sections in `BassManagement` (`bass_management.rs:16-63`, `147-167`, `205`, `210`).
- LFE gain compensation so sub level is independent of active source-channel count
  (`bass_management.rs:192-216`).
- A **default + docs decision** for `remove_bass_from_sources` (the locked decision in DECISIONS.md), plus a one-time
  construction-time warning when `lfe_channel >= output_channels`, and documentation of the additive-LFE /
  LFE-collision behavior.
- Denormal flush-to-zero in the biquad IIR (`bass_management.rs:90-102`).
- An **integration test that runs `mix_audio` with `bass_management: Some(...)`** (closes the Sprint-0 "bass
  never exercised through `mix_audio`" gap), using the render harness.

**Out of scope** (log to `docs/bugs.md` if touched)
- The hard-clamp limiter at `mixer.rs:584-587` (owned by Sprint 6 — true-peak/soft-knee limiter). Do not change
  the clamp here; just be aware bass-managed bass is summed *before* it.
- NaN/non-finite sanitisation of the summed mix (Sprint 6). The denormal flush here is a different, narrower
  concern (tiny-but-finite values, not NaN/Inf) confined to the biquad state.
- Per-channel `channel_volumes` calibration (Sprint 6) and any downmix/upmix gain in the routing matrix
  (Sprint 6 / Sprint 8).
- Runtime reconfiguration of bass management (none exists; `sample_rate` is stored but unused —
  `bass_management.rs:141-142`). Not needed (YAGNI).

## Findings addressed

Severities are as reconciled in the findings dump (BASS-MANAGEMENT section). All file:line citations below were
re-verified against current source before writing.

1. **Bass double-counted into mains AND LFE by default** — MEDIUM (confirmed; over-stated as high).
   With `remove_bass_from_sources = false` (the default), the low-passed bass is unconditionally summed into the
   LFE while the source channel is left full-range.
   - Verified: `bass_management.rs:206` (`lfe_sum += bass;`), `:216` (`output[lfe_idx] += lfe_sum;`); the source
     high-pass runs only inside `if self.config.remove_bass_from_sources` at `:209-211`. Default `false` at
     `bass_management.rs:129` and `config.rs:241`.
   - Evidence: in the default config the same sub-80 Hz energy is emitted by the mains *and* added to the sub →
     ~+6 dB correlated bass and comb filtering with any path/phase difference.
   - **Caveat (over-stated):** this is the recognized **LFE+Main / "Double Bass"** mode (AVRs expose it
     deliberately; `test_bass_management_adds_to_existing_lfe` at `bass_management.rs:640-680` shows the author
     intended it as selectable). So the **fix is a default + docs decision**, not a forced behavior change —
     **decided upfront in DECISIONS.md** (see Caveats and changelog).

2. **2nd-order Butterworth crossover → 180° cancellation at fc (not Linkwitz-Riley)** — MEDIUM (confirmed;
   over-stated as high).
   - Verified: `lowpass()` (`bass_management.rs:22`) and `highpass()` (`:46`) both set
     `alpha = sin_omega / (2.0_f32).sqrt()`, i.e. a single Q=1/√2 Butterworth biquad (12 dB/oct, −3 dB at fc).
     The crossover wires exactly one LP and one HP per source (construction `:147-167`; applied at `:205`, `:210`).
   - Evidence: a 2nd-order Butterworth LP and HP at the same cutoff are 180° out of phase at fc, so the
     high-passed mains and low-passed sub **cancel (notch)** at the crossover instead of summing flat. This bites
     only when `remove_bass_from_sources = true` (mains carry the high-passed signal).
   - **Fix:** 4th-order Linkwitz-Riley — cascade two identical Butterworth biquads for **both** LP and HP
     (−6 dB at fc, in-phase outputs) so the recombined response is flat and in-phase at fc.

3. **No LFE gain compensation** — MEDIUM.
   - Verified: `lfe_sum += bass;` accumulates every source channel with no scaling (`bass_management.rs:206`),
     then `output[lfe_idx] += lfe_sum;` (`:216`).
   - Evidence: with stereo correlated bass (mono content on L and R), the LFE gets 2× amplitude (+6 dB) vs a
     single source; level scales with source count rather than being normalized.
   - **Fix:** normalize the summed LFE by the active source count (or expose an LFE gain) so sub level is
     count-independent.

4. **LFE channel out of range → bass silently dropped, no warning, no fold-back** — LOW.
   - Verified: `process()` returns early when `lfe_ch >= output_channels` (`bass_management.rs:188-190`).
   - Evidence: on a device with fewer channels than `lfe_channel`, the whole bass-management pass is a silent
     no-op (mains stay full-range — no bass lost, but no redirection either). Config validation can't see the
     device channel count, so it's only caught at runtime by this in-range guard.
   - **Fix:** emit a **one-time warning at construction** when `lfe_channel >= output_channels`
     (`main.rs:422` already has `output_channels` in scope at the `BassManagement::new` call). Optional, behind
     the DECISIONS.md (D32) option: fold extracted bass back into the mains when no LFE exists.

5. **Content routed directly to the LFE index is summed-on-top unfiltered** — LOW.
   - Verified: the LFE index is added to, not replaced — `output[lfe_idx] += lfe_sum;` (`bass_management.rs:216`);
     the code does skip a source that *is* the LFE (`src_ch == lfe_ch` → continue, `:197`).
   - Evidence: if another voice routes full-range material to the LFE output index, it is mixed with the
     bass-managed sum rather than crossed over — a collision, arguably intended (additive LFE).
   - **Fix:** **document** that the LFE output index receives (directly-routed content) + (extracted bass);
     optionally low-pass the final LFE bus after summation. Document-only unless DECISIONS.md changes it for the LP.

6. **Denormals in the biquad/IIR processed every sample → CPU spikes** — LOW (from RT-safety).
   - Verified: the biquad `process()` (`bass_management.rs:90-102`) is a transposed direct-form-II IIR with
     persistent `z1`/`z2` state; on a fade/decay tail those state values can become denormal and be processed
     every sample.
   - Evidence: denormal arithmetic is dramatically slower on common CPUs and runs on the **audio thread** —
     periodic CPU spikes / xrun risk after content goes quiet.
   - **Fix:** flush-to-zero in the biquad — add a tiny denormal-prevention offset (DC kill) or FTZ so state can't
     stay denormal.

## Caveats (refuted / over-stated — do not chase ghosts)

- **Findings 1 and 2 were dumped as `high` but reconciled to `medium`.** The verdict reasoning is explicit: both
  matter **only when the user opts into bass management** (default off), the LFE+Main double-bass mode is
  intentional, and the crossover notch only appears in the `remove_bass_from_sources = true` sub-mode and further
  depends on physical speaker placement the software can't control. Treat them as quality/correctness fixes, not
  glitch/crash fixes.
- **Finding 1 is not a unilateral code change.** Do **not** silently flip `remove_bass_from_sources` for users.
  The deliverable is a *decision* (change the default, or document loudly) made per **DECISIONS.md** and a docs
  update — see the changelog section.
- **The denormal item (6) is NOT the NaN finding.** NaN/non-finite sanitisation of the summed mix is Sprint 6's
  job (`mixer.rs:584-587`). Here we only flush *finite-but-denormal* biquad state. Don't add NaN handling in this
  sprint.
- **Do not touch the hard clamp** at `mixer.rs:584-587`. Bass-managed output is summed before it; the limiter is
  Sprint 6.

## Tasks (ordered, TDD)

Work in `src/audio/bass_management.rs` and the render-harness integration test. Each task is **failing test
first**, then the **smallest** code to pass, then refactor green. Do not weaken or `#[ignore]` any existing
test in this file.

1. **Integration test: bass management through `mix_audio` (closes the Sprint-0 gap).**
   First, write a *failing* render-harness test that builds a `MixerState` with `bass_management: Some(...)`
   (`enabled: true`, `source_channels: [0,1]`, `lfe_channel: 3`, 6 output channels) and a low-frequency sample,
   drives it through `render(...)` → `mix_audio` block-by-block, and asserts the LFE channel
   (`band_energy(out, sr, 0, crossover)` on channel 3) is non-trivial while a high-frequency-only scene leaves
   the LFE near silent. The current `MixerState::new` test helper (`mixer.rs:526-536`) hardcodes
   `bass_management: None`, so the harness needs a builder that sets it — add that builder to the Sprint-0
   render-support module rather than changing `MixerState::new`. This test exists from now on as the regression
   anchor for every later bass change.

2. **4th-order Linkwitz-Riley crossover.**
   - Failing test (unit, on `BassManagement`): build LP-only and HP-only renders of a swept/broadband signal
     through the crossover with `remove_bass_from_sources: true`, single source channel, and assert the
     **magnitude of (LP + HP) recombined** is flat-ish (within a small dB tolerance, e.g. ≤ ~1 dB ripple) across
     a band straddling `crossover_frequency_hz` — using `band_energy` in narrow bands at, below, and above fc.
     The current 2nd-order split produces a notch at fc, so this fails first.
   - Then change the filters to LR4: cascade **two identical** Butterworth biquads for the LP path and two for
     the HP path. Concretely, give each source channel a 2-stage cascade (e.g. `lowpass_filters[src]` becomes a
     pair of `BiquadFilter`, applied in series in `process()` at `:205`; same for the high-pass at `:210`).
     Reuse the existing `BiquadCoefficients::lowpass` / `highpass` (`bass_management.rs:16-63`) — LR4 is two
     Butterworth sections in series, so the coefficients are unchanged; only the **number of stages** changes.
   - Keep the existing per-source structure in construction (`:147-167`) and the `passthrough` handling for
     non-source channels intact.
   - Re-confirm `test_lowpass_attenuates_high_frequency` / `test_highpass_attenuates_low_frequency` (now steeper,
     ~24 dB/oct) still pass; the existing attenuation thresholds (`> 20 dB`, `< -3 dB`) should still hold with a
     steeper slope.

3. **LFE gain compensation (count-independent sub level).**
   - Failing test: render the same correlated low-frequency content into **1 source channel** vs **2 source
     channels** and assert the LFE-channel `rms` is within tolerance **independent of source count** (today the
     2-source case is ~+6 dB).
   - Then normalize `lfe_sum` by the number of **active** source channels actually contributing this frame
     (channels that pass the `src_ch >= output_channels || src_ch == lfe_ch` guard at `:197`), computed once per
     `process()` call (not per frame). Prefer count-normalization for the default; only add a configurable LFE
     gain knob if wanted later (note it as a possible follow-up — YAGNI otherwise).
   - Update `test_bass_management_extracts_to_lfe` / `test_bass_management_adds_to_existing_lfe`
     (`bass_management.rs:457-495`, `:640-680`) only if their absolute power assertions move; keep their intent.
     If an assertion has to change, say so loudly in the commit message — do not quietly retune a passing test.

4. **Denormal flush-to-zero in the biquad.**
   - Failing test: drive the biquad with an impulse then silence for many samples and assert the state
     (`z1`/`z2`) reaches **exactly 0.0** (or is flushed below the denormal threshold) within a bounded number of
     samples, instead of decaying into denormal territory. (Reading state needs the `#[cfg(test)] reset()`
     pattern or a small test accessor; prefer asserting on `process()` output settling to exact 0.0.)
   - Then add a flush in `BiquadFilter::process()` (`bass_management.rs:90-102`): either add a tiny DC offset and
     subtract it, or clamp `z1`/`z2` to 0.0 when `abs < 1e-30`. Smallest correct change; keep transposed-DF-II
     structure.

5. **One-time `lfe_channel >= output_channels` warning + docs for findings 1, 4, 5.**
   - Add a `tracing::warn!` **once at construction** in `BassManagement::new` (`bass_management.rs:147-167`) when
     `config.lfe_channel >= output_channels`, stating bass management will be a no-op. (Construction is the right
     place — `process()` runs on the audio thread and must not log per-call; `main.rs:422` passes
     `output_channels` in.) Test: construct with an out-of-range LFE and assert the warning is emitted (capture
     via a tracing test subscriber; test output must be pristine — assert the message, don't let it leak).
   - **Locked decision (DECISIONS.md D30):** change `remove_bass_from_sources` default to `true` when enabled, **or**
     keep `false` and document the additive LFE+Main behavior loudly in `README.md`. Do **not** implement either
     until signed off. Whichever is chosen, document it.
   - Document the LFE-collision behavior (finding 5) in `README.md`: the LFE output index receives directly-routed
     content **plus** extracted bass.

## Files to create / touch

- **Touch:** `src/audio/bass_management.rs` — LR4 cascade (per-source filter pairs at `:147-167`, applied at
  `:205`/`:210`), LFE count-normalization (`:192-216`), denormal flush in `process()` (`:90-102`),
  construction-time warning (`:147-167`). Add/adjust unit tests in its `#[cfg(test)] mod tests`.
- **Touch:** the Sprint-0 render-support module — add a `MixerState` builder that sets
  `bass_management: Some(...)` (do **not** change the `#[cfg(test)] MixerState::new` helper at
  `mixer.rs:526-536`; extend the harness builder instead).
- **Touch:** integration test file under `tests/` (or a `#[cfg(test)]` module) for the `mix_audio` bass-managed
  render test.
- **Touch (docs):** `README.md` (LFE+Main behavior, LFE-collision note, the `remove_bass_from_sources` default
  decision), and `docs/bugs.md` for any out-of-scope discoveries.
- **Do not touch:** `mixer.rs:584-587` (Sprint 6 limiter), `config.rs` schema unless DECISIONS.md changes it a new
  LFE-gain field.

## Verification

**Lane A (Docker / Linux) — primary lane for this sprint:**
- `scripts/validate.sh` is green: `cargo build --release` with `-D warnings`, `cargo clippy --all-targets
  -- -D warnings`, `cargo fmt --check`, full `cargo test`.
- Render-harness assertions hold:
  - **Crossover flatness:** `band_energy(LP_only + HP_only, sr, lo, hi)` summed across narrow bands straddling
    `crossover_frequency_hz` is flat-ish (within tolerance) — no notch at fc.
  - **LFE count-independence:** LFE-channel `rms` for the 1-source and 2-source correlated-bass scenes match
    within tolerance.
  - **Bass extraction through `mix_audio`:** the `Some(bass_management)` render shows real low-band energy on the
    LFE channel and near-silence on the LFE for a high-frequency-only scene.
  - **No new clicks:** `max_inter_sample_delta` on the LFE and main channels stays below the click threshold
    across the crossover region.
- Denormal flush: biquad state settles to exactly 0.0 after silence within the bounded sample budget.
- Construction warning fires for `lfe_channel >= output_channels` and is asserted (pristine test output).

**Lane B (native macOS, real device):**
- No bass-management-specific real-device assertion is required this sprint (Lanes = A). Still run
  `scripts/validate.sh --native` so the host build/lint/test suite (including the new tests) is green and the
  real-device smoke from earlier sprints does not regress.

**Lane C (manual Windows):**
- Nothing to add this sprint. Do not append Windows steps to `MANUAL-VERIFICATION.md`.

## Acceptance criteria (mirror the tracker)

- [ ] 4th-order Linkwitz-Riley crossover; render-harness shows flat-ish acoustic-sum magnitude through the crossover `[A]`
- [ ] LFE gain compensation makes sub level independent of active source count `[A]`
- [ ] `remove_bass_from_sources` default decided + documented; one-time warning when `lfe_channel >= output_channels`; LFE-collision documented `[A]`
- [ ] Denormal flush in the IIR; integration test exercises bass management through `mix_audio` `[A]`

## Behavior-change / changelog notes

- **Behavior-changing — is decided upfront in DECISIONS.md (finding 1):** the `remove_bass_from_sources` default decision.
  If the default is flipped to `true` when bass management is enabled, that **changes the audio** for anyone who
  enabled bass management and relied on the additive LFE+Main behavior. **Implement the locked decision (DECISIONS.md)**
  before changing the default; document the chosen behavior in `README.md` and `CHANGELOG.md` either way.
- **Behavior-changing (audio):** the LR4 crossover changes the acoustic response (steeper slopes, flat in-phase
  sum at fc) and the LFE count-normalization changes sub level (a 2-source setup is ~6 dB quieter in the sub than
  before). Both affect only users who have bass management enabled. Note both in `CHANGELOG.md`.
- **Non-audible:** the denormal flush and the construction-time warning have no effect on correct-level audio;
  the warning is operator-visible only.
- **Not changed:** the hard clamp / limiter (Sprint 6) and NaN handling (Sprint 6).

## Definition of Done

Lane A green · Lane B green (host suite, no regression) · LR4 crossover landed with a flatness render-harness
assertion · LFE count-normalization landed with a count-independence assertion · denormal flush landed with a
settling assertion · the `mix_audio` bass-managed integration test landed (Sprint-0 gap closed) ·
construction-time out-of-range-LFE warning landed and asserted · `remove_bass_from_sources` default **decided
per DECISIONS.md** and documented · LFE+Main and LFE-collision behavior documented in `README.md` · no
existing test weakened or ignored · `docs/bugs.md` updated for any out-of-scope finds · committed atomically to
the branch as units complete · `cargo build --release` warning-free.
