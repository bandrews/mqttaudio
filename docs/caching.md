# Caching Strategy

## Goals

1. **Low latency** - Cached files play instantly
2. **Freshness** - Detect when files change during development
3. **Simplicity** - No complex invalidation logic
4. **Network efficiency** - Don't re-download unchanged files

## Cache Layers

### Layer 1: Memory Cache

**What:** Fully decoded PCM audio in RAM
**Lifetime:** Until mqttaudio exits
**Purpose:** Zero-latency playback for repeated sounds

```rust
HashMap<String, Arc<DecodedBuffer>>
```

**Behavior:**
- First play of a file: Decode → store in memory
- Subsequent plays: Instant playback from memory
- Shared via Arc (zero-copy)

**Eviction:** None in v1.0 (unlimited growth). Files are only removed on exit.

**Future enhancement:** LRU eviction when memory usage exceeds threshold.

### Layer 2: Disk Cache

**What:** Downloaded audio files (original format: WAV, OGG, MP3, etc.)
**Lifetime:** Persists across restarts
**Purpose:** Avoid re-downloading unchanged files

**Structure:**
```
~/.mqttaudio/cache/
  ├── files/
  │   ├── abc123.wav
  │   ├── def456.ogg
  │   └── ...
  └── metadata.json
```

**metadata.json format:**
```json
{
  "http://example.com/sound.wav": {
    "local_file": "abc123.wav",
    "etag": "\"33a64df551425fcc55e4d42a148795d9f25f89d4\"",
    "last_modified": "Wed, 21 Oct 2015 07:28:00 GMT",
    "last_validated": "2025-10-19T10:30:00Z",
    "file_size": 1048576
  },
  "http://example.com/music.mp3": {
    "local_file": "def456.mp3",
    "etag": null,
    "last_modified": "Thu, 15 Aug 2024 14:22:00 GMT",
    "last_validated": "2025-10-19T10:28:00Z",
    "file_size": 5242880
  }
}
```

**Eviction:** Manual only (cache_clear command). No automatic eviction in v1.0.

**Future enhancement:** Size-based eviction, TTL expiration.

## Cache Flow

### First Play (HTTP File)

```
1. Play command: http://example.com/sound.wav
2. Check memory cache → MISS
3. Check disk cache metadata → MISS
4. HTTP GET http://example.com/sound.wav
   - Capture ETag and Last-Modified headers
5. Stream to disk cache: files/abc123.wav
6. Simultaneously begin decoding (streaming decode)
7. Start playback as soon as first chunk decoded
8. Continue decoding in background
9. Store decoded PCM in memory cache
10. Update metadata.json with cache entry
```

**Latency:** ~100-300ms (network dependent)

### Second Play (Cached, Within Revalidation Window)

```
1. Play command: http://example.com/sound.wav
2. Check memory cache → HIT
3. Play immediately from memory
```

**Latency:** < 5ms (instant)

### Third Play (Cached, Outside Revalidation Window)

Config: `revalidate_after_seconds: 300`

```
1. Play command: http://example.com/sound.wav
2. Check memory cache → HIT
3. Check last_validated timestamp → 6 minutes ago (> 300 seconds)
4. Play immediately from memory
5. Async background task:
   - HTTP HEAD http://example.com/sound.wav
   - If-None-Match: <etag>
   - If-Modified-Since: <last_modified>
6. Server responds:
   - 304 Not Modified → Update last_validated, done
   - 200 OK → Mark cache entry as stale
7. Next play will re-download
```

