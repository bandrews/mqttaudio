# Post-Review Sprint Plan

Decisions resolved with Ben on 2026-08-19; this plan turns the deferred
review findings (see `triage.md`) into ordered, committable work. All work
happens on `claude/mqttaudio-quality-review-u1gcz7` with a commit per
completed item or coherent group, tests first where the layer allows.

## Decisions of record

| Topic | Decision |
|---|---|
| Empty `security.allowed_directories` (D1) | Empty = local playback unrestricted. Keep, and make sure docs state it plainly. |
| `cache.enabled` (D2) | Implement for real: `false` bypasses the disk cache entirely. |
| Revalidation (D3) | Build it: conditional requests using stored ETag/Last-Modified, gated by `revalidate_after_seconds` (0 = check every access). |
| Stream caching (D4/D5) | Fix both: completed streaming downloads write through to the disk cache AND are promoted into the size-limited memory cache. |
| WebSocket logs (D8) | Wire the tracing layer so `/ws` really streams logs, and put `/ws` behind the auth token. |
| Mic-triggered ducking (D7) | Build voice-activity detection on live inputs (per-input threshold + hold). Voice-level ducking stays the mechanism; per-output-channel duck scoping is deferred until someone needs it. |
| HTTP command results (D9) | Command endpoints wait for processing and report the real outcome. |
| Command loop (D6) | Restructure: loads run as their own tasks so control commands (stopall etc.) are never queued behind a download. |
| `speed: 0` (D16) | Reject with an error (no silent 0.01 coercion). |
| Env vars (D19) | Implement `MQTTAUDIO_CONFIG` (config path) and `RUST_LOG` (EnvFilter log filtering). |
| Mic resampler (D14) | The input capture path obeys `advanced.resampler_quality`. |
| Voice/ducking leaks (D10) | Fix in this program (pairs with the command-loop work). |
| CHANGELOG (D30) | Correct inaccurate 2.0 entries in place; new work gets honest entries. |

## Sprint 1 — Cache correctness

Goal: the cache does what its config says, holds its budget, and survives
restarts for everything it downloads.

1. **`cache.enabled: false` bypasses the disk cache.** No reads, no writes;
   HTTP fetches still work. Memory cache unaffected. Tests: disabled manager
   leaves the cache dir empty and re-downloads.
2. **Atomic disk-cache writes.** Download to a temp file in the cache dir,
   rename into place, so a crash can never leave a truncated file that
   existence-only checks then serve. (Pre-req for 3 and 4.)
3. **Stable cache filenames.** Replace `DefaultHasher` (unstable across Rust
   releases) with SHA-256 as the filename hash, and strip query strings
   before extension detection. One-time effect: existing cache entries keep
   working via their stored `local_file`, new downloads use new names.
4. **Write-through for streaming downloads.** The streaming loader tees raw
   bytes to the disk cache (via the atomic path) and registers the entry on
   completion. Runtime `precache` therefore also persists. Tests: play a URL,
   restart the manager, confirm a disk hit and no re-download.
5. **Promote completed streaming loads into the memory cache.** On
   completion, convert the streaming buffer's PCM into a `DecodedBuffer`,
   `put` it in `MemoryCache` (LRU budget applies), and drop the
   `active_loads` entry. Includes fixing the latent `MemoryCache::put`
   double-subtract on key replacement (D29). Tests: budget enforced across
   streamed loads; replacing a key keeps `current_size_bytes` consistent.
6. **Revalidation.** Store fetch time per entry; when a cached URL is
   requested and `revalidate_after_seconds` has elapsed since the last check
   (0 = always), send a conditional GET (`If-None-Match`/
   `If-Modified-Since`). 304 → refresh the timestamp and serve the cached
   copy; 200 → replace the entry (atomic write) and re-decode; network error
   → serve the cached copy and log. Tests against the local axum test server
   covering 304, 200-changed, and offline paths.
7. Update `caching.md`, `configuration.md`, `config.example.json` to describe
   the now-real behavior; drop the "reserved" wording.

## Sprint 2 — Command loop, HTTP results, leaks

Goal: control commands are always instant, HTTP clients learn real outcomes,
and long uptimes stop leaking.

