# Quality Review Findings: main.rs, MQTT client, commands, CLI, config wiring

Scope: `src/main.rs`, `src/mqtt/*` (client, commands, logger), `src/lib.rs`,
CLI flags, and tracing every `Config` field to its point of use. Line numbers
refer to the tree at the time of writing.

## Critical

### C1. MQTT subscription is permanently lost after any reconnect

`src/mqtt/client.rs:40` sets `clean_session(true)` and the topic is subscribed
exactly once at startup (lines 51-54), before the event loop first polls. On a
connection drop, rumqttc 0.24 auto-reconnects inside `eventloop.poll()`, but
(verified in the vendored crate source, `MqttState::clean()`) only pending
publishes/pubrels are re-queued — subscriptions are never re-sent, and with
`clean_session=true` the broker discards them too. The `ConnAck` arm
(client.rs:79-81) only logs "MQTT connected".

Consequence: broker restart or network blip → daemon logs "MQTT connected",
looks healthy, and silently ignores every subsequent command until the process
is restarted. For a long-running installation daemon this is the most damaging
bug in the codebase.

### C2. `security.allowed_directories` does not exist (duplicate of cache/http C1)

Only consumer is a `#[cfg(test)]` accessor. Playback decodes any local path
unconditionally (`src/cache/mod.rs:129-139`). `docs/configuration.md` promises
"If empty, only HTTP/HTTPS URLs can be played" and "Path traversal (`../`) is
blocked" — neither is true.

## Major

### M1. Startup MQTT "connect" never touches the network; fatal-error path is dead code

`connect_mqtt` (client.rs:24-59) only constructs the client and queues a
subscribe into rumqttc's in-memory request channel; `subscribe().await` cannot
fail for network reasons. So (1) "Subscribed to topic: X" is logged before any
I/O; (2) the fatal path in main.rs:386-394 ("Failed to connect to MQTT broker"
→ exit(1)) can effectively never trigger. A typo'd `--server` yields a daemon
that logs success and then spins logging "MQTT error: ..." every 5 s.

### M2. Macros are silently ignored for the nested `message` format

`expand_macros` (`src/mqtt/commands.rs:373-427`) merges macro parameters into
the top level of the JSON object, but `MqttCommand::get_params`
(commands.rs:28-36) returns only the `message` object when it exists — every
macro-supplied parameter is dropped. `docs/commands.md` says the nested format
"is equivalent to the flattened format". A legacy-format user's
`{"command":"play","message":{...},"macro":"quiet"}` plays at full volume with
no warning.

### M3. `audio.buffer_size` is defined, documented, validated — and never used

`find_output_config` (`src/audio/engine.rs:235,250`) hardcodes
`cpal::BufferSize::Default` in both branches; main.rs never passes the config
value. The buffer size printed at startup (main.rs:258) is the device default,
compounding the confusion.

### M4. `mqtt.client_id` and `mqtt.reconnect_delay_seconds` are unimplemented

`connect_mqtt` unconditionally generates a random id (client.rs:35-36), and the
reconnect delay is hardcoded to 5 s (client.rs:92) — which doesn't even match
the config default of 10. Brokers with per-client-id ACLs or monitoring can't
be satisfied; the delay knob does nothing.

### M5. `cache.enabled` and `cache.revalidate_after_seconds` unimplemented
(duplicates cache/http M4/M5 — see that file.)

### M6. WebSocket log streaming advertised but never emits a line
(duplicates cache/http M1 — the tracing layer is never installed;
main.rs:211-232 installs only the fmt layer and `MqttLogLayer`.)

### M7. Documented environment variables don't exist

`docs/configuration.md` promises `MQTTAUDIO_CONFIG`, `MQTTAUDIO_CACHE_DIR`, and
`RUST_LOG`. There is no `env::var` call anywhere in `src/`, and `RUST_LOG` is
ignored because logging uses `LevelFilter`, not `EnvFilter`. A systemd unit
setting `MQTTAUDIO_CONFIG` silently runs with defaults.

### M8. `--lfe-channel` / `--crossover-frequency` can never activate bass management

