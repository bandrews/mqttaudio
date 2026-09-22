export const meta = {
  name: 's6-dsp',
  description: 'Sprint 6 mixer DSP correctness — output stage, interpolation/loop, pitch, ducking',
  phases: [
    { title: 'OutputStage', detail: 'F1 calibration, F2 limiter, F3 NaN guard, F4 clamp' },
    { title: 'Interp', detail: 'F9 reverse neighbor, F5 equal-power, F6 overlap-on-wrap' },
    { title: 'Pitch', detail: 'F10 preroll, F11 advance, F12 flush' },
    { title: 'Ducking', detail: 'D1-D5' },
    { title: 'Green', detail: 'full green + behavior-change docs' },
    { title: 'Verify', detail: 'adversarial audit' },
  ],
}

const SHARED = `
Repo /Users/bandrews/src/mqttaudio (branch v2.1), HEAD is green: \`cargo build --release\` (-D warnings),
\`cargo clippy --all-targets -- -D warnings\`, \`cargo fmt --check\`, \`cargo test\`, and \`./scripts/validate.sh\`
all pass NOW. This is Sprint 6 (mixer DSP correctness) on the Sprint-5 lock-free engine.

AUTHORITATIVE SPEC: read docs/sprints/sprint-06-mixer-dsp-correctness.md IN FULL — every finding has an exact
file:line, the fix, and the render-harness assertion to write. Also read the relevant DECISIONS.md entries
(D23 F4 Play-volume clamp; D25 D3 restore honors fade_duration_ms; D26 F2 soft-knee/tanh limiter, ceiling
default -1.0 dBFS + master gain + clip AtomicU64 on /status; D27 cubic/Hermite interpolation for the non-pitch
speed path + keep the ±100 range, document fast-path aliasing; D28 equal-power loop crossfade + overlap-on-wrap;
D29 optional per-route downmix gain default 1.0). Follow CLAUDE.md + the SPRINT-TRACKER charter: NO shortcuts,
never weaken/delete/#[ignore] a test (the sprint explicitly RE-PURPOSES test_saturation to assert limiter
behavior — that is a justified change, not a deletion), root-cause only, smallest reasonable change, match
surrounding style.

TDD: for each finding write the FAILING render-harness assertion first (Sprint 0 harness: render(), rms,
peak, band_energy, max_inter_sample_delta; add the ramp / transient-loop / NaN-poisoned / duck-scene fixtures
the sprint's task 1 describes if not already present), confirm it fails, then make the minimum change, confirm
green.

HARD RT-SAFETY INVARIANT (Sprint 5, D22a): do NOT re-introduce any allocation, free, or lock in the audio
callback path. mix_audio / mix_sample_into_output / mix_sample_with_pitch_correction / the ducking
get_multiplier path all run in the callback. Any new per-buffer state (ducking scratch, pitch accumulator,
per-channel gain vec, preroll samples) must be PRE-ALLOCATED in the audio-thread-owned structures, never
allocated in the callback. tests/alloc_harness.rs MUST STAY GREEN (all 5 tests) — re-run it after your
changes; if a fix would allocate/free on RT, pre-allocate instead. The DSP body math may change but the
no-alloc/no-free/no-lock contract is inviolable.
`

phase('OutputStage')
const out = await agent(
  `Sprint 6 OUTPUT-STAGE group (findings F1, F2, F3, F4) on the main tree. Implement per the spec, TDD-first.\n` +
  `- F3 NaN/non-finite guard in the final output loop (is_finite else 0.0).\n` +
  `- F2 soft-knee/tanh limiter with configurable output_ceiling (default -1.0 dBFS) + optional master_gain on the f32 bus, replacing the brickwall clamp; a clip/over AtomicU64 counter surfaced in handle_status; re-purpose test_saturation to assert limiter (not ==1.0). (config.rs + validation; http/handlers.rs single field.)\n` +
  `- F1 per-channel calibration: resolve channel_volumes aliases->indices ONCE at MixerState construction (off RT) into a pre-allocated Vec<f32> length output_channels (default 1.0) stored on MixerState; apply in the final loop. Final-loop order: per-channel gain -> limiter/soft-clip -> finite-guard -> ceiling clamp.\n` +
  `- F4 clamp Play volume to [0,1] inside the ActiveSample::new* constructors (D23).\n` +
  SHARED +
  `\nKeep cargo build + the existing tests + tests/alloc_harness.rs green (pre-allocate the per-channel gain vec — no callback alloc). Report files changed, the new tests + their assertions, and confirm alloc_harness still passes.`,
  { label: 'output-stage', phase: 'OutputStage', model: 'opus' }
)

