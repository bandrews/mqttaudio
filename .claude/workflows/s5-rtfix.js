export const meta = {
  name: 's5-rtfix',
  description: 'Fix the RT-thread frees the Sprint 5 verification found (command-return ring, un-box, small fixes)',
  phases: [
    { title: 'Fix', detail: 'command-return ring + un-box + fade-drift + reap-fallback + reap test; reach green' },
    { title: 'Verify', detail: 'adversarial re-audit of RT-safety + the specific fixes' },
  ],
}

const SPEC = `
Repo: /Users/bandrews/src/mqttaudio (branch v2.1). HEAD commit 2c0eea4 is the WIP Sprint 5 lock-free RT
engine integration (option D, uncontended mutex — see docs/sprints/DECISIONS.md D22a). Build/clippy/fmt
pass; \`cargo test\` passes EXCEPT a deliberately-added failing gate (see below). Follow CLAUDE.md +
docs/sprints/SPRINT-TRACKER.md charter: NO shortcuts, never weaken/delete/#[ignore] a test to pass,
root-cause only, smallest reasonable change, match surrounding style, two ABOUTME lines on new files.

THE GATE (already written, currently FAILING — make it pass by correct means, do NOT change the test's
intent): tests/alloc_harness.rs::draining_mutation_commands_is_free_free queues four heap-owning mutation
commands (FadeOutMatching, SeekMatching, SetVoiceVolume, SetVolumeMatching) and runs ONE armed callback
step (lock -> drain_commands -> mix_audio -> reap_finished), asserting ZERO allocations and ZERO frees.
It fails today with "got 4" frees because drain_commands drops each consumed AudioCommand (its String /
Vec / SampleSelector heap) on the RT thread. Your fixes must make it pass (0 alloc, 0 free) without
weakening it; you MAY tighten it.

ROOT-CAUSE FIXES (src/rt_engine.rs is a top-level lib module; AudioCommand/apply_command/drain_commands/
command_channel/graveyard/reap_finished/AudioCallbackState live there):

1. COMMAND-RETURN RING (the main fix). Add a return ring so spent mutation commands are dropped OFF the
   RT thread, mirroring the sample graveyard:
   - Add types CommandReturnProducer = HeapProducer<AudioCommand>, CommandReturnConsumer =
     HeapConsumer<AudioCommand>, and pub fn command_return_channel(cap) -> (..,..).
   - Add field \`pub command_returns: CommandReturnProducer\` to AudioCallbackState.
   - Refactor application so the RT path never DROPS a heap-owning command:
       * Extract a \`fn apply_mutation(state: &mut MixerState, cmd: &AudioCommand, sr: u32)\` that handles the
         by-reference (read-only) commands: FadeOutAll, FadeOutSamples, FadeOutMatching, SetVoiceVolume,
         SetInputVolume, SeekMatching, SetSpeedMatching, SetVolumeMatching, SetDuckTarget. (These already
         only READ their selector/ids/voice/change and mutate the MIXER, so &cmd works. SetDuckTarget ->
         applier.apply_target(change).) For AddSample/AddLiveInput it is a no-op (handled by move in drain).
       * Change drain_commands signature to take \`returns: &mut CommandReturnProducer\` and, per popped cmd:
           - AudioCommand::AddSample(sample)    => state.active_samples.push(sample)   // see fix 2: un-boxed move, NO free
           - AudioCommand::AddLiveInput(input)  => state.live_inputs.push(input)        // un-boxed move, NO free
           - other                              => { apply_mutation(state, &cmd, sr); let _ = returns.push(cmd); }
         So a heap-owning husk is MOVED into the return ring (dropped later, off-RT by the control reaper),
         never dropped in the callback. drain stays bounded by max.
       * Keep the existing by-value \`pub fn apply_command(state, cmd, sr)\` working for the unit tests:
         implement it as { match move-commands and push to state; else apply_mutation(state,&cmd,sr); } — it
         may drop the husk itself (tests are not RT). Its existing callers/tests must keep passing.
   - The control-side reaper that already drains the graveyard (src/main.rs reap_finished_samples / the 20ms
     tick) must ALSO drain the command-return ring and drop the husks there (off-RT). Wire the consumer into
     the control loop next to grave_rx. main.rs creates the ring at setup and bundles the producer into
     AudioCallbackState; engine.rs run_mix_callback destructures the new field and passes it to drain_commands.

2. UN-BOX AddSample / AddLiveInput so moving them into the mixer frees nothing. Change the enum variants to
   AudioCommand::AddSample(ActiveSample) and AudioCommand::AddLiveInput(LiveInput) (inline, not Box). Update
   apply_command/drain (push the value directly), the rt_engine unit tests, and the construction sites in
   src/main.rs (Play -> AudioCommand::AddSample(sample); the live-input setup -> AddLiveInput(input)) — drop
   the Box::new wrappers. (The AudioCommand enum gets larger; the ring is pre-allocated so this is a one-time
   memory cost, not an RT allocation.)

3. REAP GRAVEYARD-FULL: in reap_finished, if graveyard.push(sample) returns Err (ring full), DO NOT drop the
   sample on the RT thread — leave it in active_samples (re-insert / skip removing it) to be reaped next
   block. Never free a sample in the callback.

4. DUCKING FADE-DRIFT (src/audio/ducking.rs compute_changes): currently emits a SetDuckTarget only when the
   resolved TARGET moves, ignoring fade_frames changes, so a same-target/faster-fade rule activating mid-fade
   no longer accelerates the fade (the old update_duck_states always re-armed begin_duck with the min fade).
   Track the last (target, fade_frames) per voice and emit a change when EITHER differs. Keep
   applier_matches_legacy_engine passing and add/extend a test for the same-target-faster-fade case.

5. TEST the control-side reaper (src/main.rs reap_finished_samples): it currently has ZERO coverage. Add a
   unit test (the Fixture already owns cmd ring, ducking_engine, active_counts, playing, snapshot): drive a
   ducking primary active (so a background voice ducks), then simulate that primary's last sample finishing
   (push it onto the graveyard / decrement), run the reaper, and assert (a) active_counts for the voice hits
   0 and the voice is removed, (b) a restore SetDuckTarget (target 1.0) is emitted on the command ring, and
   (c) the snapshot/ playing map is updated. Cover the guard that a voice with count>1 does NOT restore early.

KNOWN RESIDUAL TO DOCUMENT (do NOT try to fully fix here; log in docs/bugs.md as Sprint 5/9): a Speed command
that TOGGLES pitch correction on a live voice still creates/drops the signalsmith Stretch on the RT thread
(SetSpeedMatching{pitch_correction:true/false} -> enable/disable_pitch_correction). Eliminating it needs the
control thread to pre-build the PitchCorrector and send it in the command + return the old one via a graveyard;
that is a separate change. Note it; the alloc gate deliberately does not toggle pitch.

GREEN GATE (reach ALL, by correct means): \`cargo fmt --check\`; \`RUSTFLAGS="-D warnings" cargo build --release\`;
\`cargo clippy --all-targets -- -D warnings\`; \`cargo test\` ALL pass INCLUDING
draining_mutation_commands_is_free_free and the other alloc_harness tests and your new reaper test.
Report the exact gate results (paste the test result summary lines), files changed, and anything you were
forced to compromise. A false "green" is the worst possible outcome — be brutally honest.
`

