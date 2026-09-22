export const meta = {
  name: 's9-cleanup',
  description: 'Sprint 9 cleanup/observability/packaging — scaffolding removal, /metrics, proptest, tests, systemd',
  phases: [
    { title: 'TestGaps', detail: 'F9 proptest, F10 HTTP errors, F11 websocket, F12 real-value status' },
    { title: 'Cleanup', detail: 'F1 scaffolding, F2 banners, F4 config, F5 voice-id, F6 seek, F7/F8 warnings' },
    { title: 'Observability', detail: 'F3 /version + /metrics + enriched status, F13 JSON logging + systemd' },
    { title: 'Green', detail: 'full green + docs + CHANGELOG' },
    { title: 'Verify', detail: 'adversarial audit' },
  ],
}

const SPEC = `
Repo /Users/bandrews/src/mqttaudio (branch v2.1), HEAD green (cargo build -D warnings / clippy --all-targets
-D warnings / fmt --check / cargo test / ./scripts/validate.sh all pass). This is Sprint 9 — the FINAL sprint:
cleanup, observability, packaging, and the deferred test-coverage items, on top of Sprints 1-8.

AUTHORITATIVE SPEC: read docs/sprints/sprint-09-cleanup-observability-packaging.md IN FULL (13 findings F1-F13,
exact file:line, fixes, the ordered TDD task list, and the OUT-OF-SCOPE caveats — especially: do NOT touch
MIN_BUFFER_FRAMES / mark_playing / cleanup_completed_loads (Sprint 4) or ducking_rules.target_volume validation
(Sprint 2)). Read the locked decisions DECISIONS.md D38 (skip hot-reload), D39 (remove dev scaffolding +
--test-tone/--file/--test-mixer flags + hardcoded /Users/bandrews paths), D40 (/version + /metrics: uptime,
active voices, clip/over count, xrun/dropout, per-voice ducking), D41 (seek clamps to total_frames_or_estimate;
unique auto voice id _auto_<millis>_<n>; the crossfade-without-loop + channel_map<->LFE warnings are
doc/warning only).

Follow CLAUDE.md + the SPRINT-TRACKER charter: NO shortcuts; never weaken/delete/#[ignore] a test to pass
(the ONE sanctioned #[ignore] is the Lane-B device smoke); root-cause only; smallest reasonable change. When a
counter is produced by an earlier sprint, SURFACE it — never fabricate a value: the Sprint-5 xrun counter is
xruns: Arc<AtomicU64> (created in main.rs, incremented in the cpal error callback) and the Sprint-6 limiter
clip counter is clip_count: Arc<AtomicU64> (on MixerState, read today in handle_status) — thread these into
/metrics. If something is genuinely absent, surface what exists and log the gap in docs/bugs.md.

RT-SAFETY: this sprint is cleanup/observability/tests — do NOT add allocation/free/lock to the audio callback;
tests/alloc_harness.rs (5+ tests) must stay green. Surfacing the xruns counter on /status finally closes the
documented Sprint-5 residual.

GREEN GATE: cargo fmt --check; RUSTFLAGS="-D warnings" cargo build --release (with NO file-level
#![allow(dead_code)] banner remaining in chunked_resampler.rs and no NEW blanket allows); cargo clippy
--all-targets -- -D warnings; cargo test ALL pass. The proptest suite must run under Lane A — wire it into
scripts/validate.sh if needed. A false green is the worst outcome.
`

phase('TestGaps')
const tests = await agent(
  `Sprint 9 TEST-GAPS group on the main tree (F9, F10, F11, F12), TDD-first. These are additive tests; they must not change production behavior.\n` +
  `- F9: add proptest as a [dev-dependencies], write tests/fuzz_command_config.rs asserting parse_command(arbitrary json), expand_macros(arbitrary), and Config-from-arbitrary-JSON each return Ok/Err and NEVER panic. If a real panic is found that is another sprint's bug, write the failing case, #[ignore] it with a pointer, and log in docs/bugs.md. Wire the proptest suite into scripts/validate.sh so Lane A runs it.\n` +
  `- F10: in tests/http_api_test.rs add error-path tests: POST /command non-JSON -> 400 + CommandResponse error shape; POST /play and /volume missing required field -> 4xx; drop cmd_rx then POST -> 500 with success:false.\n` +
  `- F11: unit-test LogVisitor (synthetic message + non-message first field) and WebSocketLogLayer::on_event format; add an axum WebSocket integration test (router with websocket_enabled=true) upgrading /ws, asserting the welcome {type:"connected",..version..} JSON, broadcasting via LogBroadcaster, and asserting the client gets {type:"log",message:..}.\n` +
  `- F12: mirror test_samples_endpoint_returns_position_ms — populate voices/inputs/cache and assert serialized VALUES (muted toggles with volume==0.0, channels reported, cache entry counts/sizes), not just is_array()/is_object().\n` +
  SPEC +
  `\nKeep build + all tests green. Report files changed, the new tests, any panic found (with its owning sprint), and confirm the proptest suite runs under scripts/validate.sh.`,
  { label: 'test-gaps', phase: 'TestGaps', model: 'opus' }
)

