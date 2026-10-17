export const meta = {
  name: 's5-integrate',
  description: 'Wire the lock-free RT engine (Sprint 5, option-D uncontended mutex) into main.rs/engine.rs',
  phases: [
    { title: 'Core', detail: 'production code: ducking split, AudioCallbackState bundle, callback, handlers' },
    { title: 'Green', detail: 'rewrite Fixture tests, extend alloc harness, reach full green' },
    { title: 'Verify', detail: 'parallel adversarial audit' },
  ],
}

const DESIGN = `
SPRINT 5 — LOCK-FREE RT ENGINE, OPTION D (uncontended mutex). Decision recorded in
docs/sprints/DECISIONS.md as D22a. Read it. The repo HEAD is GREEN; \`cargo build --release\`,
\`cargo clippy --all-targets -- -D warnings\`, \`cargo test\`, and \`./scripts/validate.sh\` all pass now.
DO NOT regress them. Follow CLAUDE.md and docs/sprints/SPRINT-TRACKER.md (the charter): no shortcuts,
no disabling/weakening/#[ignore]-ing tests, root-cause only, smallest reasonable change, match surrounding
style, files start with the two ABOUTME: lines.

GOAL: the cpal output callback must do NO allocation and NO free and must never be locked by the control
plane. We keep MixerState behind a mutex (it must survive stream rebuilds) but bundle it with the
command-ring consumer + graveyard producer, and the CONTROL THREAD NEVER LOCKS IT. All control->audio
mutations go through the existing SPSC command ring; all HTTP status comes from a control-side snapshot.
mix_audio's DSP is UNCHANGED (wrapped, not rewritten).

EXISTING SUBSTRATE (already committed, in src/rt_engine.rs — a top-level lib module; reuse it, do not
rewrite it):
  - enum AudioCommand { AddSample{sample:Box<ActiveSample>, notify_active:bool}, AddLiveInput(Box<LiveInput>),
    FadeOutAll{fade_ms}, FadeOutSamples{ids,fade_ms}, FadeOutMatching{selector,fade_ms},
    SetVoiceVolume{voice,volume}, SetInputVolume{input,volume}, SeekMatching{selector,position_ms},
    SetSpeedMatching{selector,speed,pitch_correction}, SetVolumeMatching{selector,volume} }
  - apply_command(&mut MixerState, AudioCommand, output_sample_rate)
  - command_channel(cap) -> (CommandProducer, CommandConsumer)   [ringbuf SPSC; producer.push(cmd)->Result<(),cmd>, consumer.pop()->Option<cmd>]
  - drain_commands(&mut CommandConsumer, &mut MixerState, sr, max) -> usize
  - graveyard_channel(cap) -> (GraveyardProducer, GraveyardConsumer)  [HeapRb<ActiveSample>]
  - reap_finished(&mut MixerState, &mut GraveyardProducer) -> usize   [moves is_finished() samples out for off-RT drop]

REQUIRED CHANGES TO THE SUBSTRATE (Core stage):
  (a) AudioCommand::AddSample becomes a tuple: AddSample(Box<ActiveSample>)  (drop notify_active; ducking is
      handled separately). In apply_command, AddSample just pushes the sample (delete the ducking block).
  (b) Add AudioCommand::SetDuckTarget(crate::audio::ducking::DuckTargetChange); in apply_command apply it to
      state.ducking_applier (if Some) via applier.apply_target(&change). Import DuckTargetChange.
  (c) Add: pub struct AudioCallbackState { pub mixer: MixerState, pub commands: CommandConsumer,
      pub graveyard: GraveyardProducer }  — the bundle the callback owns behind one Arc<Mutex<>>.
  (d) Update the two rt_engine tests that construct AddSample{...} to AddSample(Box::new(..)). Add a small
      unit test that SetDuckTarget applied to a state with a Some(DuckingApplier) ducks the voice.

DUCKING SPLIT (src/audio/ducking.rs) — ADD this validated code (the existing DuckingEngine/notify_voice_active/
update_duck_states/get_multiplier/tests stay UNCHANGED; this is additive plus one new field):
  - Add field to DuckingEngine: \`last_targets: HashMap<String, f32>\` (init HashMap::new() in new()).
  - Add struct DuckTargetChange { pub voice: String, pub target_volume: f32, pub fade_frames: usize }
    deriving Debug, Clone, PartialEq.
  - Add struct DuckingApplier { duck_states: HashMap<String, DuckState> } deriving Default, with:
      new() -> Self;
      apply_target(&mut self, change: &DuckTargetChange): entry(change.voice).or_insert_with(DuckState::new);
        if change.target_volume >= 1.0 { state.begin_restore(change.fade_frames) } else { state.begin_duck(change.target_volume, change.fade_frames) }
      get_multiplier(&mut self, voice_id, frames) -> f32: same body as DuckingEngine::get_multiplier
      #[cfg(test)] peek_multiplier(&self, voice_id) -> f32: same as engine's.
  - Add method to DuckingEngine:
      compute_changes(&mut self, voice_id: &str, is_active: bool) -> Vec<DuckTargetChange>:
        self.active_voices.insert(voice_id.to_string(), is_active);
        for each voice in potential_voices(): let (target, fade_frames) = resolve_target(&voice);
          let last = last_targets.get(&voice).copied().unwrap_or(1.0);
          if (target-last).abs() > f32::EPSILON { last_targets.insert(voice.clone(), target); push DuckTargetChange }
      potential_voices(&self) -> HashSet<String>: every primary + ducked voice across rules.
      resolve_target(&self, voice_id) -> (f32, usize): find_applicable_rules; if empty -> (1.0, ms_to_frames(2000));
        else (min target_volume by total_cmp, ms_to_frames(min fade_duration_ms)).
  - Tests to ADD: applier_ducks_then_restores; compute_changes_only_emits_on_target_move; applier_matches_legacy_engine
    (drive control.compute_changes then applier.apply_target and assert the multiplier matches a legacy
    engine.notify_voice_active+get_multiplier within 1e-4).

MixerState (src/audio/mixer.rs): change field \`ducking_engine: Option<DuckingEngine>\` to
\`ducking_applier: Option<DuckingApplier>\`; update import; update MixerState::new (#[cfg(test)]) default;
in mix_audio the two get_multiplier sites use \`state.ducking_applier\` / \`applier.get_multiplier(...)\`.
Also update src/audio/test_support.rs SceneBuilder: field ducking_engine->ducking_applier, the ducking()
method takes a DuckingApplier, build() maps it. Update any render-harness/bench test that built a ducking
scene with a DuckingEngine to build a DuckingApplier (apply_target to set up duck state).

ENGINE CALLBACK (src/audio/engine.rs):
  - run_mix_callback(bus, callback_state: &Arc<Mutex<rt_engine::AudioCallbackState>>, xruns: &Arc<AtomicU64>):
      let mut guard = callback_state.lock();
      let acs = &mut *guard;  // ONE deref, then destructure for disjoint &mut:
      let rt_engine::AudioCallbackState { mixer, commands, graveyard } = acs;
      rt_engine::drain_commands(commands, mixer, OUTPUT_SR?, 64);  // SR: store output sample rate in the bundle or pass it in — see note
      crate::audio::mixer::mix_audio(bus, mixer);
      rt_engine::reap_finished(mixer, graveyard);
    DELETE the old voices_before/voices_after HashSet building, the ducking notify loop, and the
    *active_voices.lock() write — all gone (reconciliation is control-side now).
    NOTE on sample rate for fades during drain: add \`pub output_sample_rate: u32\` to AudioCallbackState so
    drain_commands has it (fades need it). Set it at construction.
  - build_typed_output_stream / build_output_stream / spawn_output_supervisor: replace the
    (mixer_state: Arc<Mutex<MixerState>>, active_voices: Arc<Mutex<HashSet<String>>>) parameters with
    (callback_state: Arc<Mutex<rt_engine::AudioCallbackState>>, xruns: Arc<AtomicU64>). Thread them through.
    In the cpal error callback, increment xruns (xruns.fetch_add(1, Ordering::Relaxed)) IN ADDITION to the
    existing error_flag store. Use std::sync::atomic::AtomicU64.

MAIN (src/main.rs) SETUP (around the current mixer_state/active_voices creation ~441-517, 605-667):
  - let (cmd_tx, cmd_rx) = rt_engine::command_channel(1024);
  - let (grave_tx, mut grave_rx) = rt_engine::graveyard_channel(1024);
  - build MixerState (ducking_applier: if ducking rules configured Some(DuckingApplier::new()) else None;
    keep bass_management as today; output_channels as today).
  - let callback_state = Arc::new(parking_lot::Mutex::new(rt_engine::AudioCallbackState{ mixer, commands: cmd_rx, graveyard: grave_tx, output_sample_rate }));
  - let xruns = Arc::new(AtomicU64::new(0));
  - control-side DuckingEngine: Some(DuckingEngine::new(rules, output_sample_rate)) if rules configured, kept
    in the control loop (NOT in MixerState). active-voice counts: HashMap<String, usize>.
  - status snapshot: Arc<RwLock<StatusSnapshot>> (std::sync::RwLock). Define StatusSnapshot (see HTTP).
  - spawn_output_supervisor(callback_state.clone(), xruns.clone(), ...).
  - The live-input setup that did mixer_state.lock().live_inputs.push(live_input) now pushes
    AudioCommand::AddLiveInput(Box::new(live_input)) onto cmd_tx.

COMMANDCTX + handle_command (src/main.rs): CommandCtx no longer holds mixer_state/active_voices. It holds:
  cmd_tx: &CommandProducer (note: producer is single-owner; handle_command is the only caller — it runs in the
  command loop — so the loop owns cmd_tx and passes &mut or via a RefCell/&; simplest: handle_command takes
  &mut CommandCtx or the producer is in the ctx behind &mut). Also ducking engine (&mut Option<DuckingEngine>),
  active counts (&mut HashMap), snapshot (&Arc<RwLock<StatusSnapshot>>), cache_manager + voice_manager + config
  as before. Convert ALL 13 mixer_state.lock() handler bodies to push the matching AudioCommand:
    Play -> build ActiveSample as today (cache load, voice_manager, channel resolve), then: if ducking engine
      present, for ch in engine.compute_changes(&voice_id, true) push SetDuckTarget(ch); bump active_counts[voice];
      push AddSample(Box::new(sample)); refresh snapshot.
    StopAll -> push FadeOutAll{fade_ms:10}.  shutdown() -> push FadeOutAll{fade_ms:50}.
    VoiceStop -> sample_ids = voice_mgr.clear_voice(voice); push FadeOutSamples{ids:sample_ids, fade_ms:10}.
    VoiceFadeOut -> ids = voice_mgr.get_voice_sample_ids(voice); push FadeOutSamples{ids, fade_ms:time_ms}.
    VoiceVolume -> set in voice_mgr (as today) then push SetVoiceVolume{voice, volume:actual}.
    InputVolume -> push SetInputVolume{input, volume}.  InputMute -> push SetInputVolume{input, volume: if mute 0 else 1}.
    Seek -> push SeekMatching{selector, position_ms}.  Speed -> push SetSpeedMatching{selector, speed, pitch_correction}.
    Stop -> push FadeOutMatching{selector, fade_ms: fade_out_ms.unwrap_or(10)}.  Volume -> push SetVolumeMatching{selector, volume}.
    Precache/CacheClear/CacheInvalidate -> UNCHANGED (they touch the cache, not mixer_state).
  Selector-based commands carry the SampleSelector (Clone); the audio side matches it. The "matched no samples"
  warnings are dropped (fire-and-forget); that is an accepted behavior change — do not try to preserve them.

CONTROL LOOP (the tokio command loop in main.rs ~640-700): add a periodic reaper. Use
  tokio::select! over { the existing command source, a tokio::time::interval(20ms) tick, the shutdown signal }.
  On each tick: while let Some(finished) = grave_rx.pop() { let v = finished.voice_id.clone(); drop(finished);
  let c = active_counts.get_mut(&v); decrement; if it reaches 0 { remove; if engine present, for ch in
  engine.compute_changes(&v, false) push SetDuckTarget(ch) } }; then refresh the snapshot. (Dropping \`finished\`
  here is the off-RT free.) Keep the existing graceful-shutdown behavior (fade then flush cache) but trigger
  the fade via FadeOutAll command, and give the callback a moment to drain it as today.

HTTP (src/http/mod.rs AppState + src/http/handlers.rs): AppState replaces mixer_state with
  status: Arc<RwLock<StatusSnapshot>>. Define StatusSnapshot { active_samples: usize, samples: Vec<SampleStatus>,
  inputs: Vec<InputStatus> } with whatever fields handle_status/handle_samples/handle_inputs currently read from
  mixer.active_samples / mixer.live_inputs (id, sample_id, voice_id, file_path for samples; index/voice/volume
  for inputs). The control thread builds/refreshes it (sample count + per-sample static info it knows; live
  POSITION is not available control-side, so omit position or report 0 and note it). handle_status/samples/inputs
  read the snapshot (status.read()). Everything that went through send_command stays as-is (commands funnel to
  handle_command).

CONSTRAINTS: parking_lot::Mutex for callback_state. std::sync::RwLock for the snapshot. No new crates. Keep the
  existing Sprint-2 graceful shutdown + Sprint-1 device rebuild working. mix_audio body unchanged. Do not remove
  comments unless provably false. Two ABOUTME lines on any new file.
`