**Latency:** < 5ms (playback doesn't wait for validation)

### Play After Cache Marked Stale

```
1. Play command: http://example.com/sound.wav
2. Check memory cache → HIT but marked stale
3. Remove from memory cache
4. Check disk cache → Entry marked stale
5. HTTP GET http://example.com/sound.wav (re-download)
6. Update disk cache
7. Decode to memory
8. Play
```

**Latency:** ~100-300ms (like first play)

## Cache Validation (HTTP)

### Using ETag

Best method for cache validation. ETag is an opaque identifier for a specific version of a resource.

**Server provides:**
```
ETag: "33a64df551425fcc55e4d42a148795d9f25f89d4"
```

**Client validates:**
```
HEAD /sound.wav HTTP/1.1
If-None-Match: "33a64df551425fcc55e4d42a148795d9f25f89d4"
```

**Server responds:**
- `304 Not Modified` - File unchanged, use cache
- `200 OK` - File changed, new ETag provided

### Using Last-Modified

Fallback if ETag not available. Less reliable (1-second granularity).

**Server provides:**
```
Last-Modified: Wed, 21 Oct 2015 07:28:00 GMT
```

**Client validates:**
```
HEAD /sound.wav HTTP/1.1
If-Modified-Since: Wed, 21 Oct 2015 07:28:00 GMT
```

**Server responds:**
- `304 Not Modified` - File unchanged
- `200 OK` - File changed, new Last-Modified provided

### No Cache Headers

If server provides neither ETag nor Last-Modified:
- Always re-download (cache disabled for this URL)
- Or use file size as weak validator
- Or trust cache based on time only

## Revalidation Timing

Controlled by `cache.revalidate_after_seconds` config:

| Value | Behavior | Use Case |
|-------|----------|----------|
| `0` | Always check server before playing | Development (frequent file changes) |
| `300` | Check after 5 minutes | Balanced (default) |
| `3600` | Check after 1 hour | Stable production |
| `86400` | Check after 24 hours | Very stable content |

**Trade-off:**
- Low value: Fresh content, more network traffic
- High value: Faster playback, may miss updates

## Local File Caching

Local files (file:// or absolute paths) are NOT cached on disk (already local).

**Behavior:**
- First play: Decode → store in memory cache
- Subsequent plays: Use memory cache
- No staleness checking (assume file system is source of truth)

**Future enhancement:** Watch file system for changes (inotify, FSEvents, ReadDirectoryChangesW).

## Cache Commands

### Clear Entire Cache

```json
{"command": "cache_clear"}
```

**Effect:**
- Delete all files in cache directory
- Clear metadata.json
- Clear memory cache
- Next play of any file requires full download/decode

**Use case:** Free disk space, force fresh download of everything

### Invalidate Specific File

```json
{
  "command": "cache_invalidate",
  "message": {
    "file": "http://example.com/updated.wav"
  }
}
```

**Effect:**
- Remove from memory cache
- Mark disk cache entry as stale (or delete it)
- Next play will re-download

**Use case:** During development, file changed on server

## Precache Command

```json
{
  "command": "precache",
  "message": {
    "file": "http://example.com/bigfile.wav"
  }
}
```

**Effect:**
- Download (if not cached)
- Decode to memory
- Do NOT play

**Use case:** Pre-load files before show/experience starts

**Note:** Playing a file has the same caching effect. Precache just avoids playback.

## Cache Storage Format

### File Naming

Cached files use content-addressable names to avoid collisions:

```rust
fn cache_filename(url: &str) -> String {
    let hash = sha256(url);
    let extension = extract_extension(url).unwrap_or("dat");
    format!("{}.{}", hash[..12], extension)
}
```

Example:
- URL: `http://example.com/sounds/thunder.wav`
- Cache file: `a3b5c7d9e1f2.wav`

**Why hash?** URLs can have special characters, be very long, or have same filename but different paths.

### Metadata Storage

**metadata.json** contains all cache entries:

```json
{
  "version": 1,
  "entries": {
    "http://example.com/sound.wav": {
      "local_file": "abc123.wav",
      "etag": "\"33a64df...\"",
      "last_modified": "Wed, 21 Oct 2015 07:28:00 GMT",
      "last_validated": "2025-10-19T10:30:00Z",
      "file_size": 1048576,
      "content_type": "audio/wav"
    }
  }
}
```

Loaded on startup, updated after downloads, saved periodically.

## Error Handling

### Disk Cache Corrupted

If metadata.json is invalid:
- Log warning
- Start with empty cache
- Optionally delete corrupted cache files

### Disk Cache File Missing

If metadata references `abc123.wav` but file doesn't exist:
- Remove entry from metadata
- Re-download on next play

### Network Errors

If validation HEAD request fails:
- Use cached version anyway (graceful degradation)
- Log warning
- Try again on next play

### Disk Full

If cache write fails due to full disk:
- Log error
- Continue without caching
- Audio still plays (streaming decode)

## Performance Characteristics

### Startup Time

- Load metadata.json (O(1), small JSON file)
- No pre-validation of cached files
- Fast startup regardless of cache size

### Memory Usage

Memory cache grows unbounded in v1.0:
- 1 minute of stereo 48kHz f32 PCM ≈ 23 MB
- 10 cached files × 30 seconds ≈ 115 MB
- 100 cached files × 10 seconds ≈ 384 MB

**Future enhancement:** LRU eviction based on memory threshold.

### Disk Usage

Disk cache grows unbounded in v1.0:
- Original file formats (compressed)
- Typical WAV file: 1-10 MB/minute
- Typical OGG file: 100-500 KB/minute
- Typical MP3 file: 1-3 MB/minute

**Future enhancement:** Size-based eviction.

### Network Usage

With `revalidate_after_seconds: 300`:
- HEAD request every 5 minutes per file (tiny, ~200 bytes)
- Full download only when file changes
- Efficient for stable content

## Testing Cache Behavior

### Test 1: First Play
```bash
# Clear cache
echo '{"command": "cache_clear"}' | mosquitto_pub -t audio/commands -l

# Play file (should download)
echo '{"command": "play", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l

# Check cache directory
ls ~/.mqttaudio/cache/files/
cat ~/.mqttaudio/cache/metadata.json
```

### Test 2: Cached Play
```bash
# Play same file again (should be instant)
echo '{"command": "play", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l

# Should be < 10ms latency (check logs)
```

### Test 3: Staleness Detection
```bash
# Update file on server
curl -X POST http://example.com/update-file

# Wait for revalidation window to expire
sleep 310

# Play file (uses cache but triggers async validation)
echo '{"command": "play", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l

# Play again (should re-download)
echo '{"command": "play", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l
```

### Test 4: Manual Invalidation
```bash
# Invalidate specific file
echo '{"command": "cache_invalidate", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l

# Play file (should re-download)
echo '{"command": "play", "message": {"file": "http://example.com/test.wav"}}' | mosquitto_pub -t audio/commands -l
```

## Future Enhancements

### v1.1: Smarter Eviction
- LRU eviction for memory cache (configurable max size)
- Disk cache size limit with LRU eviction
- Per-file TTL support

### v1.2: Better Validation
- File system watching for local files
- Content-based hashing (detect changed files even without ETag)
- Parallel validation of all cached files on startup (optional)

### v1.3: Advanced Features
- Pre-fetch related files (playlist-based)
- Compression of disk cache (store as FLAC instead of WAV)
- Shared cache between multiple mqttaudio instances
