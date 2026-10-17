export const meta = {
  name: 's8-input',
  description: 'Sprint 8 live-input robustness — alloc-free capture, async SRC drift, underrun, controls, non-f32',
  phases: [
    { title: 'InputRT', detail: 'F2 alloc-free resample, F1 async SRC drift, F4 non-f32, F6 guard' },
    { title: 'ControlPlane', detail: 'F3 underrun, F8 mute-restore, F7 voice_volume, F9 ducking trigger, F5 route warn' },
    { title: 'Green', detail: 'full green + behavior-change docs + refine W-2' },
    { title: 'Verify', detail: 'adversarial audit' },
  ],
}

const SPEC = `
Repo /Users/bandrews/src/mqttaudio (branch v2.1), HEAD green (cargo build -D warnings / clippy --all-targets
-D warnings / fmt --check / cargo test / ./scripts/validate.sh all pass). Sprint 8 (live-input robustness) on
the Sprint-5 lock-free engine + Sprint-6 off-RT ducking notify path (compute_changes -> SetDuckTarget) +
Sprint-7.

AUTHORITATIVE SPEC: read docs/sprints/sprint-08-live-input-robustness.md IN FULL (exact file:line, fix, and
the failing test per finding; ordered TDD task list; caveats). Locked decisions (DECISIONS.md): D33 (always
async SRC steered by ring-buffer fill toward ~half-full via set_resample_ratio, EVEN when input==output rate),
D34 (input mute stores + restores the pre-mute volume, not 1.0), D35 (voice_volume affects matching live
inputs even with no sample-backed voice), D36 (mark configured input voices active so they can be ducking
primaries — "always active while the stream is open" is an acceptable v1 — via the Sprint-6 OFF-RT notify
path, never the capture callback), D37 (typed non-f32 input stream -> convert to f32, mirroring Sprint 1's
output dispatch).

Follow CLAUDE.md + the SPRINT-TRACKER charter: NO shortcuts; never weaken/delete/#[ignore] a test; root-cause
only; smallest reasonable change; match style. TDD: failing test first (Sprint 0 render harness + the Sprint 5
allocation-counting harness in tests/alloc_harness.rs), confirm it fails, minimum code, confirm green.

HARD RT-SAFETY INVARIANT (Sprint 5, D22a): the cpal CAPTURE callback (input.rs) must allocate/free/lock
NOTHING on the RT thread, same as mix_audio. Pre-allocate de-interleave accumulators + the resampler output
buffer (rubato output_buffer_allocate) and use process_into_buffer — never Vec::push/drain(..).collect()/
resampler.process() in the callback. Do NOT notify ducking from the capture callback (D36 uses the off-RT
path). tests/alloc_harness.rs must stay green AND gain a test proving the resampling capture block is
allocation-free.
`

phase('InputRT')
const inputrt = await agent(
  `Sprint 8 INPUT-RT group on the main tree (findings F2, F1, F4, F6 — all in src/audio/input.rs), TDD-first.\n` +
  `- F2: extract the resampling-callback body into a testable, alloc-free fn (e.g. resample_block(state: &mut ResampleState, data: &[f32], producer)) with pre-sized de-interleave accumulators + a reusable rubato output buffer (output_buffer_allocate) + process_into_buffer. Add an allocation-harness test (tests/alloc_harness.rs) proving zero alloc across many invocations — confirm it FAILS against the current Vec::push/drain-collect/process() code first.\n` +
  `- F1 (D33): drive the resampler ratio from measured ring-buffer fill (shared atomic fill counter between producer/consumer), slow control loop nudging set_resample_ratio toward ~half-full, gentle gain clamped well inside SincFixedIn's 2.0 max-relative-ratio; route the equal-rate case through async SRC too. Drift-sim test: producer at r_in vs consumer at mismatched r_out (±50ppm and ±1%) over a long simulated run keeps ring fill bounded (never 0 or capacity); a steady sine's band_energy stays within tolerance (no pitch wobble).\n` +
  `- F4 (D37): inspect supported_config.sample_format() and build a typed input stream (I16/U16/I32/F32) converting to f32 before pushing to the ring; keep format choice orthogonal to resampling. Pure-helper dispatcher unit test incl. forced-I16 -> f32.\n` +
  `- F6: debug_assert!(data.len() % channels == 0) at the de-interleave loop top.\n` +
  SPEC +
  `\nKeep build + all tests + tests/alloc_harness.rs green. Report files changed, the new tests + assertions, and confirm the new capture-callback alloc test passes (0 alloc).`,
  { label: 'input-rt', phase: 'InputRT', model: 'opus' }
)

