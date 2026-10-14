# Command ordering

How the daemon keeps commands in order while plays load in the background.

**Status:** proposed design, under adversarial review. The daemon does not
implement it yet; until it does, the race described below is live (see
`docs/bugs.md`).

## The problem

A `play` prepares its audio (open, probe, decode, download) outside the command
loop, so a slow load never delays `stopall` or commands for other sounds. Before
this design, nothing ordered the commands that arrived while a load was in
flight. A `speed` sent right after the `play` it was meant for ran first, matched
nothing, and the play started afterwards at normal speed. A `voice_stop` sent
right after a looping `play` left the loop running indefinitely. With messages
under ~300 µs apart, this happened most of the time (see `docs/bugs.md` for the
measurements).

## The guarantee

1. Commands that touch the same voice take effect in the order they arrived.
2. A command that touches a play which has not started yet waits for that play
   to start (or fail), then takes effect.
3. A play never starts before an earlier command that would have touched it,
   had the play been sent first.
4. A command that touches nothing that is still waiting takes effect
   immediately. A slow load only delays commands for the voices it touches.
5. `stopall` and `fadeall` take effect immediately. They cancel every play that
   has not started. Commands that were waiting run first, in arrival order,
   then the stop.

The outcome is the same as processing commands one at a time in arrival order,
except that unrelated commands do not wait for slow loads.

"Arrival" means the order in which the command loop receives commands: MQTT
messages in broker delivery order and HTTP requests in the order their handlers
submit them, interleaved through one channel.

## What a command touches

Every command that can affect a sound is described by three sets, computed when
it arrives.

- **Voices**: the voices whose sounds it can affect.
- **Criteria**: for commands that select sounds, the `voice:`, `id:` and `file:`
  values of their selector. A sound matches when any one of them is equal, the
  same OR rule `SampleSelector::matches` uses.
- **Identity**: for plays, the `voice:`, `id:` and `file:` of the sound the play
  will create.

| Command | Voices | Criteria | Identity |
|---|---|---|---|
| `play` | its voice | - | its voice, id, file |
| `voice_stop`, `voice_fade_out`, `voice_volume` | the named voice | the named voice | - |
| `seek`, `speed`, `stop`, `volume` | voices of started sounds the selector matches, plus voices of unstarted plays it matches | selector `voice`, `id`, `file` | - |

A play without a `voice` gets its automatic voice name when it arrives rather
than when it starts. The sound then has the same voice before and after it
starts, so commands that reach it by `id` or `file` order against each other
through that voice.

`internal_id` only names sounds that have already started. Internal ids are
assigned when a sound starts, so no client can name an unstarted play by one.
A selector's `internal_id` therefore contributes the voice of the started sound
it matches and nothing else.

## When a command waits

A later command L waits for an earlier unfinished entry E when either holds:

- **Shared voice**: E and L touch a common voice.
- **E would reach L**: L is a play and E's criteria match L's identity. Without
  this rule, a waiting `stop {file: F}` could be overtaken by a play of `F` in an
  unrelated voice, and would then stop a sound that was sent after it.

An entry is unfinished while it is a play that has not started (still loading,
or loaded and waiting its turn), or a command waiting its turn.

When an entry finishes, the waiting entries are re-examined in arrival order,
and each one with no remaining blocker runs. A play's load starts on arrival
regardless; only its start waits.

Examples:

- `play V` (loading), then `speed {voice V}`: the speed waits and applies to
  the new sound once it starts.
- `play V` (loading), then `volume {voice W}`: different voices, so the volume
  runs at once.
- `play V` (loading), `voice_stop V`, `play V` (cached): the second play waits
  for the stop, which waits for the first play. The first sound starts and is
  stopped, and the second keeps playing.
- Existing sound in voice W with file F, then `play F` into V (loading), then
  `stop {file: F}`: the stop touches W (started sound) and V (unstarted play).
  It waits for the play and then stops both.
- `stop {file: F}` waiting behind a load in V, then `play F` into W: the play
  waits for the stop (E would reach L), so it is not stopped by a command sent
  before it.

## Plays in the same voice

Two plays into the same voice start in arrival order. A cached play sent right
after a slow play into the same voice waits for the slow one, so sounds sent
together into one voice start in the order sent, as close together as the slower
load allows, at the cost of the later one's latency. Plays in different voices
never wait for each other, even for the same file or id.

## Failures and cancellation

- **Load fails** (missing file, HTTP error, decode error): the play finishes as
  failed, its error is reported, and the commands waiting on it run against
  whatever is playing.
- **Load task panics**: treated as a failed load. The task id maps back to the
  play, so nothing waits forever.
- **HTTP client gives up on a play** (timeout or disconnect): the loaded play is
  discarded instead of started, as before, and its waiting commands run.
- **HTTP client gives up on a waiting command**: the command still runs when its
  turn comes. A stop that timed out on the client side still stops the sound.
- **`stopall` / `fadeall`**: in-flight loads are aborted as before, every
  unstarted play is cancelled (HTTP 409), waiting commands run in arrival order,
  and then the stop runs. A waiting command whose only target was a cancelled
  play matches nothing and reports that.
- **Too many waiting**: at most 256 entries may be waiting, meaning they had an
  earlier blocker when they arrived (a play with no blocker only loads, and loads
  are already capped at 32). A command or play that would have to wait while 256
  are waiting is rejected with "Too many commands are waiting for earlier
  plays". Commands that need not wait are unaffected. The bound keeps a burst of
  released commands inside the 1024-entry audio command ring.
- **Shutdown**: waiting commands are dropped.

## Not ordered

- **`precache`, `cache_clear`, `cache_invalidate`, `cache_reload`** run as
  background tasks and do not take part. Ordering them against plays would
  need to order the start of a load, not the start of a sound, so it is a
  separate problem (see `docs/bugs.md`).
- **`input_volume`, `input_mute` and the talkback commands** act on live inputs.
  On the audio thread they set an input's own volume and mute, which are
  independent of the voice volume `voice_volume` sets, so the result does not
  depend on their order.
- **Malformed commands** (parse errors, empty selectors) fail at once.

## Implementation

- `src/sequencing.rs` (library, pure logic): `Sequencer<T>` keeps the unfinished
  entries in arrival order with their voices, criteria and identity, decides
  whether an arriving entry must wait, and yields entries as they become
  runnable. It has no I/O, so it is covered by unit and property tests.
- `src/main.rs`: a `CommandScheduler` owns the sequencer and the load tasks
  (`JoinSet`, mapping task ids to entries). The command loop and the test
  fixture both drive it, so the tests exercise the code the daemon runs. It
  computes each command's voices from the live sound map (`playing`) and the
  sequencer's unstarted plays.
- `handle_command` stays the single place a command takes effect.

## Testing

- Sequencer unit tests for each rule, and a property test over random command
  streams and random load completion orders. It checks that conflicting
  entries take effect in arrival order, that nothing waits forever, and that
  an entry with nothing to wait for runs at once.
- Command-loop tests through `CommandScheduler`, using real WAV files and a real
  in-test HTTP server whose responses are held until the test releases them.
  That makes "still loading" deterministic without mocking the load path.
- The MQTT reproduction from `docs/bugs.md` re-run against the built daemon.
