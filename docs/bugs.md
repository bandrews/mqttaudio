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

## `input_mute` discards the configured volume

Unmuting sets the volume to 1.0 rather than restoring what was configured, so
a microphone set to 0.7 comes back at full level, and one boosted to 2.0 comes
back quieter. There is already a code comment noting this in `src/main.rs`.

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