phase('Core')
const core = await agent(
  `You are implementing the PRODUCTION CODE for the Sprint 5 lock-free RT engine on the main repo tree at /Users/bandrews/src/mqttaudio. ` +
  `Make all the changes in the "Core stage" scope below. Goal for THIS stage: the non-test production code is complete and \`cargo build --release\` compiles ` +
  `(tests may not compile yet — the next agent fixes tests). Read every file before editing it. Work carefully; this is the heart of the app.\n\n` +
  DESIGN +
  `\n\nWhen done, run \`cargo build --release 2>&1 | tail -40\` and report: the exact files changed, whether the release build (non-test) compiles, any remaining errors verbatim, and anything you were unsure about or deviated on. Be honest about what is incomplete.`,
  { label: 'core', phase: 'Core', model: 'opus' }
)

phase('Green')
const green = await agent(
  `A previous agent implemented the Sprint 5 lock-free RT engine production code on the main tree at /Users/bandrews/src/mqttaudio. ` +
  `Here is that agent's report:\n\n${core}\n\n` +
  `YOUR JOB: make the WHOLE project green to the project's gate, with NO shortcuts. Specifically:\n` +
  `1. Rewrite the ~15 binary unit tests in src/main.rs (the \`Fixture\`-based tests that previously asserted via mixer_state.lock()) to the new command model: build the rings + an AudioCallbackState (or just a MixerState + a CommandConsumer), drive handle_command (or push AudioCommand directly), drain the command ring into the mixer state via rt_engine::drain_commands, and assert the resulting MixerState — preserving each test's ORIGINAL intent. Do NOT delete, #[ignore], or weaken any test to pass; if a test cannot be expressed, STOP and report it rather than hacking.\n` +
  `2. Extend tests/alloc_harness.rs with a test that builds an AudioCallbackState (representative scene: plain + looping + pitch sample, ducking applier with a target applied) and, after warmup, asserts that one full callback step — lock + drain_commands + mix_audio + reap_finished — does ZERO allocations and ZERO frees across several blocks (use the existing counting allocator pattern; the lock itself must not allocate).\n` +
  `3. Ensure render-harness parity: the existing tests/render_harness_test.rs and src/audio tests still pass (mix output unchanged).\n` +
  `4. Reach FULL GREEN by correct means only: \`cargo build --release\` (no warnings), \`cargo clippy --all-targets -- -D warnings\` (clean), \`cargo fmt\` then \`cargo fmt --check\`, and \`cargo test\` (all pass). Fix root causes. If the previous stage left the production code wrong, FIX it (you have full latitude on the production code too).\n\n` +
  DESIGN +
  `\n\nReport: the final results of build, clippy, fmt --check, and \`cargo test\` (paste the test result summary lines), the files you changed, how many Fixture tests you rewrote, and anything still failing or any place you were forced to compromise. Be brutally honest — a false "it's green" is the worst outcome.`,
  { label: 'green', phase: 'Green', model: 'opus' }
)

