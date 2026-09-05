# Quality Review Findings: Documentation vs. Reality

Scope: README.md, CHANGELOG.md, config.example.json, all of docs/, cross-checked
against src/config.rs, src/main.rs (CLI), src/mqtt/commands.rs. Line numbers
refer to the tree at the time of writing. Items already covered by the other
findings files are noted briefly, not repeated in full.

## Critical

### C1. `security.allowed_directories` unenforced
Documented as a security control in README.md:149-152, configuration.md:391-408
("If empty, only HTTP/HTTPS URLs can be played", "Path traversal is blocked"),
config.example.json, troubleshooting.md, getting-started.md, and CHANGELOG.md
("Canonical path resolution with symlink handling"). None of it exists in code.
See findings-cache-http.md C1 / findings-commands-config.md C2.

### C2. Cache revalidation documented in detail, does not exist
docs/features/caching.md:135-147 describes a conditional-request/304 flow and
promises "you don't need to manually invalidate the cache"; CHANGELOG claims
"ETag/Last-Modified validation". ETags are stored but never sent back; all
fetches are unconditional. See findings-cache-http.md M4.

### C3. Microphone-triggered ducking (live input as `primary_voice`) is not implemented

The flagship example in three docs — ducking.md:142-166 ("whenever the
presenter speaks, music automatically ducks"), microphone-input.md:214-238,
configuration.md:554-591 (escape-room example with `"primary_voice":
"gm_mic"`) — cannot work. `DuckingEngine::notify_voice_active` is called only
when samples start/finish (src/main.rs:874, 657-661); live input setup
(main.rs:548-558) never notifies the engine, and there is no signal-level
activity detection in the mixer. Live inputs can be *ducked* as
`ducked_voices`, but a rule whose `primary_voice` is an input `voice_id` never
fires. The documented setup silently does nothing.

## Major

### M1. `mqtt.client_id` / `reconnect_delay_seconds` dead fields, docs wrong twice
Example claims id auto-generates "mqttaudio_<pid>" — actual code uses a random
suffix, and a configured id is ignored entirely; reconnect wait is hardcoded
5 s vs documented/parsed default 10. See findings-commands-config.md M4.

### M2. CHANGELOG documents `fadeout` / `soundFadeOut` — neither exists
`parse_command` (commands.rs:436-612) accepts `fadeall | soundFadeAll` but not
`fadeout`/`soundFadeOut`; both return UnknownCommand despite CHANGELOG's
"Legacy commands continue to work without changes". Meanwhile `soundFadeAll`,
which does exist, is documented nowhere. The `/legacy` folder CLAUDE.md points
at is absent from the repo, so the original contract cannot be checked.

### M3. troubleshooting.md documents a `--buffer-size` CLI flag that doesn't exist
troubleshooting.md:149-151. clap has no such flag; the process exits with
"unexpected argument". (And the config field it points at is itself dead —
findings-commands-config.md M3.)

### M4. All three documented environment variables unimplemented
`MQTTAUDIO_CONFIG`, `MQTTAUDIO_CACHE_DIR`, `RUST_LOG`
(configuration.md:616-622). No `env::var` in src/; logging uses `LevelFilter`,
not `EnvFilter`. See findings-commands-config.md M7.

### M5. `cache.enabled` dead field
See findings-cache-http.md M5.

### M6. Wrong issue tracker in troubleshooting.md
troubleshooting.md:369 points to `github.com/anthropics/claude-code/issues` —
the wrong project entirely.

### M7. `logging.verbose` in a config file has no effect
config.example.json says it's "Equivalent to level: 'debug'", but log level is
derived solely from `logging.level` (main.rs:201-208); `verbose` works only via
the `-v` CLI path (which sets level during merge).

## Minor

- **Default mismatches:** `inputs[].latency_ms` documented 25, actual 20
  (config.rs:303). `http.port` documented 8080, actual 0/auto (config.rs:419).
- **Undocumented CLI flags:** `--test-tone`, `--file`, `--test-mixer`;
  `--http-port` and `--max-cache-mb` missing from configuration.md's CLI
  reference.
- **configuration.md tables incomplete:** cache table omits `enabled` and
  `revalidate_after_seconds`; mqtt table omits `client_id` and
  `reconnect_delay_seconds`.
- **`internal_id` selector parameter undocumented** for stop/seek/speed/volume
  (commands.rs:196-272), though `/status/samples` exposes the ids.
- **http-api.md endpoint table omits `/input/volume` and `/input/mute`**
  (routes.rs:81-82).
- **`POST /play` does not accept `channel_map`** (handlers.rs:96-113) despite
  README/http-api claiming endpoints mirror all MQTT commands; routing over
  HTTP requires `POST /command`.
- **`/ws` always unauthenticated** — undocumented (see findings-cache-http m1).
- **http-api.md:133-134 says sample volumes are "0.0-1.0"** — stale; range is
  0.0-4.0 (MAX_GAIN).
- **caching.md "playing files are never evicted"** — mechanism exists but has
  zero non-test callers (see findings-cache-http m3).
- **config.example.json contradictions:** input `volume` note says 0.0-1.0
  (validation allows 0.0-4.0); `_usage` says copy to `config.json` but
  auto-discovery looks for `./mqttaudio.json` (config.rs:486); the live
  top-level `inputs` entry opens the default capture device if copied verbatim.
- **Config search path doc is Linux-specific:** on macOS `dirs::config_dir()`
  is `~/Library/Application Support/...`, not `~/.config/...`.
- **Install steps shaky:** README clones `-b refactor` (merged; stale);
  getting-started.md uses placeholder URL `yourusername/mqttaudio`;
  Windows prerequisites contradict between README and getting-started.
- **README vs CHANGELOG disagree on legacy version's language** (C++ vs
  Python); `/legacy` folder referenced by CLAUDE.md does not exist in the repo.
