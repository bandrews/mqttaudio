# Architecture

How mqttaudio is put together, for contributors. For behavior and configuration, see the
[user documentation](../README.md#documentation).

## Overview

mqttaudio is one process with two halves:

- **The control plane** runs on a tokio runtime. It receives commands (MQTT and HTTP), loads and
  decodes audio, keeps the voice registry, the ducking engine and the caches, and serves status.
  It may allocate, lock and block.
- **The audio thread** is the output device's callback. It owns all mixer state and does nothing
  that can block: no allocation, no contended lock, no I/O, no logging.

The two halves talk through lock-free single-producer/single-consumer rings (the `ringbuf` crate)
and atomics, with two exceptions: a cold full-load play's progressive buffer is an
`Arc<RwLock<StreamingBuffer>>` shared with its decoder, which the audio thread only `try_read`s, and the
output callback's own state sits behind a mutex that only the callback locks. The control plane never
reads mixer state. It keeps its own view of what is playing, updated from what it sent and from what
the audio thread hands back.

```
 MQTT client ──┐                                         ┌── HTTP status / metrics
 HTTP server ──┤ CommandRequest (JSON + optional reply)  │   (control-side snapshots,
               ▼                                         │    atomics published by audio)
 ┌─────────────────────────────────────────────────────────────────────────────┐
 │ Control loop (src/main.rs)                                                   │
 │  macro expansion → parse → prepare (load task) → handle_command              │
 │  voice registry · ducking engine · status snapshot · reaper tick (20 ms)     │
 └───────────────┬──────────────────────────────────────────────▲──────────────┘
                 │ command ring (AudioCommand)                   │ graveyard rings
                 ▼                                               │ (finished samples,
 ┌─────────────────────────────────────────────────────────────────────────────┐ spent commands)
 │ Audio callback (src/audio/engine.rs → src/audio/mixer.rs)                    │
 │  drain ≤64 commands → mix samples, live inputs, streamed sources            │
 │  → bass management → channel gain × master gain → limiter → device format    │
 └─────────────────────────────────────────────────────────────────────────────┘
        ▲ capture rings (drift-corrected)          ▲ window rings (decoded PCM)
        │                                          │
 capture callbacks (src/audio/input.rs)     streamed-decode threads (src/audio/streamed_source.rs)
```

## Threads and tasks

| What | Where | Notes |
|------|-------|-------|
| Control loop | `main()` in `src/main.rs` | One tokio task: commands, load results, the 20 ms reaper tick, the 30 s cache-freshness tick, shutdown |
| Freshness pass | spawned from the control loop | Revalidates in-memory downloads every 30 s (not when freshness is `pinned`) |
| MQTT event loop | `src/mqtt/client.rs` | Re-subscribes on every reconnect; drops (and counts) commands if the control loop falls behind |
| HTTP server | `src/http/` | axum; command endpoints wait up to 30 s for the control loop's reply |
| Load tasks | `src/loading.rs` | One task per `play`/`precache`/cache command; up to 32 in flight, 4 of them loading at once; `stopall`/`fadeall` abort them |
| Progressive decoders | `src/cache/mod.rs` | Blocking-pool tasks that decode a full-load file into a growing buffer |
| Download tasks | `src/cache/http_stream.rs`, `src/loading.rs` | Async tasks that feed HTTP bodies to decoders, tee them to the disk cache, and register finished downloads |
| Streamed-decode threads | `src/audio/streamed_source.rs` | One dedicated thread per windowed play, filling a bounded ring |
| Output supervisor | `spawn_output_supervisor` in `src/audio/engine.rs` | Owns the output stream; rebuilds it with backoff after a fatal device error, then exits the process if it cannot recover |
| Audio callback | cpal's output thread | Runs `run_mix_callback` |
| Capture callbacks | cpal's input threads | Convert, resample and write into per-input rings |
| State tick | `src/http/mod.rs` | ~15 Hz `/ws/state` broadcaster; idle unless telemetry is on and a client is connected |
| MQTT log publisher | `src/mqtt/logger.rs` | Publishes log records when `logging.mqtt_topic` is set |

## A command's life

1. **Arrive.** MQTT publishes are fire-and-forget. HTTP requests carry a oneshot reply so the
   client learns the real outcome (`CommandRequest` in `src/mqtt/commands.rs`).
2. **Expand and parse.** `expand_macros` merges `macro` presets, then `parse_command` produces an
   `AudioCommand`. Legacy names and the nested `message` format are handled here.
3. **Prepare.** Commands that touch files (`play`, `precache`, cache commands) run
   `loading::prepare` in a load task, so a slow download never delays `stopall` or a volume
   change. `stopall`/`fadeall` bump a generation counter; a load that finishes afterwards is
   discarded and its HTTP caller gets `409`.
4. **Resolve.** `handle_command` does the work that allocates or needs control-side state: voice
   and sample ids, channel-alias resolution, ducking targets, pitch-corrector construction. The
   result is a built `rt_engine::AudioCommand` (for example `AddSample(ActiveSample)`). Commands that
   pick sounds or inputs, such as `stop` or `input_mute`, carry their selector, which the audio
   thread matches against what it is playing.
5. **Apply.** The audio callback drains up to 64 commands at the top of each block and applies
   them to `MixerState`. Commands that own heap memory are moved back to the reaper on the
   command-return ring instead of being freed on the audio thread.
6. **Reap.** When a sample or streamed source finishes, the callback moves it to a graveyard ring.
   The reaper tick drops it off the audio thread, updates the voice registry and status snapshot,
   and asks the ducking engine to restore ducked voices when a primary goes idle.

## Loading paths

`loading::prepare` chooses how a `play` gets its audio (`src/cache/strategy.rs` makes the
full-versus-windowed decision from a header probe, the size/duration thresholds and the memory
budget's headroom):

- **Memory-cache hit.** The decoded buffer is shared by `Arc`; the play starts on the next block.
- **Full load (cold).** A progressive decoder fills a `SampleBuffer::Streaming` in the background
  and the play starts as soon as the requested start position is decoded. Seeking works on the
  progressive buffer, and looping starts once the decode has finished. The finished decode is then
  promoted into the memory cache, if it fits, and the playing sample is switched to the complete
  buffer (`UpgradeSampleBuffer`), which pitch correction needs.
- **Windowed.** A `StreamedSource` plays from a fixed-size ring that a dedicated thread keeps
  filled, so memory stays at the window size however long the asset is. Windowed voices play
  forward only.
- **HTTP.** An uncached URL is opened once. Its `Content-Length` feeds the same decision; the open
  response is then either decoded into a full-load buffer or read through a bounded,
  back-pressured reader for a windowed play. Cacheable downloads are teed to the disk cache as
  they play.

The caches (`src/cache/`) are a memory cache of decoded PCM with a hard byte budget and LRU
eviction, and a disk cache of downloaded files keyed by a SHA-256 of the URL, with ETag /
Last-Modified revalidation.

## The mix

`mix_audio` in `src/audio/mixer.rs` renders one block, in this order:

1. Advance every ducked voice's gain ramp once for the block.
2. Mix active samples (full-load), live inputs and streamed sources into the f32 bus, applying
   sample, voice and ducking gains, fades and channel routes (with per-route gains).
3. Bass management (`src/audio/bass_management.rs`): Linkwitz-Riley crossover, bass from the
   source channels summed into the LFE channel.
4. Per-channel calibration gain × master gain.
5. Replace any non-finite sample with silence.
6. Soft-knee limiter toward the configured ceiling, then a hard clamp at the ceiling.
7. Publish output peak meters (only when telemetry is enabled).

The callback then converts the bus to the device's sample format (`build_output_stream`).

Mixer storage is reserved up front (`MixerState::with_limits`): `audio.max_sounds` samples (256 by
default), `audio.max_streamed_sounds` streamed sources (64) and one slot per configured input. The
control thread checks both sound limits before sending a play (`admit_under_sound_limits`). Its
`playing` map never counts fewer sounds than the mixer holds, because a sound enters it before it is
sent and leaves after the mixer hands it back, so the reserved lists never grow. At the sample limit
the audio thread replaces the oldest non-looping sample; the control thread logs that, and refuses
the play when every sample loops or when the streamed limit is reached. The output callback sizes
its bus on its first block; see [Known issues](bugs.md).

## Live inputs

Each distinct capture configuration (device, latency, channels, rate) opens one stream
(`src/audio/input.rs`). Its callback converts to f32 and feeds an asynchronous resampler whose
ratio is steered from the ring's fill level, which absorbs clock drift between the input and
output devices. Input entries that share a capture stream each get their own ring and appear in
the mixer as separate `LiveInput`s with their own routes, volume and voice id.

Applied input state (volume, mute, health counters) is published through atomics, so
`/status/inputs` reports what the audio thread applied, not what was last requested. The capture
callbacks also publish each channel's peak level, which the reaper tick reads for activity
detection.

## What the control plane can see

The control plane never locks mixer state. HTTP status comes from:

- **The status snapshot** (`http::StatusSnapshot`), rebuilt by the control loop whenever it adds
  or reaps a sample.
- **Atomics written by the audio thread:** the limiter clip counter, first-mix latency probes,
  and, when telemetry is enabled (`POST /telemetry`), each sample's playback position and the output
  peak meters. The stream-error (xrun) counter is bumped by cpal's error callback.
- **The voice registry** (`src/voice.rs`) and the ducking snapshot, both owned by the control
  loop.

## Real-time rules

Code that runs in the audio callback or a capture callback must not:

- allocate or free memory,
- take a lock that another thread can hold for long (the callback's own mutex is uncontended:
  only the output callback locks it),
- do I/O, log, or make blocking system calls.

`tests/alloc_harness.rs` counts allocations and frees on the mix, command and ducking paths and
fails if a steady-state block performs any. It cannot see allocations made inside the C++
time-stretcher used for pitch correction; that stretcher is built on the control thread and
shipped to the audio thread ready to use.

## Source layout

| Path | Contents |
|------|----------|
| `src/main.rs` | CLI, startup, the control loop and command dispatch (`handle_command`) |
| `src/lib.rs` | The library crate the binary, tests and benchmarks build on |
| `src/config.rs` | Config structs, defaults, validation and command-line overrides (`main.rs` applies the environment overrides) |
| `src/loading.rs` | Off-loop preparation of play and cache commands |
| `src/rt_engine.rs` | The control-to-audio command set and rings |
| `src/voice.rs` | Voice registry: voice ids, sample ids, voice volumes |
| `src/talkback.rs` | Talkback lease state machine |
| `src/audio/engine.rs`, `rebuild.rs` | Output device selection, stream building, the mix callback, supervision and rebuild backoff |
| `src/audio/mixer.rs` | `MixerState`, samples, streamed sources, live inputs, `mix_audio` |
| `src/audio/ducking.rs` | Ducking rules (control side) and the gain applier (audio side) |
| `src/audio/bass_management.rs` | LFE crossover |
| `src/audio/input.rs`, `activity.rs` | Capture, drift-corrected resampling, activity detection |
| `src/audio/streaming*.rs`, `streamed_source.rs`, `chunked_resampler.rs` | Progressive and windowed decoding |
| `src/audio/decoder.rs`, `resampler.rs`, `pitch_correction.rs` | One-shot decoding, rate conversion, time-stretching |
| `src/audio/device*.rs`, `alsa_probe.rs` | Device listing, capability probing, output-format choice |
| `src/cache/` | Memory and disk caches, HTTP streaming, the load-strategy decision |
| `src/http/` | REST routes, handlers, WebSocket logs and state ticks |
| `src/mqtt/` | MQTT client, command parsing, MQTT log publishing |
| `src/config_editor/` | The `--configure` terminal editor |
| `webui/` | The React web control app ([docs/webui](webui/README.md)) |

## Dependencies

| Crate | Purpose |
|-------|---------|
| cpal | Audio device I/O (CoreAudio, ALSA, WASAPI) |
| symphonia | Decoding |
| rubato | Sample-rate conversion |
| signalsmith-stretch | Time-stretching for pitch-corrected speed changes (a C++ library; building it needs libclang for bindgen and a C++ compiler) |
| ringbuf | Lock-free rings between threads |
| rumqttc | MQTT client |
| reqwest | HTTP downloads |
| axum, tower-http | HTTP server |
| tokio | Async runtime |
| ratatui | Terminal UI for `--configure` |
| serde, serde_json | Config and command parsing |
| clap | Command-line parsing |
| tracing, tracing-subscriber | Logging |
| parking_lot | Non-poisoning mutexes |
| sysinfo | Available-memory detection for the cache budget |
| sha2 | Disk-cache file names |
| chrono | Disk-cache timestamps |
| dirs | Config and home directory paths |
| rand | Random MQTT client ids |
| futures, futures-util, bytes | HTTP body streaming |
| alsa, libc (Linux) | Device capability probing and capture-open diagnostics |
