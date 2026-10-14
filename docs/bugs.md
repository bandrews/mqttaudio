# Known Bugs and Rough Edges

Issues noticed while working on other things. Each entry says what is wrong and
why it was left alone, so a later change can pick it up deliberately.

A full quality review lives in `docs/quality-review-2026-08/`: its
`findings-*.md` files hold the verified issues, `triage.md` splits them into
fixed-vs-deferred, and `summary.md` is the short version. The deferred items
there (D1-D30) are the current backlog of known issues beyond this file.

## Two input entries cannot share one capture device

Each `inputs` entry opens its own capture stream. Two entries naming the same
ALSA `hw:` device will not both open it, so several microphones on one
interface must share a single entry, and therefore share one volume, one
`voice_id` and one ducking behaviour.

Supporting it means fanning one capture stream out to several `LiveInput`
readers rather than the single-producer/single-consumer ring buffer used today.
Documented as a limitation in `docs/features/microphone-input.md` instead.

## `fadeall` does not fade live inputs

`fadeall` fades the playing samples, matching what `stopall` stops. A live
microphone keeps running through it, which is usually what you want but is
worth knowing. `input_mute` is the per-microphone control.

## The audio callback locks a std Mutex

`mix_audio` runs under `mixer_state.lock()`, which the HTTP handlers, the
command loop and the input health monitor also take. A non-realtime thread
holding that lock while the audio thread waits on it is a priority inversion
that shows up as an output glitch. It has not caused a reported problem, but it
is the reason to keep every other lock holder short.

## `mqtt::client::tests::test_mqtt_event_processing` needs a live broker

The test publishes to `localhost:1883` and fails without a broker running. It
is a real integration test, not a broken one, but it makes `cargo test` fail on
a machine that has no MQTT server.

## Commands sent right after a `play` can run before it

A `play` loads its file outside the command loop, so `stopall` is never queued
behind a slow load. Nothing orders the commands that arrive while that load is
in flight: a `speed`, `seek`, `volume`, `stop`, `voice_stop`, `voice_fade_out`
or `voice_volume` aimed at the new sample runs immediately, matches nothing,
and the play starts afterwards with none of it applied. A `voice_stop` sent
right after a looping `play` leaves the loop running indefinitely. Two plays can
also start in the opposite order to how they were sent. Only `stopall` and
`fadeall` are covered, by the generation counter that discards loads they
overtake.

Both `main` (the Play arm hands the load to `tokio::spawn`) and `v2.1` (the play
waits in the `loads` JoinSet and re-enters the loop once prepared) have this.
v2.1 also does not log the miss for MQTT senders: the selector check calls
`ctx.fail`, and the loop only reads `ctx.error` when an HTTP reply is waiting.
On `main` it shows up as `Command failed: Speed command matched no active
samples`.

Measured with a local broker and a null ALSA sink, sending `play` then `speed`
for the same voice on one connection, 20 pairs per gap, counting `Set speed`
log lines:

| Gap between messages | `main` dropped | `v2.1` dropped |
|---|---|---|
| 0 µs, cached file | 20/20 | 20/20 |
| 100 µs, cached file | 19/20 | 20/20 |
| 300 µs, cached file | 12/20 | 8/20 |
| 1 ms, cached file | 1/20 | 0/20 |
| 1 ms, first play of a 30 s WAV | 20/20 | 3/20 |
| 5 ms, first play of a 30 s WAV | 20/20 | 0/20 |
| 20 ms, first play of a 30 s WAV | 6/20 | 0/20 |

The window is the load time, so it grows with cold files, HTTP URLs, and
contention for the cache lock. A delay between messages narrows the window
but does not close it. HTTP clients that wait for the `play` response before
sending the next command are ordered correctly, because the response is only
sent once the play has been dispatched.

Left alone because the fix is a decision about ordering semantics, not a local
change. One option is to hold a command whose selector or voice names a play
that is still loading until that play dispatches. Another is to dispatch
prepared plays and everything behind them strictly in arrival order.
