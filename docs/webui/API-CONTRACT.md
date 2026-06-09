# API Contract — mqttaudio daemon (code-verified)

This is the **source-of-truth** surface the web app builds against. It was reverse-engineered from the Rust
code (not the prose docs), with `file:line` citations. Where the daemon's behavior and the human docs
disagreed, the code won and the docs were corrected (see the program's doc-reconciliation work). The web app's
typed API client (Sprint W0) must encode exactly what is here, including the **transport quirks** — they are
the things most likely to silently break a UI.

> Targets `mqttaudio` v2.0.0 on the `streaming-memory-redesign` line. Re-verify citations before relying on a
> line number; treat the **behavior** as the contract, the line number as a pointer.

---

## 1. Transport & message shape

- The web app talks to the **HTTP server only** (DW5). REST endpoints mirror every MQTT command.
- A command is `{"command": "<name>", …params}`. Params may be **flattened** at the top level **or** nested
  under a `message` object. **Nested wins wholesale:** if `message` is present, the flattened top-level keys
  are *ignored entirely* (not merged) — `commands.rs:25-33`, test `:2300`. The typed REST handlers all emit the
  nested form; the raw `/command` endpoint forwards arbitrary JSON verbatim.
- **Command names are case-sensitive exact matches** (`commands.rs:450`). `Play`/`PLAY` → UnknownCommand.
- **Legacy aliases exist for exactly three commands:** `play` ⇄ `soundPlay`, `stopall` ⇄ `soundStopAll`,
  `precache` ⇄ `soundPrecache` (`commands.rs:451/475/508`). No other command has an alias.
- **`MissingMessage` is a coarse gate:** commands that "require params" only check that *something* was sent,
  not that required fields are present; a missing required field surfaces later as a JSON error
  (`commands.rs:36-38`). The raw `POST /command` always returns `200 {success:true}` on **enqueue** — it does
  **not** validate the command or report parse errors to the caller (`handlers.rs:154-175`). The UI cannot use
  the HTTP status to confirm a command was valid.
- **A non-JSON `/command` body returns 400 with the `CommandResponse` JSON shape** (D61, daemon Sprint 14):
  `{"success": false, "error": "Invalid JSON: …"}` — previously this was axum's plaintext rejection, the one
  `/command` error that did not parse like the others. Clients that special-cased the plaintext body should
  read the JSON shape instead.

## 2. Commands (runtime-controllable; fully UI-drivable)

Validation/clamping happens **downstream** in the mixer, not at parse time, unless noted — the parser accepts
out-of-range `volume`/`speed`. Defaults below are the parser/handler defaults.

| Command | Required | Optional (default) | Notes |
|---------|----------|--------------------|-------|
| `play` (`soundPlay`) | `file` | `id`, `volume`(1.0), `voice`(auto), `loop`(false), `crossfade_ms`(0), `fade_in`, `start_position_ms`, `channel_map`, `mode`(auto), `window_ms`, `prebuffer_ms`, `freshness`(trusting), `cacheable`(true) | Only command taking routing/load/window params. See §3, §4. |
| `stop` | one selector | `internal_id`,`id`,`file`,`voice`, `fade_out_ms` | Selector OR-logic (§3). |
| `stopall` (`soundStopAll`) | — | — | Stops everything. |
| `volume` | `volume` + a selector | `internal_id`,`id`,`file`,`voice` | `volume` clamped 0..1 downstream. |
| `seek` | `position_ms` + a selector | `internal_id`,`id`,`file`,`voice` | Unsupported on `mode:stream` (forward-only). |
| `speed` | `speed` + a selector | `internal_id`,`id`,`file`,`voice`, `pitch_correction`(false) | Range: no pitch → −100..100 (negative = reverse); with pitch → 0.05..8.0 (no reverse). Clamped at `mixer.rs:369-379`. |
| `voice_volume` | `voice`, `volume` | — | |
| `voice_fade_out` | `voice`, **`time`** | — | **Wire key is `time` (ms).** Typed REST body uses `time_ms` (§5). |
| `voice_stop` | `voice` | — | |
| `input_volume` | `input`, `volume` | — | `input` is a **string** — index `"0"` or a `voice_id`. A bare number errors. |
| `input_mute` | `input`, **`mute`** | — | `mute` required over MQTT; **defaults false** on the typed REST endpoint (§5). |
| `precache` (`soundPrecache`) | `file` | — | Decode/cache without playing. |
| `cache_clear` | — | — | Clears memory + disk. |
| `cache_invalidate` | `file` | — | Exact key/file string. |
| `cache_reload` | `file` | — | Invalidate + re-precache (fresh + instant). |

## 3. Selector model (`stop`/`volume`/`seek`/`speed`)

Four optional criteria — `internal_id`, `id`, `file`, `voice` — flattened directly into the message (not a
nested `selector` object). Matching is **pure OR with check order `internal_id → id → file → voice`**
(`commands.rs:72-111`): a sample matches if **any** specified criterion matches.

- `internal_id`: sent as a **string**, matched by parsing to `u64` against the system id (`commands.rs:80-86`).
  A non-numeric value silently never matches. Surfaced (already stringified) by `/status/samples`.
- `id`: exact string vs the user-provided play `id`.
- `file`: **exact string equality** vs the play's `file` (no path normalization) — targets *all* samples of
  that file.
- `voice`: exact `voice_id` — targets *all* samples in the voice.
- **Empty selector (all four absent) matches nothing and is a silent no-op** (`commands.rs:114-119`). The UI
  **must warn** when no selector is set.

## 4. `channel_map` (play only)

Array of `{src, dest, gain?}` (`commands.rs:43-51`). `src`/`dest` are each a **ChannelRef**: a JSON number, a
numeric string (`"3"` → index 3), or an alias string resolved against `audio.channel_aliases`
(`config.rs:99-191`). `gain` is an optional per-route `f32`, default `1.0`, clamped `0.0..=8.0` (non-finite →
1.0) at construction (`main.rs:1744-1750`).

- Mixing is **additive summing**: `output[dest] += sample * volume * route_gain` (`mixer.rs:1326`). Fan-out
  (one `src` → many `dest`) and many→one summing both work; summing into one `dest` can **clip** — per-route
  `gain` is the mitigation.
- **Out-of-range `src`/`dest` routes are silently skipped** at mix time (`mixer.rs:1263-1265`).
- **Unknown alias does not error at parse** — it aborts that single play with a logged error and a silent early
  return (`main.rs:1733-1736`). Command returns 200; no sound.
- **Empty `channel_map: []` is valid and produces silence** (no routes). Distinct from *omitted* (omitted =
  default 1:1 over decoded channels, `main.rs:1264`).
- **CAVEAT — per-route `gain` is ignored on `mode:stream` plays.** Only the full-load path calls
  `set_channel_route_gains` (`main.rs:1744-1765`); the streamed path drops `gain` (`main.rs:1247-1278`).
- `channel_map` is **not** on the typed `/play` endpoint — reach it via `/command` (DW10).

## 5. REST endpoints

**Command endpoints** (POST; auth enforced only when a token is configured, or always under `require_auth`):
`/command` (raw JSON — the universal path), `/play`, `/stop`, `/stopall`, `/volume`, `/seek`, `/speed`,
`/voice/volume`, `/voice/fade_out`, `/voice/stop`, `/input/volume`, `/input/mute`, `/cache/clear`,
`/cache/invalidate`, `/cache/reload`, `/precache` (`routes.rs:78-96`).

**Typed-endpoint quirks (must encode in the client):**
- `POST /play` accepts **only** `file`,`id`,`volume`,`voice`,`fade_in`,`start_position_ms`,`loop`(or
  `loop_mode`),`crossfade_ms`. It **silently ignores** `channel_map`,`mode`,`window_ms`,`prebuffer_ms`,
  `freshness`,`cacheable` (`handlers.rs:181-198`). Use `/command` for those (DW10).
- `loop`: MQTT/flattened accepts **only** `loop`; typed `/play` accepts **`loop` or `loop_mode`**
  (`handlers.rs:194`).
- `voice_fade_out`: wire key `time`; **typed `/voice/fade_out` body key is `time_ms`** (`handlers.rs:550`).
- `input_mute`: `mute` required over MQTT; **typed `/input/mute` defaults `mute:false`** (`handlers.rs:631`).

**Status/read endpoints** (open by default; gated under `require_auth`): `/health`, `/version`, `/metrics`,
`/status`, `/status/samples`, `/status/voices`, `/status/cache`, `/status/inputs` (`routes.rs:101-108`).

| Endpoint | Live payload (refresh) |
|----------|------------------------|
| `/health` | `{status,service,version}` — liveness only; slow poll / connection-loss probe. |
| `/version` | `{name,version,git_sha?}` — fetch once. |
| `/status` | counts (`active_samples/inputs/voices`, `output_channels`), `clip_count`, `xruns`, cache mem/disk `{entries,size_bytes}`, `status`,`version`. Poll 1–2 s. |
| `/status/samples` | per-sample `internal_id,id,voice,file,total_frames,total_ms,sample_rate,volume,voice_volume,speed,loop_mode,windowed`. `position`/`position_ms`/`progress_percent` are real **when telemetry is enabled** (Sprint W6, `GET/POST /telemetry`), else `0`. Poll 1–2 s. |
| `/telemetry` | `{enabled}` (Sprint W6). `GET` reads the opt-in flag; `POST {"enabled":bool}` sets it. Off by default. |
| `/status/voices` | per-voice `id,sample_count,volume,ducking_multiplier` (1.0 = not ducked). Poll 1–2 s. |
| `/status/inputs` | per-input `index,voice_id,volume,channels,muted` (`muted = volume==0.0`). Poll 1–2 s. |
| `/status/cache` | memory/disk `{entries,size_bytes,size_mb}`. Poll 2–5 s. |
| `/metrics` | `uptime_seconds`, `clips`, `xruns`, active counts, `output_channels`, `cache{memory_bytes,memory_entries,memory_headroom_bytes,memory_cap_bytes,disk_bytes}` (`null` cap = unlimited), `ducking` map. Poll 1 s. Richest read surface. |

**Label hygiene:** `clips` and `xruns` are **cumulative** counters — compute rates by diffing successive polls.
`xruns` counts cpal **fatal stream-error/rebuild** callbacks (`engine.rs:437-442`), not per-buffer underruns;
label it "stream errors / rebuilds".

## 6. WebSocket `/ws` (today: logs only)

`/ws` upgrades and streams **log lines only**: a `{type:"connected",…}` frame then `{type:"log",message}` per
tracing event (`websocket.rs:49-103`); it drops under load (`broadcast` Lagged). It carries **no playback/state
events**. Gated by `require_auth`; the handler itself does no token check. Behind the proxy (DW1) the browser
connects same-origin; bypassing the proxy forces `?token=` in the WS URL (DW9).

## 7. Planned additions (this program — not present today)

These do **not** exist yet; the sprints add them. The client should feature-detect (try/fallback), per DW3/DW6.

- **`GET /config`** (Sprint W8, DW11): read-only running config, secrets redacted.
- **Live sample position** — *landed in Sprint W6.* Real `position`/`position_ms`/`progress_percent` on
  `/status/samples` when telemetry is opted-in via `GET/POST /telemetry` (DW3); `0` when off. The over-the-WS
  delivery (tick frames) is still Sprint W7.
- **Meters** (Sprint W7): output peak/RMS + per-input capture level atomics, exposed via the state channel
  (and/or `/status/meters`).
- **State-event WebSocket channel** (Sprint W7, DW12): a second channel of typed events
  (`play/stop/seek/voice_volume/input_mute/ducking/sample_finished`) + throttled (~15–20 Hz) tick frames
  (position/meters). Same gating.

## 8. Runtime-controllable vs config-only (what the UI can *change* vs only *show*)

- **Changeable live** (commands above): all playback/voice/sample/cache state, plus per-input `volume`/`mute`.
- **Config-only — display + emit-snippet + restart (DW8):** ducking rules, bass/LFE crossover, channel
  aliases, per-channel calibration (`channel_volumes`), limiter `output_ceiling_db` + `master_gain`, macros,
  input device definitions, mqtt/http/cache settings. There is **no** runtime config-mutation command and **no**
  hot-reload; config is read once at startup.
