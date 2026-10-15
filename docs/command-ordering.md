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
   time in arrival order.
2. Nothing waits. Commands never wait for loads, and plays never wait for each
   other. A slow or stuck load only delays its own sound.
3. `stop`, `voice_stop` and `voice_fade_out` cancel matching plays that have not
   started. The cancelled play is never heard, and an HTTP client waiting on it
   gets 409. `stopall` and `fadeall` cancel every play that has not started, as
   they always have.

## Plays that have not started

A play is unstarted from the moment the command loop receives it until it
starts, fails or is cancelled. Its load begins immediately. While it is
unstarted, the loop keeps a record of its voice (if the play names one), its
`id`, its `file`, and the settings carried onto it.

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
`voice_stop` and `voice_fade_out` cancel the loading plays, so the voice ends
with them.

## Differences from one-at-a-time processing

- **Timing.** A command reaches the playing sounds before earlier plays start,
  and plays start in the order their loads finish. A cached play can start
  before an earlier slow one, in any voice.
- **Stops cancel instead of interrupting.** A stop or fade that reaches an
  unstarted play cancels it, instead of starting it and then stopping or fading
  it.
- **A play that fails to load still held its voice open while it loaded.** A
  `voice_volume` sent meanwhile set the voice's volume, which stays if another
  play into that voice starts. Processing one at a time, the failed play would
  never have existed, and the `voice_volume` would have found no voice to set.
- **HTTP play whose client gave up** (timeout or disconnect): the loaded play
  is discarded instead of started, as before.
- **Windowed plays** play forward only. A carried `speed` or `seek` is ignored
  with a warning, as the commands are for a streamed voice. A carried `volume`
  applies.
- **Crossfades across voices are not linked.** A fade of voice A and a play into
  voice B are independent. A cold play into B starts when it has loaded, so
  `precache` the file first if the crossfade must not leave a gap.

## Not affected

- **`precache`, `cache_clear`, `cache_invalidate` and `cache_reload`** run as
  background tasks next to play loads and are not ordered against them (see
  `docs/bugs.md`).
- **`input_volume`, `input_mute` and the talkback commands** act on live inputs
  and never on sample plays.
- **Malformed commands** (parse errors, empty selectors) fail at once, as before.

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
  the daemon runs. `handle_command` stays the one place a command takes effect.
  A play's parsed command and HTTP reply stay in the scheduler; the load task
  returns only the prepared audio. A stop that cancels a play aborts its load.
  `stopall` and `fadeall` replace the load task set, which aborts every load
  and drops any finished but unstarted result.

## Testing

- `PendingPlays` unit tests, and a property test that runs random command
  streams with random load completion orders and failures. It checks the final
  state of every sound (and each voice's volume) against processing the same
  commands one at a time in arrival order, with a failed play counted as a
  sound until its load fails.
- Command-loop tests through `CommandScheduler`, with real WAV files, the real
  cache and a real in-test HTTP server whose response headers are held until
  the test releases them. That keeps a play deterministically unstarted without
  mocking the load path.
- Characterization tests of the existing loop behaviour, written before the
  loop is restructured.
- The MQTT reproduction from `docs/bugs.md`, re-run against the built daemon.