- **CHANGELOG 2.0.0 promises docs that don't exist** (cheat sheet, roadmap,
  performance tuning guide, testing strategy).
- **architecture.md structure listing stale:** omits http/routes.rs,
  http/websocket.rs, audio/device.rs, audio/alsa_probe.rs.

## Inventory

### Config fields

| Field | In code | Documented | Status |
|---|---|---|---|
| mqtt.server / port / topic | yes | yes | OK |
| mqtt.client_id | yes | example only | dead field |
| mqtt.reconnect_delay_seconds | yes | example only | dead field (hardcoded 5s) |
| mqtt.username / password | yes | yes | OK |
| audio.device / sample_rate / channels | yes | yes | OK |
| audio.buffer_size | yes | yes | dead field (device default used) |
| audio.channel_names / channel_aliases / channel_volumes | yes | yes | OK |
| cache.enabled | yes | example only | dead field |
| cache.directory | yes | yes | OK |
| cache.revalidate_after_seconds | yes | example only | dead field |
| cache.precache / precache_blocking / max_memory_mb | yes | yes | OK (max_memory_mb not honored for streamed loads) |
| security.allowed_directories | yes | yes | parsed, never enforced |
| logging.level / mqtt_topic | yes | yes | OK |
| logging.verbose | yes | example only | no effect from config file |
| http.* | yes | yes | port default doc wrong (8080 vs 0) |
| bass_management.* | yes | yes | OK (CLI flags can't activate it) |
| inputs[].* | yes | yes | latency_ms default doc wrong (25 vs 20) |
| ducking_rules[] | yes | yes | mic-as-primary promise not implemented |
| advanced.resampler_quality | yes | yes | OK |
| macros | yes | yes | broken for nested `message` format |

### MQTT commands

| Command | In code | Documented | Status |
|---|---|---|---|
| play / soundPlay | yes | yes | OK |
| stopall / soundStopAll | yes | yes | OK |
| fadeall | yes | yes | OK |
| soundFadeAll | yes | no | undocumented alias |
| fadeout / soundFadeOut | no | CHANGELOG | documented, doesn't exist |
| stop / seek / speed / volume | yes | yes | `internal_id` param undocumented |
| voice_stop / voice_fade_out / voice_volume | yes | yes | OK |
| precache / soundPrecache | yes | yes | OK |
| cache_clear / cache_invalidate | yes | yes | OK |
| input_volume / input_mute | yes | MQTT only | HTTP endpoints undocumented |