phase('Cleanup')
const cleanup = await agent(
  `Sprint 9 CLEANUP group on the main tree (F1, F2, F4, F5, F6, F7, F8), building on the test-gaps work, TDD-first for behavior changes.\n` +
  `- F1 (D39): delete init_test_sine_wave/play_file/test_mixer from engine.rs (incl. hardcoded /Users/bandrews paths) + their now-unused imports; delete the --test-tone/--file/--test-mixer Args fields + their handler blocks in main.rs. Removal only — must not touch the real command path. Build stays warning-free.\n` +
  `- F2: remove the #![allow(dead_code)] "Phase 10" banner in chunked_resampler.rs; build -D warnings; remove any now-genuinely-dead item OR add a NARROW accurately-commented #[allow(dead_code)] on the specific item (never re-blanket). If removing it surfaces the file-level blankets in streaming_decoder.rs:6 / cache/http_stream.rs:6 and they break the gate, address them the same narrow way; otherwise log in docs/bugs.md. Do NOT touch Sprint-4 items (MIN_BUFFER_FRAMES/mark_playing/cleanup_completed_loads).\n` +
  `- F5 (D41, behavior change): unique auto voice id — append a process-global AtomicU64 (or VoiceManager sample id) so two same-millisecond Plays without an explicit voice get distinct ids (_auto_<millis>_<n>). Failing test: two no-voice Plays -> distinct voice_id. Changelog.\n` +
  `- F6 (D41, behavior change): seek clamps against total_frames_or_estimate() (like start_position_ms), not frames(). Failing test on a streaming buffer: forward seek past the loaded edge lands at the requested frame. Changelog.\n` +
  `- F4: add an optional schema_version config field (default current; warn on unknown/newer); add bass source-channel sanity (reject duplicate sources, or a source == resolved LFE channel) with clear messages including the offending value. Failing tests for each. Do NOT touch ducking_rules.target_volume validation (Sprint 2).\n` +
  `- F7: one-time startup tracing::warn! when bass management is enabled and a configured route/channel_map destination can equal the LFE channel; document the bypass in README. Doc/warning only.\n` +
  `- F8: tracing::warn! at command dispatch when crossfade_ms is set without loop:true (or crossfade_samples*2 >= buffer len); document loop-boundary scope in README. Test captures the warning (pristine output). No blend-math change.\n` +
  SPEC +
  `\nKeep build + all tests + tests/alloc_harness.rs green. Report files changed, new tests, and confirm the build is warning-free with no blanket allow(dead_code) reintroduced.`,
  { label: 'cleanup', phase: 'Cleanup', model: 'opus' }
)

phase('Observability')
const obs = await agent(
  `Sprint 9 OBSERVABILITY group on the main tree (F3, F13), building on prior work, TDD-first.\n` +
  `- F3 (D40): capture start_time: Instant at daemon start, thread it (+ the Sprint-5 xruns Arc<AtomicU64> and Sprint-6 clip_count Arc<AtomicU64> and a per-voice ducking snapshot) into AppState. Add /version (returns {name, version, git_sha?}) and /metrics (real fields: uptime_seconds>0, xruns, clips/over, active voices, per-voice ducking state) routes in routes.rs + handlers in handlers.rs. Enrich /status (or /status/voices) with per-voice ducking state. Failing tests: /version returns name+version; /metrics returns the real fields (uptime>0; clips/xruns are numbers — assert exact serialized shape, never a placeholder). Where a counter is truly absent, surface what exists + log the gap in docs/bugs.md. This finally surfaces the xruns counter (closes the Sprint-5 residual).\n` +
  `- F13: add a logging.format config option ("text" default | "json") building a tracing_subscriber::fmt().json() layer in main.rs when selected, keeping the MQTT log layer composable; a test (or documented manual check) that JSON mode emits line-delimited JSON. Add packaging/mqttaudio.service (Type=simple, config path, Restart=on-failure, a hardening stanza). Document both in README.\n` +
  SPEC +
  `\nKeep build + all tests + tests/alloc_harness.rs green (no callback alloc/lock added). Report files changed, new tests/routes, and confirm /metrics surfaces the real xruns + clip counters.`,
  { label: 'observability', phase: 'Observability', model: 'opus' }
)