phase('Interp')
const interp = await agent(
  `Sprint 6 INTERPOLATION/LOOP group (F9, F5, F6) on the main tree, building on the prior output-stage work.\n` +
  `- F9 (HIGH): reverse playback must interpolate frame_n..frame_n+1 (direction-independent), frac = src_pos.fract(); remove the is_reverse neighbor special-case in mix_sample_into_output. -0.5x ramp test.\n` +
  `- F5: equal-power loop crossfade (cos/sin), ~±0.5 dB through the crossfade (D28). Evaluate equal-power fade in/out; if kept linear, document why.\n` +
  `- F6: overlap-on-wrap — the next loop pass starts PAST the overlapped head (begin at cf_samples) so [0..cf_samples] is not replayed; transient-loop test asserts the head appears once per loop and no seam click.\n` +
  SHARED +
  `\nKeep build + all tests + tests/alloc_harness.rs green. Report files changed, the new tests, and confirm alloc_harness still passes and forward-playback tests are unchanged.`,
  { label: 'interp', phase: 'Interp', model: 'opus' }
)

phase('Pitch')
const pitch = await agent(
  `Sprint 6 PITCH group (F10, F11, F12) on the main tree, building on prior work.\n` +
  `- F10 (HIGH): pre-roll the stretcher on the None->Some enable transition so there is no silent gap/click. Add PitchCorrector::preroll(&pre_samples) over the crate's pre-roll primitive (the wrapper currently exposes only reset() — ADD preroll; do NOT assume a seek exists) and call it in enable_pitch_correction using the samples before sample.position. Use Sprint 5's pre-allocated pitch scratch — NO vec! in the callback.\n` +
  `- F11: advance sample.position by the INTEGER input frames actually fed to the stretcher, carrying the fractional remainder in a new accumulator field on the (audio-thread-owned) ActiveSample, so slices abut exactly. Test at a NON-integer product (buffer 512 x speed 0.7) for phase continuity.\n` +
  `- F12: at EOF flush/drain the stretcher to emit the buffered tail instead of overshooting and returning silence.\n` +
  SHARED +
  `\nKeep build + all tests + tests/alloc_harness.rs green (preroll/accumulator must not allocate in the callback). Report files changed, new tests, and confirm alloc_harness passes.`,
  { label: 'pitch', phase: 'Pitch', model: 'opus' }
)

phase('Ducking')
const duck = await agent(
  `Sprint 6 DUCKING group (D1, D2, D3, D4, D5) on the main tree, building on prior work. NOTE the engine was\n` +
  `already split in Sprint 5 into control-side DuckingEngine (compute_changes) + audio-side DuckingApplier\n` +
  `(get_multiplier); apply these within that split.\n` +
  `- D1 (HIGH): advance each distinct voice's duck fade EXACTLY ONCE per buffer (not once per sample/input). In the applier, separate advance from read: advance_buffer(distinct_voices, frames) once, then a non-advancing current_multiplier(voice) applied to every sample/input of that voice. Pre-allocated scratch for the distinct-voice set — NO callback alloc. Test: two overlapping samples on one ducked voice -> fade advances once, both see the same multiplier.\n` +
  `- D2: interpolate the duck multiplier PER FRAME (lerp buffer-start->end) in the mix loops, like voice_volume/fade_state. Test: smooth, no per-buffer stairstep.\n` +
  `- D3 (D25, behavior change): restore uses the rule's fade_duration_ms (stored on the duck state in begin_duck, max across rules), not a hardcoded 2000 ms. Test: 200ms rule restores in ~200ms.\n` +
  `- D4: wire configured input voices through the off-RT notify/compute_changes path so a mic primary_voice ducks. Detection is Sprint 8 — drive the notify directly in the test.\n` +
  `- D5: gate begin_duck on params_changed so an unrelated voice toggle does not reset a settled voice's fade; consolidate to a single source of activity truth. Test: unrelated toggle doesn't reset a settled voice.\n` +
  SHARED +
  `\nKeep build + all tests + tests/alloc_harness.rs green (the once-per-buffer advance must pre-allocate any scratch). Report files changed, new tests, and confirm alloc_harness passes.`,
  { label: 'ducking', phase: 'Ducking', model: 'opus' }
)