phase('Verify')
const VERDICT = {
  type: 'object',
  additionalProperties: false,
  required: ['lens', 'issues', 'summary'],
  properties: {
    lens: { type: 'string' },
    summary: { type: 'string', description: 'one-line overall judgment' },
    issues: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['severity', 'file', 'description'],
        properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          file: { type: 'string' },
          description: { type: 'string' },
        },
      },
    },
  },
}
const lenses = [
  ['rt-safety', 'Audit the audio callback path (src/audio/engine.rs run_mix_callback + the cpal closures, src/audio/mixer.rs mix_audio, rt_engine drain/apply/reap). Confirm: the control thread NEVER locks callback_state (grep main.rs/http for it locking the bundle); the callback does no heap allocation and no free (the only lock is the uncontended callback_state lock; reap moves finished samples to the graveyard, it does not drop them); drain is bounded. Report any allocation/free/contended-lock on the RT path.'],
  ['parity', 'Verify behavior parity vs the pre-change handlers. For each of the 13 commands, compare what apply_command does on the audio side to what the OLD handle_command did under mixer_state.lock() (use git to see the old code). Confirm fades/seek/speed/volume/voice-volume/input semantics match, and that ducking via compute_changes->SetDuckTarget reproduces the old notify_voice_active behavior. Report any semantic drift.'],
  ['threading', 'Audit cross-thread correctness: command-ring producer single-owner; graveyard producer on audio side / consumer on control side; the control-side DuckingEngine + active_counts + snapshot are only touched by the control loop; no data races; stream-rebuild still works (callback_state is an Arc clone into the rebuilt closure; rings survive). Report any race, ownership, or rebuild-survival bug.'],
  ['coverage', 'Audit the tests: were ALL ~15 Fixture tests preserved with their original intent (not weakened/deleted/ignored)? Does the alloc harness actually exercise a full callback step (lock+drain+mix+reap) and assert zero alloc/free? Is ducking-off-RT and the graveyard reaper covered? Use git to compare test counts before/after. Report any test that was weakened, removed, or that tests mocked behavior.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(
    `You are an adversarial reviewer auditing an uncommitted Sprint 5 RT-engine integration on the main tree at /Users/bandrews/src/mqttaudio (read-only review; do not edit). Use \`git diff\` to see the change. Lens: ${lens}.\n\n${prompt}\n\nDesign context (option D):\n${DESIGN}\n\nReturn your findings.`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT }
  )
))

return { core, green, findings: findings.filter(Boolean) }
