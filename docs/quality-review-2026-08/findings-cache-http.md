# Quality Review Findings: Cache and HTTP Subsystem

Scope: `src/cache/*` (mod, disk, memory, http_stream), `src/http/*` (mod,
routes, handlers, websocket), cross-checked against `src/config.rs`,
`docs/http-api.md`, and `docs/features/caching.md`. Verified against code as of
this review; line numbers refer to the tree at the time of writing.

## Critical

### C1. `security.allowed_directories` is not enforced anywhere

`src/config.rs:199-212` defines `SecurityConfig { allowed_directories }`, and
the accessor `allowed_directories()` is `#[cfg(test)]` only. No production code
reads the setting. The play path (`src/main.rs` play handling) passes the
client-supplied `file` string straight into
`cache_mgr.get_or_load_streaming(&file, ...)`, which for non-HTTP paths calls
`decoder::decode_file` (`src/cache/mod.rs:129-139`) with no path check.

The feature is documented as real in five places (`docs/configuration.md`,
`docs/troubleshooting.md`, `docs/getting-started.md`, `README.md` — "Directories
allowed for local file access", "path traversal attempts are blocked").

Consequence: anyone who can publish to the MQTT topic or POST to `/play` can
play (and probe for the existence of) any file the daemon can read, regardless
of configuration.

### C2. One stalled HTTP download wedges the command loop; a `/status` poll then silences audio

Neither HTTP client has a timeout: `reqwest::get(url)` at
`src/cache/http_stream.rs:287` and `src/cache/disk.rs:251` use the default
client, and the reader's `wait_for_data` (`http_stream.rs:129-150`) waits on a
Condvar with no timeout. The command loop holds the CacheManager's
`std::sync::Mutex` across the await (`src/main.rs:737-739`).

Scenario: `play` names a URL on a server that accepts the TCP connection but
never sends headers. `start_http_stream` awaits forever while the cache mutex
is held; every subsequent command queues forever. Worse, `handle_status`
(`src/http/handlers.rs:599-602`) locks `mixer_state`, then `voice_manager`,
then blocks on `cache_manager` — one `/status` poll during the stall pins the
mixer mutex, the audio callback blocks on it, and output goes permanently
silent. (Distinct from, and worse than, the mixer-lock note already in
`docs/bugs.md`.)

## Major

### M1. WebSocket log stream is dead — tracing layer never installed

`WebSocketLogLayer::new` (`src/http/websocket.rs:136-143`) is called nowhere.
The `LogBroadcaster` is created privately inside `start_server`
(`src/http/mod.rs:46`) and never returned, so `main.rs` cannot wire it into
`tracing_subscriber`. Yet `main.rs` logs "WebSocket logs available at
ws://{}/ws" and `docs/http-api.md` documents `/ws` for real-time log streaming.
Clients get the welcome message, then permanent silence.

### M2. Streaming loads never cleaned up: failed URLs poisoned until restart; `max_memory_mb` not honored for streamed HTTP audio

`cleanup_completed_loads` (`src/cache/mod.rs:292-322`) is called only from
tests. Consequences:

1. `get_or_load_streaming` returns any existing `active_loads` entry
   unconditionally (`mod.rs:100-103`), including one whose buffer hit
   `mark_error`. Errored buffers report `is_complete() == false`
   (`src/audio/streaming.rs:125-131`), so even the (uncalled) cleanup would
   never remove them. After one transient network/decode failure mid-download,
   every future `play` of that URL gets the dead errored buffer until restart.
2. Completed loads keep their entire decoded PCM in `active_loads` forever and
   are never promoted into `MemoryCache`, so `cache.max_memory_mb` does not
   apply to any HTTP file played through the streaming path. Memory grows
   unbounded with distinct URLs played.

### M3. Streamed HTTP audio is never written to the disk cache

`get_or_load_streaming` on a disk-cache miss goes straight to
`start_streaming_load` (`src/cache/mod.rs:124-127`); nothing persists bytes
(`disk.rs:315`: "Note: This does NOT cache to disk"). The runtime `precache`
command also uses `precache_streaming`, so it never produces a disk entry
either. Only startup precache with `precache_blocking=true` reaches disk.
`docs/features/caching.md` promises "downloaded once, persists across
restarts" — false for everything played via `play` with a URL.

### M4. HTTP revalidation (`revalidate_after_seconds`, ETag/Last-Modified) unimplemented

`config.rs` parses `revalidate_after_seconds` (default 300); nothing else
references it. `DiskCache` stores `etag`, `last_modified`, `last_validated`
(`disk.rs:14-30, 263-276, 294-302`) but `is_cached` checks only existence
(`disk.rs:180-187`); no conditional request is ever issued.
`docs/features/caching.md` describes a full If-None-Match/304 flow that does
not exist. Stale files are served forever until manual `cache_invalidate`.

### M5. `cache.enabled` is parsed but ignored

`config.rs:170` defines it (default true); `main.rs` constructs the
`CacheManager` unconditionally. `"cache": {"enabled": false}` changes nothing.

### M6. HTTP command endpoints always return success

Every handler in `src/http/handlers.rs` (e.g. `handle_play` at 148-154)
reports only whether the command string landed on the mpsc channel. Parse
errors, macro-expansion errors, missing files, decode failures are logged
server-side and never surfaced; with the WebSocket stream dead (M1) there is no
remote way to learn a command failed. Docs never state responses are
fire-and-forget.

## Minor

### m1. `/ws` bypasses `auth_token`

`routes.rs:107-108` adds the route to the un-authenticated router; auth layer
applies only to `command_routes` (routes.rs:101-104). Harmless today because
the stream is dead (M1); must be fixed together with M1.

### m2. `MemoryCache::put` can double-subtract size when replacing an existing key

`src/cache/memory.rs:87-89` subtracts the old entry's size but leaves the entry
in the map; the eviction loop (92-103) can pick that same key and subtract its
size again in `evict_one` (127-129). In release (overflow checks off)
`current_size_bytes` wraps to ~`usize::MAX`, after which every `put` evicts the
whole cache. Latent today (no live caller re-puts an existing key), but a trap
for whoever wires up load promotion (M2).

### m3. "Currently playing files are never evicted" protection is inert

`mark_playing`/`mark_not_playing` (`memory.rs:181-191`) are called only from
tests. Documented as active in `docs/features/caching.md`. Playback itself is
safe (each `ActiveSample` holds its own `Arc`); cost is an unnecessary
re-decode after eviction.

### m4. `bind_address` cannot be IPv6

`http/mod.rs:59` builds `format!("{}:{}", bind_address, port)`; `"::1"` yields
`"::1:8080"`, which fails to parse; server start failure is logged as non-fatal
so the daemon silently runs without HTTP.

### m5. Auth token compare: non-constant-time, query form not URL-decoded

`routes.rs:44-57` uses `==` and matches the raw `token=` value; a token with
URL-encodable characters can never authenticate via the documented query form.
Query tokens also land in proxy/access logs.

### m6. Cache filename hashing and extension detection fragile

`cache_filename_for_url` comment claims SHA256 but uses `DefaultHasher`
(`disk.rs:142-149`), unstable across Rust releases (toolchain upgrade orphans
files; nothing garbage-collects `files/`). URLs with query strings defeat
extension detection (`disk.rs:152-156`) and the Symphonia hint
(`cache/mod.rs:176-179`).

### m7. `download_and_cache` writes non-atomically and buffers whole files in RAM

`disk.rs:279-289` does `response.bytes().await` then `fs::write`. Crash or
disk-full mid-write leaves a truncated file at the final path that existence-only
`is_cached` treats as valid; playback then fails with confusing decode errors.

### m8. Docs/endpoint mismatches

- `docs/http-api.md` says port default 8080; `config.rs` defaults to 0
  (auto-select).
- `/input/volume` and `/input/mute` exist (`routes.rs:81-82`) but are absent
  from the docs endpoint tables.
- HTTP `/input/mute` marks `mute` as `#[serde(default)]`
  (`handlers.rs:567-570`), so `{"input":"mic"}` silently unmutes; the MQTT
  command requires the field. Omitting the field flips state instead of erroring.
- Streaming `total_frames` estimate is `content_length / 4`
  (`cache/mod.rs:155-159`) — bytes of *encoded* data, so `/status/samples`
  `total_ms`/`progress_percent` are nonsense for compressed HTTP streams.

### m9. Dropping an `HttpStreamReader` doesn't cancel its download

`cancel()` is never called outside tests; on early decoder abort
(`cache/mod.rs:210-216`) the spawned task keeps downloading the full file into
a buffer nobody reads.

## Verified fine

- Auth middleware covers every command route; CORS wraps outermost so
  preflight succeeds with auth enabled.
- `/command` flat and nested formats both accepted.
- All documented endpoints exist with matching methods.
- Memory-cache LRU core logic correct and tested; `max_memory_mb=0` means
  unlimited.
- 404 URLs do not poison `active_loads` (status check precedes insert).
- `HttpStreamReader` Read/Seek/Condvar protocol correct, including forward-seek
  blocking and EOF/error propagation.
- `websocket_enabled`, `cors_permissive`, `bind_address` (IPv4), `port`,
  `max_memory_mb` (for the non-streaming path), `precache_blocking` honored.
- Manually deleted cache files trigger re-download, not decode errors.
