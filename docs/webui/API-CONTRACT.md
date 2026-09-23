# API Contract — mqttaudio daemon (code-verified)

This is the **source-of-truth** surface the web app builds against. It was reverse-engineered from the Rust
code (not the prose docs); where the daemon's behavior and the human docs disagreed, the code won. The web
app's typed API client (Sprint W0) must encode exactly what is here, including the **transport quirks** — they
are the things most likely to silently break a UI.

> Re-verified against `2.1.0-rc.1` plus the fixes under `[Unreleased]` in the changelog (September 2026).
> Function names are given as pointers; treat the **behavior** as the contract. The user-facing reference for
> the same surface is [docs/commands.md](../commands.md) and [docs/http-api.md](../http-api.md).

---

## 1. Transport & message shape

- The web app talks to the **HTTP server only** (DW5). REST endpoints mirror every MQTT command.
- A command is `{"command": "<name>", …params}`. Params may be **flattened** at the top level **or** nested
  under a `message` object. **Nested wins wholesale:** if `message` is present, the flattened top-level keys
  are *ignored entirely* (not merged) — `MqttCommand::get_params`. The typed REST handlers all emit the
  nested form; the raw `/command` endpoint forwards arbitrary JSON verbatim.
- **Macros** (`"macro": "name"` or an array, top level only) are merged before parsing: command params win,
  then earlier macros over later ones (`expand_macros`). An unknown macro name is logged and skipped.
- **Command names are case-sensitive exact matches** (`parse_command`). `Play`/`PLAY` → UnknownCommand.
- **Legacy aliases:** `play` ⇄ `soundPlay`, `stopall` ⇄ `soundStopAll`, `precache` ⇄ `soundPrecache`, and
  `fadeall` ⇄ `soundFadeAll` / `fadeout` / `soundFadeOut`. No other command has an alias.
- **Unknown params are ignored**, so a misspelled key is silently dropped rather than rejected.
- **Every command route reports the real outcome**, `/command` included: it waits (up to 30 s) until the
  control loop has carried the command out and answers `200 {"success":true,"message":"Command completed"}`
  or an error status with `{"success":false,"error":"…"}`: `400` malformed/invalid, `403` forbidden (path
  outside `allowed_directories`, a refused or out-of-range talkback request, unmuting a talkback-held
  input), `404` nothing to act on (missing/undecodable file, failed URL, unmatched selector, empty voice,
  unavailable input, no open talkback microphone), `409` cancelled by a later `stopall`/`fadeall`, `500`
  overloaded (32 loads in flight, full audio queue) or internal, `504` no result within 30 s of queuing.
  After a `504` the daemon drops the command: a play whose load finishes later never starts (cache
  commands still take effect).
- **A non-JSON `/command` body returns 400 with the same JSON shape** (D61). The typed endpoints instead
  answer a body they cannot read with axum's plaintext rejection (`400` bad JSON, `415` missing JSON
  content type, `422` missing/mistyped field).

## 2. Commands (runtime-controllable; fully UI-drivable)

The parser clamps play `volume` to `0..4` and rejects `speed: 0`, a non-numeric `internal_id`, and
`window_ms`/`prebuffer_ms` outside their limits; other clamping happens downstream. Defaults below are the
parser/handler defaults.

| Command | Required | Optional (default) | Notes |
|---------|----------|--------------------|-------|
| `play` (`soundPlay`) | `file` | `id`, `volume`(1.0), `voice`(auto), `loop`(false), `crossfade_ms`(0), `fade_in`, `start_position_ms`, `channel_map`, `mode`(config `load_mode`), `window_ms`(100..60000), `prebuffer_ms`(capped at the window; ≤ `window_ms` when both are sent), `freshness`(config), `cacheable`(true) | Only command taking routing/load/window params. See §3, §4. |
| `stop` | one selector | `internal_id`,`id`,`file`,`voice`, `fade_out_ms`(10) | Selector OR-logic (§3). |
| `stopall` (`soundStopAll`) | — | — | Stops everything (10 ms fade); cancels loads in flight. |
| `fadeall` (`soundFadeAll`,`fadeout`,`soundFadeOut`) | — | `time`(1000; alias `fade_out_ms`) | Fades everything out; cancels loads in flight. |
| `volume` | `volume` + a selector | `internal_id`,`id`,`file`,`voice` | Clamped `0..4`; applies to full and windowed plays. |
| `seek` | `position_ms` + a selector | `internal_id`,`id`,`file`,`voice` | Full plays only; windowed plays ignore it (still `200`). |
| `speed` | `speed` + a selector | `internal_id`,`id`,`file`,`voice`, `pitch_correction`(false) | No pitch → −100..100 (negative = reverse, `|speed|<0.01` → ±0.01); with pitch → 0.05..8.0, negative → `400`. Every speed command sets pitch correction on or off. Full plays only. |
| `voice_volume` | `voice`, `volume` | — | Clamped `0..4`; also reaches a live input with that `voice_id`. `404` if neither exists. |
| `voice_fade_out` | `voice`, **`time`** | — | **Wire key is `time` (ms).** Typed REST body uses `time_ms` (§5). |
| `voice_stop` | `voice` | — | 10 ms fade. `404` if the voice has no sounds. |
| `input_volume` | `input`, `volume` | — | `input` is a **string** — index `"0"` or a `voice_id`. A bare number is a `400` on `/command`/MQTT, `422` on the typed route. Instant, clears mute. |
| `input_mute` | `input`, **`mute`** | — | `mute` required on every path. Instant. Unmuting by `voice_id` is `403` while a talkback lease holds the input (not checked by index, nor for `input_volume`). |
| `precache` (`soundPrecache`) | `file` | — | Completes when loading has started. |
| `cache_clear` | — | — | Clears memory + disk. |
| `cache_invalidate` | `file` | — | Exact key/file string. |
| `cache_reload` | `file` | — | Invalidate + re-precache; completes when loading has started. |
| `talkback_acquire` | `client_id`, `destination`, `gain`, `lease_ms` | `source_id`(`GM_MIC`) | Lease 250..2000 ms, gain −60..12 dB, fixed destination allowlist; refusals and out-of-range values are `403`, and `404` when no `GM_MIC`/`mic` input is open. `gain`/`destination` are validated and echoed in `/status/talkback` but not applied. Renew by re-acquiring. |
| `talkback_release` | `client_id`, `lease_id` | — | `lease_id` from `/status/talkback`. |
| `talkback_hard_mute` | — | — | Ends the current lease and mutes its input. |

