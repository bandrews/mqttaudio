# Caching

mqttaudio automatically caches audio files for fast playback. HTTP files are downloaded once and reused. Files are decoded and kept in memory for instant access, with streaming support for large files.

## Cache Layers

### Memory Cache

Decoded audio (PCM) is kept in RAM for instant playback:
- First play: decode file → store in memory (streaming for large files)
- Subsequent plays: instant playback from memory
- Cleared when mqttaudio exits
- LRU eviction when memory limit is reached

### Disk Cache

Downloaded files are stored on disk:
- HTTP files are saved to the cache directory
- Persists across restarts
- Validated against the server using ETag/Last-Modified headers

## Streaming Playback

For large files or slow HTTP connections, playback begins before the entire file is loaded:
- Playback starts as soon as enough audio is buffered (~50ms worth)
- Background task continues loading the rest
- Unloaded sections return silence (rare in practice)
- Seeking to unloaded regions waits for data

## Latency

| Scenario | Typical Latency |
|----------|-----------------|
| Cached in memory (hot) | ~100 ns |
| Cold start (local file) | 6-65 ms |
| Cold start (HTTP) | ~10 ms |
| First HTTP download (full) | 100-300 ms |

## Precaching

### On Startup

Configure files to cache when mqttaudio starts:

```json
{
  "cache": {
    "precache": [
      "/sounds/startup.wav",
      "https://example.com/common-effect.mp3"
    ]
  }
}
```

### Via MQTT

Precache files before you need them:

```json
{
  "command": "precache",
  "file": "https://example.com/large-file.wav"
}
```

This downloads and decodes the file without playing it.

## Cache Commands

### Clear All Cache

Remove all cached files (memory and disk):

```json
{"command": "cache_clear"}
```

### Invalidate Specific File

Force re-download of a specific file:

```json
{
  "command": "cache_invalidate",
  "file": "https://example.com/updated-file.wav"
}
```

## Configuration

```json
{
  "cache": {
    "directory": "~/.mqttaudio/cache",
    "precache": [],
    "max_memory_mb": 512
  }
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `directory` | `~/.mqttaudio/cache` | Disk cache location |
| `precache` | `[]` | Files to cache on startup |
| `max_memory_mb` | `512` | Memory cache limit in MB (0 = unlimited) |

### CLI Option

You can also set the memory limit via command line:
```bash
./mqttaudio --server localhost --topic audio/commands --max-cache-mb 1024
```

### LRU Eviction

When the memory cache reaches its limit, least-recently-used entries are evicted:
- Recently accessed files stay in cache
- Currently playing files are never evicted
- New files trigger eviction of old entries

## Disk Cache Location

Default location: `~/.mqttaudio/cache/`

Override in your config file:
```json
{
  "cache": {
    "directory": "/custom/path"
  }
}
```

## HTTP Validation

For HTTP files, mqttaudio checks if cached files are still current:

1. On first download, stores ETag and Last-Modified headers
2. On subsequent access, sends conditional request
3. Server returns 304 (not modified) or new content
4. If file changed, re-downloads automatically

This means:
- If you update a file on your server, mqttaudio will detect it
- You don't need to manually invalidate the cache
- Validation adds minimal overhead (HEAD request)

## Local Files

Local files are cached in memory but not on disk (they're already local):

- First play: decode → store in memory
- Subsequent plays: instant from memory
- No staleness checking (file system is source of truth)

## Memory Usage

Decoded audio uses more memory than compressed files:

| Duration | Stereo 48kHz | Mono 48kHz |
|----------|--------------|------------|
| 30 seconds | ~11 MB | ~6 MB |
| 1 minute | ~23 MB | ~11 MB |
| 5 minutes | ~115 MB | ~57 MB |
| 10 minutes | ~230 MB | ~115 MB |

The default 512 MB limit allows for approximately:
- ~22 minutes of stereo 48kHz audio
- ~44 minutes of mono 48kHz audio

With LRU eviction, least-recently-used files are automatically removed when the limit is reached. Currently-playing files are protected from eviction.

## Tips

1. **Precache critical files** — Add startup sounds and frequently-used effects to the precache list
2. **Use HTTP for large libraries** — Disk cache handles large file collections efficiently
3. **Monitor memory** — Watch mqttaudio memory usage if caching many files
4. **Clear cache after updates** — If you update files on your server, invalidate or clear the cache

## Troubleshooting

**File changes not detected:**
- Make sure your HTTP server sends proper cache headers (ETag or Last-Modified)
- Use `cache_invalidate` to force re-download
- Or `cache_clear` to start fresh

**Cache directory permission errors:**
- Check write permissions on the cache directory
- Specify a different directory in your config file

**Memory usage too high:**
- Clear cache with `cache_clear` command
- Restart mqttaudio to clear memory cache