phase('ControlPlane')
const ctrl = await agent(
  `Sprint 8 CONTROL-PLANE group on the main tree (findings F3, F8, F7, F9, F5), building on the InputRT work, TDD-first.\n` +
  `- F3: graceful underrun in mix_live_input_into_output (mixer.rs ~838-885) — on shortfall, fade-to-silence over a few samples (or hold/repeat last frame), and KEEP calling advance_voice_volume() for the silent frames so the ramp stays time-accurate (do not early-break). Test: underrun mid-block during a voice-volume ramp -> max_inter_sample_delta below click threshold AND ramp value after block equals the full-block expected value.\n` +
  `- F8 (D34): add pre_mute_volume (or muted bool) to LiveInput (mixer.rs ~438-459); InputMute saves current volume + zeroes on mute, restores the saved value on unmute (main.rs ~1011 index + ~1024 voice_id). Test: vol 0.7 -> mute -> unmute == 0.7 (not 1.0).\n` +
  `- F7 (D35): VoiceVolume must update matching live_inputs even when VoiceManager has no sample-backed voice (treat a matching input as success), OR register configured input voice_ids in VoiceManager at startup. Test (handle_command harness): an input-only voice's target_voice_volume updates; a sample-backed voice still works.\n` +
  `- F9 (D36): mark each configured input's voice_id active so it can be a ducking primary, via the Sprint-6 OFF-RT notify path (control-side DuckingEngine::compute_changes -> SetDuckTarget), at input setup (always-active-while-open is fine). NEVER from the capture callback. Test: a rule with a mic primary_voice marks that voice active / moves a ducked target.\n` +
  `- F5: after a stream opens with known active_input.channels (main.rs ~444-474), warn once if any route source_channel >= channels. Test: capture the warning (pristine output).\n` +
  SPEC +
  `\nKeep build + all tests + tests/alloc_harness.rs green. Report files changed, new tests + assertions.`,
  { label: 'control-plane', phase: 'ControlPlane', model: 'opus' }
)

phase('Green')
const green = await agent(
  `Sprint 8 GREEN GATE on the main tree. Prior agents implemented the input-RT + control-plane fixes (reports below).\n` +
  `Bring the WHOLE project to the gate by correct means and finalize:\n` +
  `1. Reach: cargo fmt --check; RUSTFLAGS="-D warnings" cargo build --release; cargo clippy --all-targets -- -D warnings; cargo test (ALL pass incl. tests/alloc_harness.rs and every new Sprint-8 test). Fix root causes; if an earlier group left something wrong, fix it.\n` +
  `2. Update CHANGELOG.md + README.md + docs/features/microphone-input.md for the behavior changes (D34 mute restore, D35 voice_volume on inputs, D36 mic-as-ducking-trigger, D33 equal-rate async SRC path change, F5 route warning).\n` +
  `3. Refine the existing W-2 entry in docs/sprints/MANUAL-VERIFICATION.md (>=30-min two-device USB-mic + separate-output soak: no periodic dropouts, bounded ring fill, plus a non-f32 input device opens + routes).\n` +
  `4. Log any out-of-scope discoveries in docs/bugs.md.\n` +
  `Report exact gate results (paste the test result summary lines), confirm alloc_harness passes (incl. the new capture-callback test), list files changed, flag anything compromised. A false green is the worst outcome.\n\n` +
  SPEC +
  `\n\n--- INPUT-RT REPORT ---\n${inputrt}\n\n--- CONTROL-PLANE REPORT ---\n${ctrl}`,
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
  ['rt-safety', 'Audit the cpal CAPTURE callback path in src/audio/input.rs: it must allocate/free/lock NOTHING on the RT thread (pre-allocated de-interleave accumulators + reusable rubato output buffer + process_into_buffer; no Vec::push/drain-collect/process(); the async-SRC steering reads a shared atomic, not a lock; no ducking notify from the callback). Confirm the new alloc-harness capture test genuinely arms around resample_block and asserts 0 alloc, and tests/alloc_harness.rs all pass. Use git diff. Report any RT-thread alloc/free/lock.'],
  ['correctness', 'Verify each finding vs spec + DECISIONS: F1 async SRC actually steers set_resample_ratio from ring fill and the drift sim keeps the buffer bounded (never 0/capacity); F2 alloc-free; F3 underrun fades/holds AND advances the ramp for silent frames (time-accurate); F4 typed I16/U16/I32/F32 -> f32 dispatch; F5 route warning; F6 debug_assert; F7 voice_volume reaches input-only voices; F8 mute restores the prior calibrated volume (0.7 not 1.0); F9 input marked active via the OFF-RT path (not the callback). Report any finding not actually fixed or a test asserting the wrong thing.'],
  ['parity-tests', 'Confirm no test weakened/deleted/ignored; the gate is genuinely green (read the green report critically + spot-check); CHANGELOG/README/microphone-input.md document the D34/D35/D36/D33/F5 changes; W-2 refined in MANUAL-VERIFICATION.md. Report gaps.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(`Adversarial read-only review of the uncommitted Sprint 8 changes on the main tree at /Users/bandrews/src/mqttaudio (do not edit; git diff + read tests). Lens: ${lens}.\n\n${prompt}\n\n${SPEC}`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT })
))

return { inputrt, ctrl, green, findings: findings.filter(Boolean) }
