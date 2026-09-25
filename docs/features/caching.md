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

If its start position is not decoded within `cache.stream_prebuffer_deadline_ms` (300 ms), the play
starts anyway, silent until the decode catches up, or fails with
`No audio decoded before the prebuffer deadline` if nothing at all has decoded. While that first
decode is still running, a `seek` past the decoded part plays silence until the decode catches up,
and looping waits until the end has been decoded.

A `speed` with pitch correction on such a sound changes its speed at once without correcting the
pitch, and a warning says correction is deferred. Correction starts once the finished decode has
entered the memory cache and been swapped in. A file that is not kept (too large for the room left,
or changed or invalidated during the decode) never gets it, and later `speed` commands on that sound
change its pitch without a warning.

A **windowed** play streams: a background thread decodes into a buffer `cache.stream_window_ms` long
(1.5 s by default), and memory stays at that size however long the file is. A windowed play starts
at the beginning, cannot seek or change speed, loops a local file without a crossfade, and plays a URL
only once. If decoding falls behind, the sound fades out quickly and resumes where it left off once
audio arrives. Windowed audio never enters the memory cache.

### How a play is loaded

A play's `mode` is `"auto"`, `"full"` or `"stream"`; leaving it out, or `"auto"`, uses
`cache.load_mode` (default `auto`). In `auto`:

| File | Played in full when | Otherwise |
|------|---------------------|-----------|
| Already in the memory cache, or being loaded in full | Always, sharing that decode | |
| Local file, or URL already in the disk cache | Its decoded size is at most `cache.full_load_max_bytes` (32 MiB), it is at most `cache.full_load_max_seconds` (60 s) long, and it fits in the budget's free room | Windowed, from the file |
| Other URL | Its estimated decoded size is at most `full_load_max_bytes` and fits in the free room | Windowed, as it downloads |
| URL without a `Content-Length` | Never | Windowed, as it downloads |

A file's size and length come from its header. A URL's decoded size, or a file's when its header
gives no length, is estimated from the file size by extension: twice the size for `.wav`, `.wave`,
`.flac`, `.aiff`, `.aif`, `.alac` and `.wv`, 25 times for anything else (including ALAC in `.m4a`).

`full` plays in full whenever the estimate fits in the free room, and `stream` always windows, except
that a file already in memory or being loaded in full is shared unless the play itself says
`"stream"`. A local file that changed on disk since it was loaded is decided afresh (unless its
`freshness` is `pinned`). A windowed play of a local file or of a URL's disk copy can loop; one that
plays as it downloads cannot.

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
in progress. A load whose length is not yet known, such as an Ogg or MP3 URL loading in full, holds
all the room left until it finishes, so other files decided meanwhile play windowed unless they are
already in memory. `GET /metrics` shows the budget (`cache.memory_cap_bytes`), the room left
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

**Downloaded files** are checked in the background with a conditional request (`If-None-Match` /
`If-Modified-Since`); a play never waits for a check. A play of a cached URL uses the copy the cache
has and, when a check is due, starts one:

- `trusting` (the default): once the copy is older than `cache.revalidate_after_seconds` (300 s).
- `dev`: on every play, whatever the age.
- `pinned`: never.

(The mode is the play's `freshness`, else `cache.freshness`.) A background pass every 30 seconds
also checks the downloaded files that are decoded in memory, by the same rules under
`cache.freshness`.

A check waits up to 5 seconds for the server's answer. A `304 Not Modified` restarts the file's age.
A `200` answer is the new version: it is saved over the cached file, its decoded copy leaves the
memory cache, and plays from then on use it (sounds already playing continue with the old one). If
the server cannot be reached, the cached copy stays and the file is not checked again for another
`revalidate_after_seconds`. A server that sends neither `ETag` nor `Last-Modified` cannot answer
`304`, so every check downloads the file again. Checks hold the cache only to start and to record
their result, never while a server is answering.

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

A directory contributes its `.wav`, `.mp3`, `.ogg` and `.flac` files (not subdirectories).

A precache loads a file the way an `auto` play of it would. A file that would play in full is
decoded into the memory cache (and a URL saved to the disk cache). A file that would play windowed
is not decoded, so precaching never fills memory with files too large or long to keep: a local file
is left as it is, and a URL is downloaded into the disk cache, so its plays stream from disk.

With `cache.precache_blocking` on (the default) each entry has finished loading before the daemon
takes commands; off, the daemon starts each load and then takes commands while they finish. At
runtime, the `precache` command starts loading one file or URL and answers once the load has started,
whatever `cache.precache_blocking` says; the load finishes in the background, and a failure is only
logged. A play that arrives meanwhile shares the load.

## Cache commands

| Command | Does |
|---------|------|
| `precache` | Load a file or URL into the caches, as a play of it would ([Precaching](#precaching)) |
| `cache_invalidate` | Drop one file or URL from both caches, and abandon a load of it in progress |
| `cache_reload` | `cache_invalidate`, then `precache` |
| `cache_clear` | Empty the memory cache and delete every file in the disk cache. Loads already in progress still finish and fill the memory cache again; downloads in progress are not saved to disk |

Sounds that are playing keep playing after their file leaves a cache. See
[Commands](../commands.md#cache).

## First-play latency

- **Cached in memory:** the play starts in the next audio block, unless it has to wait for one of
  the four load slots (see below).
- **Cold full play of a local file:** a few milliseconds of decoding before the first block.
- **Windowed play:** waits for `cache.stream_prebuffer_ms` (150 ms) of audio, at most
  `cache.stream_prebuffer_deadline_ms` (300 ms). On a fast network or disk, `50` and `150` start
  sooner; keep the defaults for internet sources.
- **Uncached URL:** one round trip to the server, then as above.
- **Every play** waits for the next audio block: up to 10.7 ms at the default 512-frame buffer and
  48 kHz.

Every play, cached or not, first takes one of four load slots, so four slow loads already running
(for example uncached URLs from a slow server) delay it. Freshness checks and downloads never hold
the cache while they wait on a server.

`GET /metrics` reports `latency.play_to_first_mix_ns`, the time from a loaded play being handed to the
audio thread to its first mixed audio. It leaves out loading, downloading and prebuffering.
