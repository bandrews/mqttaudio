# Command ordering

How commands reach a sound whose play is still loading.

## The problem

A `play` prepares its audio (open, probe, decode, download) outside the command
loop, so a slow load never delays `stopall` or commands for other sounds.
Nothing tied the commands that arrived during a load to the sound being loaded.
A `speed` sent right after the `play` it was meant for ran first, matched
nothing, and the play started afterwards at normal speed. A `voice_stop` sent
right after a looping `play` left the loop running indefinitely. With messages
under ~300 µs apart, this happened most of the time (see `docs/bugs.md` for the
measurements).

## The guarantee

1. A command takes effect at once on the sounds already playing. Its setting
   also carries onto every matching play that is still loading, which starts
   with it. Each sound ends up as if the commands had been processed one at a
   time in arrival order, apart from the differences listed below.
2. Commands never wait for loads, and a play never waits for an earlier play to
   start: it starts when its own load finishes. Loads do share four load slots,
   so several slow downloads at once can hold up other loads until they finish
   or give up (30 seconds for a server to answer, 60 seconds without data).
3. `stop`, `voice_stop` and `voice_fade_out` cancel matching plays that have not
   started. The cancelled play is never heard, and an HTTP client waiting on it
   gets 409. `stopall` and `fadeall` cancel every play that has not started.

## Plays that have not started

A play is unstarted from the moment the command loop receives it until it
starts, fails or is cancelled. Its load is started when it arrives, and waits
for a free load slot if all four are busy. While it is unstarted, the loop keeps
a record of its voice (if the play names one), its `id`, its `file`, and the
settings carried onto it.

A command matches an unstarted play the same way it would match the sound the
play creates:

- `seek`, `speed`, `stop` and `volume` use their selector, with the same OR
  rule as `SampleSelector::matches` over `voice`, `id` and `file`. A selector's
  `internal_id` never matches an unstarted play. Internal ids are assigned
  when a sound starts, so no client can know one before then.
- `voice_stop` and `voice_fade_out` match plays that name their voice. A play
  without a `voice` gets its automatic voice when it starts, so a voice command
  cannot name it before then.

## What carries over

| Command | Effect on a matching unstarted play |
|---|---|
| `speed` | The play starts at that speed and pitch mode |
| `volume` | The play starts at that volume |
| `seek` | The play starts at that position |
| `stop`, `voice_stop`, `voice_fade_out` | The play is cancelled |

When several commands of one kind reach the same play, the last one wins. A
later `speed` replaces an earlier one, which is what processing them in order
would leave. Settings of different kinds do not affect each other, so their
relative order does not matter. A carried setting replaces the play's own
`speed`, `pitch_correction`, `volume` or `start_position_ms`.

A carried `seek` into a part of a cold file that has not decoded yet plays
silence until the decode reaches it, as a `seek` sent just after the play
started would. A play's own `start_position_ms` instead holds the start until
that part has decoded (up to the prebuffer deadline).

A `speed` or `seek` that the daemon ignores for a playing sound carries onto
nothing either: a negative `speed` with `pitch_correction`, and a `speed` or
`seek` whose `voice` is playing a windowed source. It logs the same warning
whether it reached a playing sound or only a play still loading.

A command reports success when it reached a playing sound or an unstarted play,
and "not found" only when it reached neither. The log says what a command did
to unstarted plays: `Carried <command> onto <n> play(s) still loading`, and
`Cancelled play of <file>: <reason>` for each play a stop cancelled.

## Voices that are still loading

A voice exists while a sound in it is playing or a play naming it is still
loading. `voice_volume` therefore needs no carrying: it sets the volume of a voice
whose only play is still loading, and the play starts at that volume. A voice
whose last playing sound stops keeps its volume while a play into it is loading.
A voice whose only plays fail or are cancelled ends with the last of them;
`voice_stop` and `voice_fade_out` cancel the loading plays, so the voice ends
with them. `/status/voices` lists a voice held open this way with
`sample_count: 0`, and `active_voices` in `/status` and `/metrics` counts it.

## Differences from one-at-a-time processing

- **Timing.** A command reaches the playing sounds before earlier plays start,
  and plays start in the order their loads finish. A cached play can start
  before an earlier slow one, in any voice.