`merge_cli_args` (config.rs:604-610) sets the fields but never `enabled=true`,
and there is no CLI way to set `source_channels` (required non-empty when
enabled). The flags work only as overrides on top of a config file that
already fully configures bass management; docs present them as standalone.

### M9. One slow `play` blocks the entire command loop — including `stopall`

The `Play` arm (main.rs:737-739) holds the cache `std::sync::Mutex` across
`get_or_load_streaming(...).await`; uncached URLs use `reqwest::get` with no
timeout. During a stall the serial command loop processes nothing — an
emergency `stopall` queues behind the hang. Local files likewise decode
synchronously inline in the loop. (Same root cause family as cache/http C2.)

## Minor

### m1. `MissingMessage` error text wrong for the flattened format
`{"command":"play"}` produces "Command missing 'message' field"
(commands.rs:348, 439-441); should name the missing parameters.

### m2. `stop` default fade mismatch with docs
Docs say `fade_out_ms` default 0; main.rs:1206 uses `unwrap_or(10)`. The 10 ms
anti-click default is sensible — the doc is wrong.

### m3. Unknown macro names are silently ignored
`expand_macros` (commands.rs:407-418) skips unknown names without logging. A
typo'd macro name plays with none of the preset parameters, no diagnostic.

### m4. `seek` on a still-streaming sample clamps to the wrong bound
The Seek arm (main.rs:1154) clamps to `sample.buffer.frames()`, which for a
streaming buffer is frames-loaded-so-far and can even be 0 when the decoder
holds the write lock (`src/audio/streaming.rs:185-192`) — the seek lands at
frame 0. The Play path uses `total_frames_or_estimate()` (main.rs:844); Seek
should too.

### m5. `internal_id` selector silently never matches non-numeric input
`SampleSelector::matches` (commands.rs:73-79) ignores parse failure;
`"internal_id":"abc"` is indistinguishable from a valid-but-gone id.

### m6. `channel_map` destinations beyond the device channel count silently dropped
Mix-time bounds check (`src/audio/mixer.rs:729`) is correct realtime behavior,
but nothing at command time warns that a route can never sound. Empty
`channel_map: []` also plays silence without warning.

### m7. MQTT username without password (or vice versa) silently ignored
client.rs:43 requires both; supplying one yields an unauthenticated connect
with no warning.

### m8. `--log-topic` in HTTP-only mode silently discards logs
The MQTT log channel is created whenever `logging.mqtt_topic` is set
(main.rs:211) but the publisher is spawned only with an MQTT connection
(main.rs:401-419); the bounded channel fills and `try_send` (logger.rs:68)
drops everything silently.

### m9. Debug-build overflow panic from malicious `start_position_ms`/`position_ms`
main.rs:841 and main.rs:1152 multiply user u64/u32 values without
`checked_mul`; panics in debug builds only.

### m10. CLI docs drift
`docs/configuration.md` CLI section omits `--http-port`, `--max-cache-mb`, and
test flags; `--channels` help text differs between code ("use max available",
accurate — engine caps at 32) and docs ("auto-detect").

### m11. lib.rs module tree is a hand-maintained duplicate that has drifted
`src/lib.rs` omits `mqtt::logger` while the binary's `mqtt/mod.rs` has all
three modules; everything compiles twice and the trees can silently diverge.

## Verified fine

- All 15 documented commands plus legacy aliases (`soundPlay`, `soundStopAll`,
  `soundFadeAll`, `soundPrecache`) parse and dispatch; every parsed Play
  parameter is genuinely used.
- Macro precedence (command > earlier macro > later macro) matches docs for
  the flattened format.
- Volume clamping consistent at MAX_GAIN=4.0 everywhere; `speed` clamped to
  documented ranges, no div-by-zero from `speed: 0`.
- The `.expect(...caught during validation)` calls in main.rs are genuinely
  unreachable given `validate()`.
- No clap short-flag collisions; `--http-port`/`--max-cache-mb` wire through
  correctly.
- Malformed JSON/binary payloads/unknown commands land in logged errors, not
  panics; serde_json rejects NaN so clamp edge unreachable from the wire.
- Mixer channel-map accesses bounds-checked; hostile indices cannot panic the
  audio callback.