1. **Loads leave the command loop.** `play`/`precache` spawn a per-request
   task that does the (possibly slow) load and then hands the ready sample to
   the mixer; the loop itself never awaits a download or decode. The cache
   manager moves behind an async-aware lock so nothing holds a std mutex
   across awaits (also un-wedges `/status` during loads).
2. **`stopall`/`fadeall` cancel pending loads** started before them, so a
   stop always silences everything even mid-load.
3. **HTTP endpoints report outcomes.** Commands carry a oneshot reply
   channel; the processing side resolves it with success or a typed error
   (parse error, file not found, path rejected, no samples matched...).
   HTTP handlers wait (with a timeout) and map to proper status codes.
   MQTT behavior unchanged (logs only). http-api.md updated; the
   fire-and-forget caveat removed.
4. **`speed: 0` rejected** at parse time with a clear error (and now
   surfaces through the HTTP result path). Docs note reverse requires a
   prior seek.
5. **Leak cleanup (D10).** When samples finish, their IDs leave
   `VoiceManager` and empty auto-voices are dropped; `DuckingEngine`
   forgets inactive voices with no duck state. Mechanism: the audio callback
   already computes finished voices; route that through a lock-free channel
   to the command-loop side, which prunes. `/status/voices` stops reporting
   ghosts. Tests: play-to-completion removes the voice.

## Sprint 3 — Microphone voice activity ducking

Goal: the escape-room story works: gamemaster talks, room audio ducks.

1. **Level detection in the capture path.** Each `inputs[]` entry gets
   `activity_threshold` (linear level, default off/null) and
   `activity_hold_ms` (default 750). The capture thread computes a running
   peak/RMS per chunk and publishes it to an atomic; no allocation or
   locking added to the audio callback.
2. **Activity → ducking bridge.** A lightweight task polls input levels
   (~50 ms cadence), applies threshold + hold hysteresis, and calls
   `notify_voice_active(voice_id, ...)` off the audio thread. A mic's
   `voice_id` as `primary_voice` now triggers rules exactly like a playing
   sample.
3. **Validation + docs.** Ducking rules gain config validation (D18:
   `target_volume` 0.0–1.0, durations sane, warn on voice names that match
   neither samples-in-use nor inputs at load time where knowable). Restore
   the mic-ducking examples in `ducking.md`, `microphone-input.md`,
   `configuration.md` with the new fields and honest requirements.
   Per-output-channel duck scoping stays deferred (revisit on demand).
4. Tests: threshold/hold state machine unit tests; integration test feeding
   synthetic capture data through the bridge and asserting duck multipliers.

## Sprint 4 — Observability & polish

1. **WebSocket log streaming for real.** Create the `LogBroadcaster` in
   main, install `WebSocketLogLayer` in the tracing subscriber, pass the
   broadcaster into `start_server`; `/ws` moves behind the same auth
   middleware as command endpoints (token via query param supported, since
   browsers can't set headers on WebSocket). http-api.md rewritten to match;
   README gets its WebSocket bullet back.
2. **Env vars.** `MQTTAUDIO_CONFIG` as the config path when `--config` is
   absent; logging switches to `EnvFilter` so `RUST_LOG` works (falling back
   to the config level when unset). Documented in configuration.md.
3. **Mic resampler obeys `advanced.resampler_quality`** instead of hardcoded
   Maximum; fix the capture-thread `pending` reallocation while in there.
4. **CHANGELOG corrected in place** (2.0 claims match reality) plus entries
   for this program's work.
5. Final sweep: `cargo build --release` warning-free, clippy reviewed, full
   test suite, docs cross-check, update `triage.md` statuses and
   `summary.md`.

## Still deferred after this program

Audio-quality subtleties (loop-crossfade double-play D12, resampler tail
loss D13, RT allocations in the pitch path D11, corrupt-packet decode aborts
D15, volume-change pops D17), `--lfe-channel` CLI activation story (D20),
bass-management runtime warnings (D21), IPv6 bind + constant-time token
compare (D22), encoded-bytes progress estimates (D24), `channel_map` HTTP
parity (D25), lib.rs duplication (D26), dev-tool paths (D27), command-time
channel warnings (D28). Each stays tracked in `triage.md`.