phase('Green')
const green = await agent(
  `Sprint 9 GREEN GATE on the main tree. Prior agents implemented the test-gaps, cleanup, and observability work (reports below). Bring the WHOLE project to the FINAL gate by correct means and finalize the program:\n` +
  `1. Reach: cargo fmt --check; RUSTFLAGS="-D warnings" cargo build --release (NO #![allow(dead_code)] banner left in chunked_resampler.rs; no new blanket allows); cargo clippy --all-targets -- -D warnings; cargo test (ALL pass incl. proptest, the new HTTP/WebSocket/status tests, and tests/alloc_harness.rs). Confirm scripts/validate.sh runs the proptest suite. Fix root causes; if an earlier group left something wrong, fix it.\n` +
  `2. Finalize docs: README.md (systemd unit, JSON logging, /version + /metrics, the F7/F8 feature-interaction notes, schema_version), CHANGELOG.md (behavior changes: F5 voice-id format, F6 seek, plus the new routes/fields), docs/bugs.md (any surfaced/deferred gaps).\n` +
  `Report exact gate results (paste the test result summary lines), confirm alloc_harness passes, list files changed, and flag anything incomplete. A false green is the worst outcome — be brutally honest.\n\n` +
  SPEC +
  `\n\n--- TEST-GAPS REPORT ---\n${tests}\n\n--- CLEANUP REPORT ---\n${cleanup}\n\n--- OBSERVABILITY REPORT ---\n${obs}`,
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
  ['cleanup-warnings', 'Confirm the dev scaffolding is GONE (no init_test_sine_wave/play_file/test_mixer, no --test-tone/--file/--test-mixer flags, no /Users/bandrews hardcoded paths) and the chunked_resampler.rs #![allow(dead_code)] banner is removed with the build still -D-warnings clean and NO new blanket allow(dead_code) (any remaining allow is narrow + accurately commented). Confirm Sprint-4 items (MIN_BUFFER_FRAMES/mark_playing/cleanup_completed_loads) and Sprint-2 ducking validation were NOT touched. Use git diff + grep. Report any leftover scaffolding, reintroduced blanket allow, or out-of-scope edit.'],
  ['observability-metrics', '/version + /metrics return REAL data, not placeholders: uptime from a real start_time; xruns from the Sprint-5 Arc<AtomicU64>; clips from the Sprint-6 clip_count; per-voice ducking state from the real engine. Verify the counters are actually threaded (not hardcoded 0/fake). F5 unique voice-id and F6 seek-to-estimate are implemented + tested + changelogged. proptest fuzz runs under scripts/validate.sh and asserts no-panic. Report any fabricated value or missing wiring.'],
  ['parity-tests', 'Confirm no test weakened/deleted/ignored (except the sanctioned device smoke + any documented proptest-found-panic #[ignore] with a bugs.md pointer); the gate is genuinely green (read the green report critically); RT-safety intact (alloc_harness green, no callback alloc/lock added); README/CHANGELOG/bugs.md updated; systemd unit + JSON logging present + documented. Report gaps.'],
]
const findings = await parallel(lenses.map(([lens, prompt]) => () =>
  agent(`Adversarial read-only review of the uncommitted Sprint 9 changes on the main tree at /Users/bandrews/src/mqttaudio (do not edit; git diff + read tests). Lens: ${lens}.\n\n${prompt}\n\n${SPEC}`,
    { label: `verify:${lens}`, phase: 'Verify', model: 'opus', schema: VERDICT })
))

return { tests, cleanup, obs, green, findings: findings.filter(Boolean) }
