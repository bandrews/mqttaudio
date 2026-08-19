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

## Update: decisions made and implemented

After this review, the deferred items below were resolved with Ben and the
big ones implemented in a four-sprint program (see `sprint-plan.md` and the
"Resolved after the review" section of `triage.md`): real `cache.enabled`,
HTTP revalidation, stream write-through + memory budgeting, the command-loop
restructure with real HTTP outcomes and load cancellation, mic-triggered
ducking, WebSocket log streaming with auth, env vars, mic resampler quality,
the voice/ducking leak cleanup, `speed: 0` rejection, and in-place CHANGELOG
corrections.

## Still deferred (see triage.md for detail)

The engineering backlog was completed in a follow-up pass (triage.md,
"Completed in the backlog pass"): loop-crossfade timing, resampler
alignment, RT-callback allocations, corrupt-packet resilience, volume-change
ramps, bass warnings, IPv6/auth hardening, streamed disk writes with
download cancellation, honest progress estimates, channel_map over HTTP,
the lib/bin module unification, dev-tool paths, and route warnings.

D20 closed the list: the `--lfe-channel`/`--crossover-frequency` CLI flags
were removed (they could never activate bass management alone, and a
self-activation default would wrongly feed every zone into the sub); bass
management is configured entirely in the `bass_management` config section.
Nothing from the review remains open beyond the known limitations recorded
in `docs/bugs.md`. D1's empty-list semantics stay as decided (empty =
unrestricted, documented).

## Verification

- `cargo build --release`: clean, zero warnings.
- Full test suite: 520 tests green (472 lib + 26 http-api + 19 streaming +
  1 broker-restart + 2 doc-carried), including new tests for the resubscribe
  fix, macro/nested-message expansion, fadeout aliases, failed-load retry,
  and allowed-directories enforcement (traversal + symlink cases).
- The broker-restart and failed-load tests were verified to fail without
  their fixes.
