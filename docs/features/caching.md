# Caching and Large Files

mqttaudio decodes each file to raw audio before playing it. Decoded audio is large, about 23 MB per
minute of stereo at 48 kHz, so the daemon keeps the files it can afford in a memory cache, streams
the ones it cannot through a small window, and keeps downloaded files on disk.

## The two caches

**The memory cache** holds decoded audio. Every play of a file shares one copy, and a file already
there starts at once.

- It has a size budget (see [Memory budget](#memory-budget)). When a new file needs room, the least
  recently used files are dropped, but never one that is playing.
- A file too large for the room left still plays; it just is not kept.
- It is empty at every start. Use [precaching](#precaching) to fill it before the first cue.

**The disk cache** holds downloaded files, so a URL is fetched once and survives restarts. It is on
while `cache.enabled` is `true` (the default) and lives in `cache.directory`:

- `metadata.json` records each URL, its size and the server's `ETag` / `Last-Modified` headers.
- `files/` holds the downloads, named from a hash of the URL (`3f9a1c2e7b40.wav`; extensions other
  than `wav`, `mp3`, `ogg` and `flac` become `.dat`). In-progress downloads end in `.part` or
  `.streaming.tmp`.

The disk cache has no size limit; it grows until `cache_clear` or until you delete it. Local files
are never copied into it.

## Full and windowed plays

A **full** play decodes the whole file into memory. It can seek, loop with a crossfade, play
backwards and change speed with pitch correction. The first play of a file does not wait for the
whole decode: it starts as soon as its start position is decoded and the rest decodes in the
background. When the decode finishes, the file enters the memory cache.

While that first decode is still running, a `seek` past the decoded part plays silence until the
decode catches up, and looping waits until the end has been decoded. Pitch correction starts only
once the finished decode has entered the memory cache and been swapped in; a file too large to keep
plays on without it.

A **windowed** play streams: a background thread decodes into a buffer `cache.stream_window_ms` long
(1.5 s by default), and memory stays at that size however long the file is. A windowed play starts
at the beginning, cannot seek or change speed, loops a local file without a crossfade, and plays a URL
only once. If decoding falls behind, the sound fades out quickly and resumes where it left off once
audio arrives. Windowed audio never enters the memory cache.

### How a play is loaded

With `"mode": "auto"` (the play's `mode`, else `cache.load_mode`):

| File | Played in full when | Otherwise |
|------|---------------------|-----------|
| Already in the memory cache | Always | |
| Local file | Its decoded size is at most `cache.full_load_max_bytes` (32 MiB), it is at most `cache.full_load_max_seconds` (60 s) long, and it fits in the budget's free room | Windowed |
| URL already in the disk cache | Always | |
| Other URL | Its estimated decoded size is at most `full_load_max_bytes` and fits in the free room | Windowed |
| URL without a `Content-Length` | Never | Windowed |

A local file's size and length come from its header. A URL's decoded size is estimated from its
`Content-Length`: twice the download for WAV, FLAC, AIFF and ALAC files, 25 times for anything else.

`"mode": "full"` plays in full whenever the estimate fits in the free room. `"mode": "stream"` is
always windowed, for local files and uncached URLs.

A URL that is already in the disk cache is always decoded in full, whatever its size and mode.
Precache and `cache_reload` also always decode in full. Keep very long remote files out of the disk
cache (with `"cacheable": false`) to avoid decoding them whole; see [Known Issues](../bugs.md).

### How much memory audio needs

Decoded audio takes `seconds × output rate × channels × 4` bytes:

| Length | Mono | Stereo | 5.1 |
|--------|------|--------|-----|
| 1 minute | 11 MB | 23 MB | 69 MB |
| 10 minutes | 115 MB | 230 MB | 690 MB |
| 1 hour | 690 MB | 1.4 GB | 4.1 GB |

(at a 48 kHz output rate)

## Memory budget

The memory cache and full plays share one budget. By default it is 40% of the memory available at
startup, at least 128 MiB and at most 1 GiB. Set a fixed size with `cache.max_memory_mb` (or
`--max-cache-mb`), or use `cache.memory_budget` for other fractions or to remove the limit; see
[Configuration](../configuration.md#cache).

The room a new full play may use is the budget minus the files that are playing and the loads still
in progress. `GET /metrics` shows the budget (`cache.memory_cap_bytes`), the room left
(`cache.memory_headroom_bytes`) and what the cache holds (`cache.memory_bytes`).

The budget covers the memory cache, not the whole process. For a hard limit on everything, run the
daemon under a memory limit such as systemd's `MemoryMax=` (see [Deployment](../deployment.md)).

## Downloads

The first play of a URL opens one request. Its `Content-Length` decides between a full and a
windowed play (above), and the same response is then decoded. The download is written to the disk
cache as it plays, unless the cache is off, the response has no `Content-Length`, or the play has
`"cacheable": false`. It is kept only if it completes with the advertised length, so a windowed play
stopped halfway does not save a partial file.

A download gives up after 10 seconds without a connection, 30 seconds without a response, or 60
seconds without data. Later plays of a URL that is cached on disk read the file from disk without
touching the network, except for [freshness checks](#freshness).

## Freshness

The caches notice changed files like this:

**Local files in the memory cache** are checked on every play: if their size or modification time
changed, they are decoded again. A `pinned` play (the play's `freshness`, else `cache.freshness`)
skips the check. Files not in the memory cache are always read fresh.

**Downloaded files** are checked with a conditional request (`If-None-Match` / `If-Modified-Since`)
once they are older than `cache.revalidate_after_seconds` (300 s):

- A file that is also in the memory cache is checked by a background pass every 30 seconds. In `dev`
  mode that pass checks every such file each time, ignoring the age; in `pinned` mode it does not
  run.
- A file only on disk is checked when it is played, in every mode. The check waits at most 5 seconds.

A `304 Not Modified` restarts the file's age. A `200` downloads the new version, and plays from then
on use it. If the server cannot be reached, the cached copy plays and the file is not checked again for
another `revalidate_after_seconds`. A server that sends neither `ETag` nor `Last-Modified` cannot
answer `304`, so every check downloads the file again.

With `cache.enabled` off nothing is kept on disk, so a URL in the memory cache is never checked;
after it leaves the memory cache, its next play downloads it again.

To pick up a change immediately, send `cache_reload`.

## Precaching

List files that must start instantly in `cache.precache`:

```json
"cache": {
  "precache": ["/opt/sounds/cues", "/opt/sounds/theme.mp3", "https://example.com/intro.wav"]
}
```

A directory contributes its `.wav`, `.mp3`, `.ogg` and `.flac` files (not subdirectories). With
`cache.precache_blocking` on (the default) each entry is fully decoded before the daemon takes
commands; off, the daemon starts each load and then takes commands while they finish.

At runtime, the `precache` command does the same for one file or URL. Precaching always decodes the
whole file, whatever its size, and a file larger than the budget's free room is not kept in memory
afterwards (a URL stays on disk). Precache the files that fit.

## Cache commands

| Command | Does |
|---------|------|
| `precache` | Load a file or URL into the caches |
| `cache_invalidate` | Drop one file or URL from both caches, and abandon a load of it in progress |
| `cache_reload` | `cache_invalidate`, then `precache` |
| `cache_clear` | Empty the memory cache and delete every file in the disk cache. Loads already in progress still finish and fill the cache again |

Sounds that are playing keep playing after their file leaves a cache. See
[Commands](../commands.md#cache).

## First-play latency

- **Cached in memory:** the play starts in the next audio block.
- **Cold full play of a local file:** a few milliseconds of decoding before the first block.
- **Windowed play:** waits for `cache.stream_prebuffer_ms` (150 ms) of audio, at most
  `cache.stream_prebuffer_deadline_ms` (300 ms). On a fast network or disk, `50` and `150` start
  sooner; keep the defaults for internet sources.
- **Uncached URL:** one round trip to the server, then as above.
- **Every play** waits for the next audio block: up to 10.7 ms at the default 512-frame buffer and
  48 kHz.

`GET /metrics` reports `latency.play_to_first_mix_ns`, the time from a play being queued to its first
audio being mixed, so you can measure your own hardware.