phase('Fix')
const fix = await agent(
  `Implement the Sprint 5 RT-safety fixes on the main tree at /Users/bandrews/src/mqttaudio. Read every file before editing. ` +
  `This is RT-critical audio code; correctness over speed.\n\n` + SPEC,
  { label: 'rtfix', phase: 'Fix', model: 'opus' }
)

phase('Verify')
const VERDICT = {
  type: 'object', additionalProperties: false,
  required: ['lens', 'summary', 'issues'],
  properties: {
    lens: { type: 'string' },
    summary: { type: 'string' },
    issues: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['severity', 'file', 'description'],
        properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          file: { type: 'string' }, description: { type: 'string' },
        },
      },
    },
  },
}
const lenses = [
  ['rt-frees', 'Re-audit the RT callback path (src/audio/engine.rs run_mix_callback, src/rt_engine.rs drain_commands/apply_mutation/reap_finished). Confirm NO command husk is dropped on the RT thread (every consumed mutation command is moved to the command-return ring; AddSample/AddLiveInput are un-boxed and moved into the mixer, not freed; reap graveyard-full leaves the sample in place, never drops). Confirm the control loop actually drains the command-return ring off-RT. Use git diff. Report any remaining RT-thread alloc or free EXCEPT the documented pitch-toggle Stretch residual.'],
  ['parity-coverage', 'Confirm the fixes did not change audible behavior or weaken tests: the ducking fade-drift fix matches the legacy engine for the same-target-faster-fade case; the new reaper test genuinely exercises un-duck-on-finish + the count>1 guard; no test was weakened/removed/ignored; the alloc gate draining_mutation_commands_is_free_free is unweakened and passes. Verify cargo test is actually green by reading the agent report critically. Report any gap or weakening.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(`Adversarial read-only review of the uncommitted Sprint 5 RT-fix on the main tree at /Users/bandrews/src/mqttaudio (do not edit; use git diff). Lens: ${lens}.\n\n${prompt}\n\nContext:\n${SPEC}`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT })
))

return { fix, findings: findings.filter(Boolean) }