- **Stops cancel instead of interrupting.** A stop or fade that reaches an
  unstarted play cancels it, instead of starting it and then stopping or fading
  it. A sound that a fade interrupts can still be reached until the fade ends;
  a cancelled play is gone at once. So a command sent after the stop that named
  only that play is answered "not found" (404) instead of reaching the fading
  sound. And when `voice_fade_out` cancels a voice's only play, the voice ends
  at once instead of when the fade would have ended, so a play into that voice
  sent during that time starts at full volume rather than at the voice's volume.
- **A play that fails to load could be reached while it loaded.** It held its
  voice open, and commands that named it succeeded. Processed one at a time, it
  would already have failed: a command naming only that play would get "not
  found", and a `voice_volume` would have found no voice to set. A play into
  that voice that arrives before the load fails starts at the volume set
  meanwhile.
- **HTTP play whose client gave up** (timeout or disconnect): the loaded play
  is discarded instead of started.
- **Windowed plays** play forward only, so a carried `speed` or `seek` is
  ignored when the play starts, with a warning. Processed one at a time, one
  naming the play's voice would be ignored with a similar warning, but one
  naming it by `id` or `file` would be skipped without one. A carried `volume`
  applies, as a `volume` sent after the play started does.
- **A windowed play makes its voice streamed only once it starts.** A `speed`
  or `seek` naming a voice is ignored while a windowed play is playing in it.
  While that play is still loading, the voice is not yet streamed, so the
  command applies to the voice's other sounds and carries onto its full plays.
  Processed one at a time, the windowed play would have started first and the
  whole command would have been ignored. With `mode: auto`, whether a play
  windows is known only once it has loaded.
- **Crossfades across voices are not linked.** A fade of voice A and a play into
  voice B are independent. A cold play into B starts when it has loaded, so
  `precache` the file first if the crossfade must not leave a gap.

## Not affected

- **`precache`, `cache_clear`, `cache_invalidate` and `cache_reload`** run as
  background tasks next to play loads and are not ordered against them (see
  `docs/bugs.md`).
- **`input_volume`, `input_mute` and the talkback commands** act on live inputs
  and never on sample plays.
- **Malformed commands** (parse errors, empty selectors) fail at once.

## Implementation

- `src/pending_plays.rs` (library, pure logic): `PendingPlays` records the
  unstarted plays, answers which of them a command matches, and holds their
  carried settings. It has no I/O, so it is covered by unit and property tests.
- `src/voice.rs`: the voice manager keeps a voice while a play naming it is
  loading, as well as while it has sounds.
- `src/main.rs`: a `CommandScheduler` owns the pending plays and the load tasks.
  It takes each request, starts loads, routes commands to playing sounds and
  pending plays, cancels on stops, and starts plays as their loads finish. The
  command loop and the test fixture both drive it, so the tests run the code
  the daemon runs. A command acts on playing sounds through `handle_command`,
  and a play starts through it; what reaches unstarted plays is recorded in
  `PendingPlays` or cancels them. A play's parsed command and HTTP reply are
  kept in the scheduler; the load task returns only the prepared audio. A stop
  that cancels a play aborts its load.
  `stopall` and `fadeall` replace the load task set, which aborts every load
  and drops any finished but unstarted result.

## Testing

- `PendingPlays` unit tests, and a property test that runs random command
  streams with random load completion orders and failures. It checks the final
  state of every sound (and each voice's volume) against processing the same
  commands one at a time in arrival order, with a failed play counted as a
  sound until its load fails. The reference has its own selector matching, so
  it does not share the matching it checks.
- Command-loop tests through `CommandScheduler`, with real WAV files, the real
  cache and a real in-test HTTP server whose response headers are held until
  the test releases them. That keeps a play deterministically unstarted without
  mocking the load path.
- A generated-script test through `CommandScheduler` with real loads, ending
  each load where the script says. It compares every sound, every named voice's
  volume and every command's outcome with sending the same commands one at a
  time through `handle_command`, a failed play again counted as a sound until
  its load fails.
- Command-loop tests for loads, cancellation, panicking loads, the load limit
  and shutdown.
- The MQTT reproduction in `docs/bugs.md`, run by hand against the release
  build; its results are recorded there.
