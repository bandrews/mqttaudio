export const meta = {
  name: 's7-bass',
  description: 'Sprint 7 bass management — LR4 crossover, LFE gain comp, denormal flush, mix_audio integration test',
  phases: [
    { title: 'Implement', detail: 'LR4 + LFE comp + denormal + warning + default flip + integration test + docs' },
    { title: 'Verify', detail: 'adversarial audit (DSP correctness + RT-safety/parity)' },
  ],
}

const SPEC = `
Repo /Users/bandrews/src/mqttaudio (branch v2.1), HEAD green (cargo build -D warnings / clippy --all-targets
-D warnings / fmt --check / cargo test / ./scripts/validate.sh all pass). This is Sprint 7 (bass management),
on top of the Sprint-5 lock-free engine + Sprint-6 DSP work.

AUTHORITATIVE SPEC: read docs/sprints/sprint-07-bass-management-and-multichannel.md IN FULL — it has the exact
file:line, fix, and render-harness assertion for each of the 6 findings, an ordered TDD task list, and the
scope boundaries. Read the locked decisions in DECISIONS.md: D30 (remove_bass_from_sources default = TRUE when
bass management is enabled; additive "LFE+Main" mode remains available via false — BEHAVIOR CHANGE, changelog +
README), D31 (4th-order Linkwitz-Riley = cascade two identical Butterworth biquads for LP and HP), D32
(normalize the summed LFE by the ACTIVE source count so sub level is count-independent; expose an lfe_gain trim
default 1.0; one-time warning when lfe_channel >= output_channels; NO final-LFE low-pass — YAGNI, note it).

Follow CLAUDE.md + the SPRINT-TRACKER charter: NO shortcuts, never weaken/delete/#[ignore] an existing test
(the sprint says: if an absolute-power assertion in test_bass_management_* must move because of LFE
normalization, change it loudly with a comment + commit note, preserving intent — do NOT quietly retune). TDD:
failing render-harness/unit test first, minimum code, confirm green.

WORK ITEMS:
1. Integration test FIRST (closes the Sprint-0 gap): a render-harness test building a MixerState with
   bass_management: Some(...) (enabled, source_channels [0,1], lfe_channel 3, >=6 output channels) via the
   existing SceneBuilder::bass_management(bm) builder (src/audio/test_support.rs:133) + a low-frequency sample;
   render block-by-block through mix_audio; assert the LFE channel carries real low-band energy
   (band_energy at/below crossover) while a high-frequency-only scene leaves the LFE near-silent. This is the
   regression anchor.
2. D31 LR4 crossover: give each source channel a 2-stage cascade (pair of BiquadFilter) for LP and for HP,
   applied in series in process(); reuse the existing lowpass/highpass Butterworth coefficients (LR4 = two
   Butterworth sections in series — coeffs unchanged, only #stages). Failing test: (LP_only + HP_only)
   recombined magnitude is flat (<= ~1 dB ripple) across bands straddling fc (the current single 2nd-order
   split notches at fc). Keep test_lowpass_attenuates_high_frequency / test_highpass_attenuates_low_frequency
   passing (steeper slope, thresholds still hold).
3. D32 LFE gain compensation: normalize lfe_sum by the active source count (channels contributing this
   process() call, computed once per call, not per frame); add lfe_gain config field (default 1.0) applied to
   the LFE. Failing test: LFE rms for a 1-source vs 2-source correlated-bass scene match within tolerance.
4. Denormal flush in BiquadFilter::process(): flush z1/z2 to 0.0 when |state| < ~1e-30 (or DC-kill). Failing
   test: after an impulse then silence, the biquad output/state settles to exactly 0.0 within a bounded sample
   count. Keep transposed-DF-II structure. This runs per-sample on the RT thread — pure arithmetic, NO alloc.
5. D30 default flip: BassManagementConfig.remove_bass_from_sources defaults to TRUE (config.rs + bass_management.rs).
6. D32 warning: one-time tracing::warn! at BassManagement::new when config.lfe_channel >= output_channels
   (output_channels is in scope at the main.rs construction call). Test: construct out-of-range, assert the
   warning is emitted via a tracing test subscriber (test output PRISTINE — capture + assert, don't leak).
7. Docs: README.md + CHANGELOG.md for the D30 default change, the LR4 acoustic change, the LFE
   count-normalization (a 2-source setup is ~6 dB quieter in the sub than before), and the LFE-collision note
   (the LFE output index receives directly-routed content PLUS extracted bass). docs/bugs.md for any
   out-of-scope finds (e.g. the no-final-LFE-LP deferral).

OUT OF SCOPE: do NOT touch the Sprint-6 limiter/clamp or NaN handling in mixer.rs; do not add runtime
reconfiguration. RT-safety (D22a) holds: nothing new allocates/frees/locks in the callback; tests/alloc_harness.rs
must stay green.

GREEN GATE: cargo fmt --check; RUSTFLAGS="-D warnings" cargo build --release; cargo clippy --all-targets -- -D
warnings; cargo test (ALL pass, incl. the new bass tests + alloc_harness). Report exact gate results, files
changed, new tests + assertions, and anything compromised. A false green is the worst outcome.
`

phase('Implement')
const impl = await agent(
  `Implement Sprint 7 (bass management) on the main tree at /Users/bandrews/src/mqttaudio, TDD-first, per the spec. Read every file before editing.\n\n` + SPEC,
  { label: 'bass', phase: 'Implement', model: 'opus' }
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
  ['dsp-correctness', 'Verify the DSP per the spec + DECISIONS: LR4 is genuinely two cascaded Butterworth biquads per LP/HP path (not a single 2nd-order); the flatness test really straddles fc and asserts <=~1 dB ripple; LFE normalization is by ACTIVE source count (computed once per process call) and the count-independence test holds; the denormal flush settles state to exactly 0.0; the warning fires on lfe_channel>=output_channels with pristine test output; remove_bass_from_sources defaults to true. Use git diff + read the tests. Report any DSP error or test that asserts the wrong thing.'],
  ['rt-safety-parity', 'Confirm RT-safety + no weakened tests: the per-sample biquad denormal flush adds no allocation/lock on the audio thread; tests/alloc_harness.rs stays green; no existing bass_management test was weakened/deleted/ignored (any moved absolute-power assertion is justified + commented + preserves intent); the mix_audio bass integration test genuinely exercises the Some(bass_management) path through render(); CHANGELOG/README document the D30 default change + LR4 + LFE normalization + LFE-collision. Independently confirm the gate is green by reading the report critically. Report gaps.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(`Adversarial read-only review of the uncommitted Sprint 7 bass-management changes on the main tree at /Users/bandrews/src/mqttaudio (do not edit; use git diff + read tests). Lens: ${lens}.\n\n${prompt}\n\n${SPEC}`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT })
))

return { impl, findings: findings.filter(Boolean) }
