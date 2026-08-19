# Quality Review Summary

A full-product review of mqttaudio: every config setting traced to its point
of use, every documented command and endpoint checked against the code, and
the audio, cache, HTTP, and MQTT subsystems read for bugs. Detailed evidence
is in the `findings-*.md` files; the fix/defer split with rationale is in
`triage.md`. Everything below was verified against the code, not guessed.

## Fixed in this review (21 items)

**MQTT and commands**
- **Reconnect deafness (critical):** the daemon subscribed once at startup
  with `clean_session=true`; after any broker restart or network blip it
  reconnected but never resubscribed, silently ignoring all commands until
  restarted. It now resubscribes on every ConnAck — covered by a regression
  test that actually restarts a broker.
- `mqtt.client_id` and `mqtt.reconnect_delay_seconds` were parsed but ignored
  (random id, hardcoded 5 s). Now honored.
- A username without a password (or vice versa) was silently dropped; now
  logged.
- Macros were silently discarded for the legacy nested `message` format; they
  now merge into the `message` object. Unknown macro names are now logged
  instead of ignored.
- The legacy `fadeout` / `soundFadeOut` commands promised by the CHANGELOG
  didn't exist; they're now aliases of `fadeall`.

**Playback engine**
- A pitch-corrected sample went **completely silent** the moment its
  streaming download finished (dispatch checked "complete" one way, the mix
  path another). It now keeps playing.
- Reverse playback at fractional speeds interpolated against the wrong
  neighbor with inverted weights — audibly garbled. Fixed.
- Streaming samples could be killed mid-playback by loader lock contention,
  or truncated when playback caught up with a slow download; samples created
  at the wrong instant got an empty channel map and played silence forever.
  All three lifetime bugs fixed.
- Ducking fades ran N× too fast when a voice had N samples, restore time was
  hardcoded to 2 s regardless of the rule's fade, and unrelated voice
  activity restarted fades into an asymptotic crawl. All fixed.
- `seek` clamped to however much of a stream had downloaded (or to frame 0
  under lock contention); it now clamps to the track length like `play`'s
  `start_position_ms`.

**Settings that did nothing, now wired**
- `audio.buffer_size` (validated, documented, never applied) now sets the
  stream buffer size, clamped to the device's range with a safe fallback.
- `logging.verbose` in a config file now actually raises the log level.
- `security.allowed_directories` was a documented security feature with **no
  implementation at all** — any file readable by the daemon could be played
  by anyone with MQTT/HTTP access. A non-empty list is now enforced
  (canonicalized, symlinks resolved, `../` blocked), with tests.
- HTTP `/input/mute` treated an omitted `mute` field as `false` and silently
  unmuted; the field is now required, matching the MQTT command.

**Robustness**
- HTTP fetches had no timeouts; one wedged server could stall a load — and
  with it the command loop — forever. Connect/header/body-stall timeouts
  added.
- A URL whose download or decode failed once was poisoned until restart
  (every later `play` joined the dead load). Failed loads are now evicted and
  retried.

**Documentation** (corrected to match reality)
- Removed: nonexistent `--buffer-size` flag, nonexistent environment
  variables, the wrong issue tracker (it pointed at an unrelated repo!).
- Corrected: HTTP port default (0/auto, not 8080), input `latency_ms` default
  (20, not 25), `stop` fade default (10 ms, not 0), volume ranges (0.0–4.0),
  cache revalidation and disk-persistence promises, "playing files are never
  evicted", mic-triggered ducking examples in three docs (mics can be ducked
  but cannot trigger ducking), README/getting-started clone commands, Windows
  prerequisites, example-config notes (input volume range, config filename,
  client_id format).
- Added: `/input/volume` + `/input/mute` endpoints, the `internal_id`
  selector, `soundFadeAll`/`fadeout` aliases, `--http-port`/`--max-cache-mb`
  in the CLI reference, a note that HTTP command responses are
  fire-and-forget acknowledgments, and an honest description of `/ws`.

## Deferred — needs a decision (30 items, see triage.md for detail)

The ones most worth a decision soon:

1. **Empty `allowed_directories` semantics (D1).** Docs used to promise
   "empty = HTTP-only", but the README quick start and every default-config
   setup relies on local playback. Enforcement now applies only to non-empty
   lists; docs say so. Decide whether empty should eventually mean deny-local.
2. **`cache.enabled` and `revalidate_after_seconds` (D2, D3)** are still
   accepted-but-inert (docs now say "reserved"). Implement or remove.
3. **Streamed HTTP audio is never written to disk cache and never enters the
   memory-cache budget (D4, D5).** Every URL play re-downloads after restart,
   `max_memory_mb` doesn't bound streamed loads, and completed loads live in
   `active_loads` forever — the real memory-growth story on long-running
   installs, together with the VoiceManager/DuckingEngine per-playback leaks
   (D10).
4. **Command-loop serialization (D6).** A slow load delays `stopall`; the
   cache mutex is held across awaits (a `/status` poll during a stall can
   even block the audio callback). Timeouts now bound the damage; the
   architecture fix (async mutex / per-load tasks) is real work.
5. **WebSocket log streaming (D8)** was advertised but never wired into
   tracing — no client ever received a log line. Docs now say so. Wire it up
   (and add auth to `/ws`) or drop the endpoint.
6. **Mic-triggered ducking (D7)** needs signal-level detection to exist; docs
   no longer claim it.
7. **HTTP responses are fire-and-forget (D9)** — clients can't learn a
   command failed. Needs an API design pass.

The rest (D11–D30) are audio-quality subtleties (loop-crossfade double-play,
resampler tail loss, RT-safety violations in the callback, decode aborts on
one corrupt packet, volume-change pops), cache-layer hardening (unstable
filename hashing, non-atomic writes, progress estimates), and smaller polish
(IPv6 bind, constant-time token compare, `speed: 0` semantics, dev-tool
paths, lib.rs duplication).

## Verification

- `cargo build --release`: clean, zero warnings.
- Full test suite: 520 tests green (472 lib + 26 http-api + 19 streaming +
  1 broker-restart + 2 doc-carried), including new tests for the resubscribe
  fix, macro/nested-message expansion, fadeout aliases, failed-load retry,
  and allowed-directories enforcement (traversal + symlink cases).
- The broker-restart and failed-load tests were verified to fail without
  their fixes.