## 3. Selector model (`stop`/`volume`/`seek`/`speed`)

Four optional criteria — `internal_id`, `id`, `file`, `voice` — flattened directly into the message (not a
nested `selector` object). Matching is **pure OR** (`SampleSelector::matches`): a sample matches if **any**
specified criterion matches.

- `internal_id`: sent as a **string** of digits, matched against the system id. A non-numeric value is a
  `400`. Surfaced (already stringified) by `/status/samples`.
- `id`: exact string vs the user-provided play `id` (not unique).
- `file`: **exact string equality** vs the play's `file` (no path normalization) — targets *all* samples of
  that file.
- `voice`: exact `voice_id` — targets *all* samples in the voice.
- **Empty selector (all four absent) is a `400`**, and a selector that matches no playing sample is a `404`
  (`handle_command`'s pre-check). The UI should still warn before sending.
- `seek`/`speed` selecting by `voice` are skipped entirely (still `200`) when a windowed sound has played in
  that voice since it was last idle.

## 4. `channel_map` (play only)

Array of `{src, dest, gain?}` (`ChannelMapping`). `src`/`dest` are each a **ChannelRef**: a JSON number, a
numeric string (`"3"` → index 3), or an alias string resolved against `audio.channel_aliases`. `gain` is an
optional per-route `f32`, default `1.0`, clamped `0.0..=8.0` (non-finite → 1.0) at construction
(`channel_route_gains`).

- Mixing is **additive summing**: `output[dest] += sample * volume * route_gain`. Fan-out (one `src` → many
  `dest`) and many→one summing both work; summing into one `dest` can **clip** — per-route `gain` is the
  mitigation.
- **Out-of-range `src`/`dest` routes are silently skipped** at mix time.
- **An unknown alias fails the play with `400`** before it loads (`loading::prepare`).
- **Empty `channel_map: []` is valid and produces silence** (no routes). Distinct from *omitted* (omitted =
  default 1:1 over decoded channels).
- Per-route `gain` applies on both the full-load and the windowed paths.
- `channel_map` is accepted by the typed `/play` endpoint and by `/command`.

## 5. REST endpoints

**Command endpoints** (POST; token required whenever `http.auth_token` is set): `/command` (raw JSON — the
universal path), `/play`, `/stop`, `/stopall`, `/fadeall`, `/volume`, `/seek`, `/speed`, `/voice/volume`,
`/voice/fade_out`, `/voice/stop`, `/input/volume`, `/input/mute`, `/cache/clear`, `/cache/invalidate`,
`/cache/reload`, `/precache`, `/talkback/acquire`, `/talkback/release`, `/talkback/hard-mute`, and
`POST /telemetry` (`create_router`).

**Typed-endpoint quirks (must encode in the client):**
- `POST /play` accepts **only** `file`,`id`,`volume`,`voice`,`fade_in`,`start_position_ms`,`loop`(or
  `loop_mode`),`crossfade_ms`,`channel_map`. It **silently ignores** `mode`,`window_ms`,`prebuffer_ms`,
  `freshness`,`cacheable` (`PlayParams`). Use `/command` for those (DW10).
- `loop`: MQTT/flattened accepts **only** `loop`; typed `/play` accepts **`loop` or `loop_mode`**.
- `voice_fade_out`: wire key `time`; **typed `/voice/fade_out` body key is `time_ms`**.
- `/fadeall` needs a JSON body; send `{}` for the default fade.
- `input_mute`: `mute` is required on the typed `/input/mute` endpoint too (a missing field is a `422`).

**Status/read endpoints** (GET; open unless `require_auth`, except `/health` and `/ready`, which are always
open): `/health`, `/ready`, `/version`, `/metrics`, `/status`, `/status/samples`, `/status/voices`,
`/status/cache`, `/status/inputs`, `/status/talkback`, `/status/meters`, `/config`, `/telemetry`.

| Endpoint | Live payload (refresh) |
|----------|------------------------|
| `/health` | `{status,service,version}` — liveness only; slow poll / connection-loss probe. |
| `/ready` | `200`/`503` `{status,ready,checks{output,inputs},failed_inputs[{voice_id,error}]}` — decided at startup. |
| `/version` | `{name,version,git_sha?}` — fetch once. |
| `/status` | counts (`active_samples/inputs/voices`, `output_channels`), `clip_count`, `xruns`, cache mem/disk `{entries,size_bytes}`, `status`,`version`. Poll 1–2 s. |
| `/status/samples` | per-sample `internal_id,id,voice,file,total_frames,total_ms,sample_rate,volume,voice_volume,speed,loop_mode,windowed`. `position`/`position_ms`/`progress_percent` are real **when telemetry is enabled** (Sprint W6), else `0`; always `0` for windowed samples, whose `total_*` are `0`. Poll 1–2 s. |
| `/telemetry` | `{enabled}` (Sprint W6). `GET` reads the opt-in flag; `POST {"enabled":bool}` sets it (token-gated). Off by default. |
| `/status/voices` | per-voice `id,sample_count,volume,ducking_multiplier`, for voices with sounds (a voice used only by a live input is absent; `/metrics` `ducking` lists every ducked voice). `ducking_multiplier` is the target the voice is at or fading to (1.0 = not ducked). Poll 1–2 s. |
| `/status/inputs` | per-input `index,voice_id,volume,channels,muted,unmuted_volume,ready,last_error,backlog_frames,max_backlog_frames,dropped_frames,trimmed_frames,underrun_frames`; `volume`/`muted`/`unmuted_volume` are the audio thread's applied state. Poll 1–2 s. |
| `/status/talkback` | `{now_ms,talkback{state,applied_live,lease_id,owner_client_id,source_id,destination,gain,lease_expires_at_ms,last_transition,last_error}}` (monotonic ms). `state` follows the lease, not the input: the microphone is live at startup while `state` reads `muted`. |
| `/status/meters` | `{output:[peak…]}` per output channel, post-limiter, linear; zeros when telemetry is off. |
| `/status/cache` | memory/disk `{entries,size_bytes,size_mb}`. Poll 2–5 s. |
| `/metrics` | `uptime_seconds`, `clips`, `xruns`, active counts, `output_channels`, `cache{memory_bytes,memory_entries,memory_headroom_bytes,memory_cap_bytes,disk_bytes}` (`null` cap = unlimited), `ducking` map, `latency{play_to_first_mix_ns{last,max},plays_measured}`, `input_capture{<voice>{…}}`, `pitch_scratch_regrows`. Poll 1 s. Richest read surface. |
| `/config` | the startup config (after CLI/env overrides), `mqtt.password` and `http.auth_token` nulled. Fetch once. |

**Label hygiene:** `clips` and `xruns` are **cumulative** counters — compute rates by diffing successive polls.
`xruns` counts every cpal stream-error callback, including xruns the backend recovered from; only
unrecoverable errors rebuild the stream. Label it "stream errors".

## 6. WebSockets

- **`/ws`** streams **log lines**: a `{type:"connected",message,version}` frame, then `{type:"log",message}`
  per tracing event; no history, and a client more than 1000 lines behind skips lines. It carries **no
  playback/state events**.
- **`/ws/state`** sends `{type:"tick",samples:[{internal_id,position_ms,progress_percent}],meters:{output:[…]}}`
  about every 66 ms, **only while telemetry is on** (and a client is connected); no welcome frame.

Both need the token whenever `http.auth_token` is set. Behind the proxy (DW1) the browser connects
same-origin and the proxy injects it; bypassing the proxy forces `?token=` in the WS URL (DW9).

## 7. Program additions — landed and deferred

Landed: `GET /config` (Sprint W8, DW11), live sample position under the `/telemetry` opt-in (Sprint W6, DW3),
output peak meters plus the `/ws/state` tick channel and the `/status/meters` poll fallback (Sprint W7, DW12).

Deferred (feature-detect if the UI ever uses them): per-input capture-level meters, RMS meters, and discrete
state-event frames (`play/stop/seek/voice_volume/input_mute/ducking/sample_finished`) on `/ws/state` — the
tick frame's sample list carries the live state today.

## 8. Runtime-controllable vs config-only (what the UI can *change* vs only *show*)

- **Changeable live** (commands above): all playback/voice/sample/cache state, per-input `volume`/`mute`,
  talkback leases, and the telemetry opt-in.
- **Config-only — display + emit-snippet + restart (DW8):** ducking rules, bass/LFE crossover, channel
  aliases, per-channel calibration (`channel_volumes`), limiter `output_ceiling_db` + `master_gain`, macros,
  input device definitions, mqtt/http/cache settings. There is **no** runtime config-mutation command and **no**
  hot-reload; config is read once at startup.