phase('Green')
const green = await agent(
  `Sprint 6 GREEN GATE on the main tree. Prior agents implemented F1-F12 + D1-D5 (their reports below). Bring the\n` +
  `WHOLE project to the gate by correct means and finalize:\n` +
  `1. Reach: cargo fmt --check; RUSTFLAGS="-D warnings" cargo build --release; cargo clippy --all-targets -- -D warnings; cargo test (ALL pass, including tests/alloc_harness.rs's 5 tests and every new Sprint-6 render-harness assertion). Fix root causes; if an earlier group left something wrong, fix it.\n` +
  `2. F7/F8/R1 LOW/optional per DECISIONS: D27 (cubic/Hermite non-pitch interpolation) should be implemented; D29 (optional per-route downmix gain, default 1.0, don't change existing 1:1) implement or document; F8 document the fast-path aliasing + any speed cap. Log anything deferred in docs/bugs.md.\n` +
  `3. Update CHANGELOG.md + README.md for the behavior changes (F4 Play-volume clamp, D3 restore timing, F2 limiter + output_ceiling/master_gain + clip counter, F5/F6 crossfade, any F8 speed cap).\n` +
  `Report the exact gate results (paste the test result summary lines), confirm alloc_harness's 5 tests pass, list files changed, and flag anything incomplete or compromised. A false green is the worst outcome.\n\n` +
  SHARED +
  `\n\n--- OUTPUT-STAGE REPORT ---\n${out}\n\n--- INTERP REPORT ---\n${interp}\n\n--- PITCH REPORT ---\n${pitch}\n\n--- DUCKING REPORT ---\n${duck}`,
  { label: 'green', phase: 'Green', model: 'opus' }
)

phase('Verify')
const VERDICT = {
  type: 'object', additionalProperties: false,
  required: ['lens', 'summary', 'issues'],
  properties: {
    lens: { type: 'string' }, summary: { type: 'string' },
    issues: { type: 'array', items: {
      type: 'object', additionalProperties: false,
      required: ['severity', 'file', 'description'],
      properties: { severity: { type: 'string', enum: ['critical','high','medium','low'] }, file: { type: 'string' }, description: { type: 'string' } },
    } },
  },
}
const lenses = [
  ['rt-safety', 'Re-audit the audio callback path for Sprint 5 regressions: no new allocation/free/lock in mix_audio / mix_sample_into_output / mix_sample_with_pitch_correction / the ducking apply path / the final output loop. New state (per-channel gain vec, ducking distinct-voice scratch, pitch accumulator + preroll) must be pre-allocated, not per-callback. Confirm tests/alloc_harness.rs all 5 pass and genuinely cover these. Use git diff. Report any RT-thread alloc/free/lock introduced.'],
  ['correctness', 'For each Sprint-6 finding (F1-F12, D1-D5) check the implementation matches the sprint spec and the new render-harness test genuinely asserts the claimed property (F9 reverse = frame_n*(1-frac)+frame_{n+1}*frac; F3 NaN->0; F2 peak<=ceiling + soft shape + clip counter; F1 per-channel gain; F4 clamp; F5 ~±0.5dB; F6 head once + no seam click; D1 advance-once + equal multiplier; D2 per-frame smooth; D3 ~200ms restore; D4 input notify ducks; D5 no reset on unrelated toggle; F10 no gap; F11 phase continuity at non-integer product; F12 tail emitted). Use git diff + run the tests mentally/by reading. Report any finding not actually fixed or any test that asserts the wrong thing / is mocked.'],
  ['parity-tests', 'Confirm no test was weakened/deleted/ignored to pass (test_saturation re-purpose is sanctioned; verify it now asserts limiter behavior, not removed). Verify the gate is actually green by reading the green agent report critically + spot-checking. Confirm CHANGELOG/README document the behavior changes (F4, D3, F2, F5/F6). Report gaps.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(`Adversarial read-only review of the uncommitted Sprint 6 DSP changes on the main tree at /Users/bandrews/src/mqttaudio (do not edit; use git diff + read tests). Lens: ${lens}.\n\n${prompt}\n\n${SHARED}`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT })
))

return { out, interp, pitch, duck, green, findings: findings.filter(Boolean) }
