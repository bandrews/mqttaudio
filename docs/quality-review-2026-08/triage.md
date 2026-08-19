# Quality Review Triage

Findings are recorded in the four `findings-*.md` files in this folder. This
document splits them into: fixed now (clearly broken, narrow fix available)
vs. deferred (needs a product/design decision, or is too invasive for a
review-scoped change). The summary of outcomes lives in `summary.md`.

## Fixed in this review

| # | Issue | Where |
|---|---|---|
| F1 | MQTT subscription lost after reconnect (resubscribe on ConnAck) | mqtt/client.rs |
| F2 | `mqtt.client_id` ignored | mqtt/client.rs |
| F3 | `mqtt.reconnect_delay_seconds` ignored (hardcoded 5 s) | mqtt/client.rs |
| F4 | Username-without-password silently ignored | mqtt/client.rs |
| F5 | Macros silently dropped for nested `message` format | mqtt/commands.rs |
| F6 | Unknown macro names silently ignored (now logged) | mqtt/commands.rs |
| F7 | `fadeout`/`soundFadeOut` legacy commands missing | mqtt/commands.rs |
| F8 | No HTTP timeouts — stalled server wedges loads forever | cache/disk.rs, cache/http_stream.rs |
| F9 | Failed streaming URL poisoned until restart | cache/mod.rs |
| F10 | Pitch-corrected sample goes silent when its streaming load completes | audio/mixer.rs |
| F11 | Reverse playback interpolates against the wrong neighbor | audio/mixer.rs |
| F12 | Streaming samples killed by lock contention / loader stalls | audio/streaming.rs, audio/mixer.rs |
| F13 | Sample created during loader lock contention permanently silent | audio/mixer.rs, audio/streaming.rs |
| F14 | Ducking fades advance N× too fast with N samples on a voice | audio/mixer.rs, audio/ducking.rs |
| F15 | Ducking restore time hardcoded 2000 ms; duck fade restarted on every activity notification | audio/ducking.rs |
| F16 | `audio.buffer_size` never applied to the output stream | audio/engine.rs, main.rs |
| F17 | `logging.verbose` in config file has no effect | main.rs |
| F18 | HTTP `/input/mute` with omitted `mute` field silently unmutes | http/handlers.rs |
| F19 | `seek` clamps to streaming load frontier (or frame 0 under contention) | main.rs |
| F20 | `security.allowed_directories` unenforced when configured | main.rs (+ tests) |
| F21 | Doc corrections: wrong issue tracker, nonexistent `--buffer-size` flag, nonexistent env vars, wrong defaults (http.port, latency_ms, stop fade), stale volume ranges, undocumented endpoints/params/aliases, false caching promises, mic-ducking examples, example-config errors, README quick-start branch | docs/*, config.example.json, README.md |

Notes on F20: enforcement is implemented for a **non-empty** list — local
paths must resolve (canonicalized, symlinks followed) under an allowed
directory, which also blocks `../` traversal. What an **empty** list means is
a product decision (docs promised "HTTP-only", but the README quick start and
every existing default-config setup plays local files) — empty continues to
allow all local paths, docs updated to say so, decision deferred (D1).

## Deferred — needs a decision or a design

| # | Issue | Why deferred |
|---|---|---|
| D1 | Empty `allowed_directories` = allow-all (current) vs deny-local (as previously documented) | Breaking change for every default-config user; conflicts with README quick start |
| D2 | `cache.enabled` dead field | Decide: implement bypass or remove the field |
| D3 | `cache.revalidate_after_seconds` / ETag revalidation unimplemented | Real feature work (conditional requests); docs corrected meanwhile |
| D4 | Streamed HTTP audio never written to disk cache | Design: stream-to-disk tee, atomicity |
| D5 | Completed streaming loads never promoted to memory cache; `max_memory_mb` not honored for them; `cleanup_completed_loads` unwired | Design: promotion vs. re-download trade-off; interacts with D4 and the latent double-subtract in `MemoryCache::put` |
| D6 | Command loop serialization: slow/blocked `play` delays `stopall`; cache mutex held across await | Architecture (async mutex / task-per-load); F8's timeouts bound the damage |
| D7 | Mic-triggered ducking (`primary_voice` = live input) | New feature: needs signal-level gate + notify wiring; docs corrected meanwhile |
| D8 | WebSocket log streaming dead (tracing layer never installed); `/ws` unauthenticated | Feature wiring across main/http boundary; docs corrected meanwhile; fix auth together with it |
| D9 | HTTP command endpoints always return success (fire-and-forget) | API design change (correlation/result reporting) |
| D10 | VoiceManager / DuckingEngine voice maps grow unboundedly | Needs a cleanup hook design (audio callback can't do it); real leak on long-running installs |
| D11 | Real-time violations in callback (vec alloc in pitch path, HashSets + tracing in finished-voice scan) | RT-safety rework, needs careful benching |
| D12 | Loop crossfade double-plays the head; loop+pitch don't compose | Audible but subtle; correct fix changes loop timing semantics |
| D13 | Resampler tail loss / initial latency (one-shot and chunked flush) | Needs rubato drain rework + test updates |
| D14 | Input capture resampler hardcodes Maximum quality; capture-thread reallocation | Decide: reuse `advanced.resampler_quality` or a per-input setting |
| D15 | One corrupt packet aborts whole decode (Symphonia DecodeError is recoverable) | Behavior change; needs corrupt-file fixtures |
| D16 | `speed: 0` coerced to 0.01; reverse-from-start instantly finishes | Semantics decision (pause? error?) |
| D17 | Pops on `input_volume`/`input_mute`/per-sample `volume` (no ramp) | Needs ramp plumbing like `voice_volume` |
| D18 | Ducking rules not validated (`target_volume` range, unmatched voice names) | Validation easy but semantics (boost allowed?) need a call. The fade-restart half was fixed with F15 |
| D19 | Env vars (`MQTTAUDIO_CONFIG`, `MQTTAUDIO_CACHE_DIR`, `RUST_LOG`) unimplemented | Decide: implement or drop permanently (docs corrected meanwhile) |
| D20 | `--lfe-channel`/`--crossover-frequency` can't activate bass management alone | Decide intended CLI story |
| D21 | Bass management silently no-ops when `lfe_channel` >= device channels; duplicate source entries corrupt filter state | Startup warning easy; wants validation design with D18 |
| D22 | IPv6 `bind_address` unsupported; auth token compare non-constant-time and query form not URL-decoded | Small but security-adjacent; batch with D8 |
| D23 | Cache filename `DefaultHasher` unstable across toolchains; query strings defeat extension detection; non-atomic cache writes; unbounded RAM buffering of downloads; reader drop doesn't cancel download | Cache-layer hardening batch |
| D24 | Streaming `total_frames` estimated from encoded bytes — status progress nonsense for MP3/OGG | Needs duration probing |
| D25 | `POST /play` lacks `channel_map`; `internal_id` selector accepts only numbers silently | API surface polish |
| D26 | lib.rs module tree duplicates the binary's and has drifted | Build restructure |
| D27 | `--test-mixer` hardcodes developer paths; `play_file` never exits | Dev-tool polish |
| D28 | Channel-map destinations beyond device width silently produce silence (no command-time warning) | Wants a warning design (device width known only after engine start) |
| D29 | `MemoryCache::put` double-subtract on key replacement (latent) | Fix together with D5 promotion work |
| D30 | CHANGELOG claims not matched by code (revalidation, eviction protection, promised docs) | Historical record; decide whether to annotate or amend |
