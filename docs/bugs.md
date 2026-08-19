# Known Bugs and Rough Edges

Issues noticed while working on other things. Each entry says what is wrong and
why it was left alone, so a later change can pick it up deliberately.

## No `fadeall` command

`stopall` exists; there is no fade-out equivalent. `voice_fade_out` can only
fade one voice at a time, so bringing an entire room down gently means sending
one command per voice.

Reported by a user. Out of scope for the microphone routing work.

## `channel_names` and `channel_aliases` are easy to confuse

`audio.channel_names` maps index to name and is display-only.
`audio.channel_aliases` maps name to index and is what routing actually
resolves against. A config that names its channels in `channel_names` and then
uses those names as a `dest_channel` fails validation with
"Unknown channel alias", with nothing pointing at the real cause.

Either the two should merge, or the validation error should say that the name
exists in `channel_names` but not in `channel_aliases`.

## Two input entries cannot share one capture device

Each `inputs` entry opens its own capture stream. Two entries naming the same
ALSA `hw:` device will not both open it, so several microphones on one
interface must share a single entry, and therefore share one volume, one
`voice_id` and one ducking behaviour.

Supporting it means fanning one capture stream out to several `LiveInput`
readers rather than the single-producer/single-consumer ring buffer used today.
Documented as a limitation in `docs/features/microphone-input.md` instead.

## Input volume cannot exceed 1.0

`inputs[].volume` and the `input_volume` command both clamp to 1.0, so a quiet
microphone cannot be boosted in software - only attenuated. The same limit
applies to bass management gain, which a user has already asked about.

## `input_mute` discards the configured volume

Unmuting sets the volume to 1.0 rather than restoring what was configured, so
a microphone set to 0.7 comes back at full level. There is already a code
comment noting this in `src/main.rs`.

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
