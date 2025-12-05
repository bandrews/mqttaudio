# Streaming Audio Loading & Performance Optimization Plan

## Current Status

| Phase | Status | Notes |
|-------|--------|-------|
| Phase 1: Benchmark Infrastructure | **COMPLETE** | Criterion benchmarks, synthetic audio, HTTP server |
| Quick Win: Configurable Resampler | **COMPLETE** | 4x speedup with Fast default |
| Phase 2: StreamingBuffer Foundation | **COMPLETE** | SampleBuffer enum, mixer integration |
| Phase 3: Chunked Resampler | **COMPLETE** | ChunkedResampler with 14 tests |
| Phase 4: Streaming Decoder | **COMPLETE** | StreamingDecoder with 7 tests |
| Phase 5: HTTP Streaming | **COMPLETE** | HttpStreamReader with 15 unit tests + 4 integration tests |
| Phase 6: Mixer Integration | **COMPLETE** | Done as part of Phase 2 |
| Phase 7: Cache Manager Updates | **COMPLETE** | get_or_load_streaming, active load tracking, 8 integration tests |
| Phase 8: LRU Eviction | **COMPLETE** | MemoryCacheEntry with timestamps, configurable limit, playing protection |
| Phase 9: Seek Support | **COMPLETE** | is_frame_loaded(), total_frames_or_estimate(), notifier() for waiting |
| Phase 10: Integration & Polish | **COMPLETE** | Play handler uses get_or_load_streaming, benchmarks verified |

---

## Problem Statement

mqttaudio currently blocks on full file load before playback begins. For a 10-minute file:
1. Download entire HTTP file (if remote)
2. Decode all packets with Symphonia
3. Resample entire file with Rubato
4. Store in memory cache
5. **Only then** can playback begin

This can take seconds for large files. Target: **<100ms cold start latency**.

---

## User Requirements (Confirmed)

1. **Cold start <100ms** - Critical for immersive game triggers
2. **HTTP must stream** - Start playing as soon as safe buffer available
3. **Memory limit configurable** - Multi-GB OK, but don't be wasteful
4. **LRU eviction** - Games proceed in waves, early sounds evictable
5. **Backward seek must work** - Latency OK, functionality required
6. **Precache still works** - Force full load before playback
7. **Blocking startup precache by default** - Can add option later
8. **Synthetic test files** - Generated on-the-fly, ~2 hours in 15-min chunks
9. **Real HTTP testing** - Embedded Rust HTTP server in benchmarks

---

## Completed Work

### Phase 1: Benchmark Infrastructure

**Files created:**
- `benches/loading_benchmark.rs` - Criterion benchmark suite
- `benches/test_support/mod.rs` - Synthetic audio generator and embedded HTTP server
- `tests/perf_exploration.rs` - Quick manual performance tests
- `tests/perf_deep_dive.rs` - Deep dive into resampling bottlenecks

**Key findings:**
- Hot load (cache hit): <0.01ms - already excellent
- Cold load WITHOUT resampling: ~0.2ms per second of audio
- Cold load WITH resampling (old settings): ~4ms per second of audio (~16-20x slower!)
- **Resampling was the dominant bottleneck**, not decoding

### Quick Win: Configurable Resampler Quality

**Root cause:** Resampler was using `sinc_len=256, oversampling_factor=256` which is far more aggressive than needed.

**Solution:** Added configurable `advanced.resampler_quality` setting with presets:

| Preset | sinc_len | oversample | Time (1 min stereo) |
|--------|----------|------------|---------------------|
| `fast` (new default) | 64 | 64 | ~60ms |
| `medium` | 128 | 128 | ~95ms |
| `high` | 256 | 128 | ~190ms |
| `maximum` (old default) | 256 | 256 | ~230ms |

**Files modified:**
- `src/config.rs` - Added `ResamplerQuality` enum, `AdvancedConfig` struct
- `src/audio/resampler.rs` - Accept quality parameter
- `src/audio/decoder.rs` - Pass quality to resampler
- `src/cache/mod.rs` - Store and use resampler quality
- `src/main.rs` - Pass config's quality to CacheManager
- `docs/configuration.md` - Document advanced settings

**Impact:** With Fast quality, a 1-minute 44.1kHz file resampled to 48kHz now takes ~60ms (decode) + ~60ms (resample) = ~120ms total. This is close to the <100ms target for many files.

### Phase 2: StreamingBuffer Foundation

**Files created/modified:**
- `src/audio/streaming.rs` (new) - SampleBuffer enum, StreamingBuffer struct, LoadingState
- `src/audio/mixer.rs` - Changed ActiveSample.buffer to SampleBuffer
- `src/audio/mod.rs` - Added streaming module
- `src/lib.rs` - Exported streaming module
- `src/http/handlers.rs` - Updated to use buffer method calls
- `src/main.rs` - Updated seek handling to use method calls

**Key implementations:**
- `SampleBuffer` enum with `Complete(Arc<DecodedBuffer>)` and `Streaming(Arc<RwLock<StreamingBuffer>>)` variants
- `StreamingBuffer` with thread-safe append/read using AtomicUsize for frame counting
- `get_sample_or_silence()` method for non-blocking audio callback access
- `impl From<Arc<DecodedBuffer>> for SampleBuffer` for backwards compatibility
- ActiveSample constructors accept `impl Into<SampleBuffer>` - existing code unchanged
- Pitch correction automatically falls back to normal mixing for streaming buffers
- 13 unit tests including concurrent read/write contention test

**Phase 6 (Mixer Integration) completed early:** Done as part of Phase 2 since the type change required updating the mixer anyway.

**Future refactoring opportunity:** The mixer (`src/audio/mixer.rs`) is ~2300 lines and too large. Consider extracting:
- `FadeState` to `src/audio/fade.rs`
- `ActiveSample` to `src/audio/active_sample.rs`
- Tests to separate test files

### Phase 3: Chunked Resampler

**Files created:**
- `src/audio/chunked_resampler.rs` (new)

**Key implementations:**
- `ChunkedResampler` struct wrapping Rubato's `SincFixedIn` for incremental processing
- Per-channel input accumulator with configurable chunk size (default 1024 frames)
- `push()` method: accepts interleaved samples, returns output when chunk ready
- `flush()` method: processes remaining samples at end of stream
- Handles de-interleaving/re-interleaving internally
- 14 unit tests including quality comparison with full-file resampler

**API:**
```rust
impl ChunkedResampler {
    pub fn new(input_rate: u32, output_rate: u32, channels: usize, quality: ResamplerQuality) -> Result<Self, Error>;
    pub fn push(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, Error>;
    pub fn flush(&mut self) -> Result<Vec<f32>, Error>;
    pub fn buffered_frames(&self) -> usize;
}
```

### Phase 4: Streaming Decoder

**Files created:**
- `src/audio/streaming_decoder.rs` (new)

**Key implementations:**
- `StreamingDecoder` struct implementing `Iterator` for progressive decoding
- Accepts any `MediaSource` (files, network streams, etc.)
- Integrates `ChunkedResampler` for optional real-time sample rate conversion
- Yields interleaved f32 chunks as decoded
- Exposes metadata: channels, sample rate, estimated frames
- 7 unit tests including comparison with full-file decode

**API:**
```rust
impl StreamingDecoder {
    pub fn new<R: MediaSource>(reader: R, hint: Option<&Hint>, target_rate: Option<u32>, quality: ResamplerQuality) -> Result<Self, Error>;
    pub fn channels(&self) -> usize;
    pub fn sample_rate(&self) -> u32;
    pub fn source_sample_rate(&self) -> u32;
    pub fn estimated_frames(&self) -> Option<u64>;
    pub fn is_finished(&self) -> bool;
}

impl Iterator for StreamingDecoder {
    type Item = Result<Vec<f32>, StreamingDecodeError>;
}
```

### Phase 5: HTTP Streaming Integration

**Files created/modified:**
- `src/cache/http_stream.rs` (new) - HttpStreamReader implementing MediaSource
- `src/cache/disk.rs` - Added `start_streaming_download()` method
- `tests/http_streaming_integration.rs` (new) - End-to-end integration tests

**Key implementations:**
- `HttpStreamReader` struct implementing Symphonia's `MediaSource` trait (Read + Seek)
- Thread-safe shared buffer using `Mutex<SharedBuffer>` and `Condvar` for synchronization
- Background download task using reqwest's `bytes_stream()` for chunked downloads
- `DownloadHandles` for safe communication between download task and reader
- Seeking support within buffered region, with blocking for forward seeks beyond buffer
- Progress tracking via `AtomicUsize` for bytes downloaded
- Cancellation support for aborting downloads
- 15 unit tests covering: basic read/write, seeking, multi-threaded producer/consumer, error handling
- 4 integration tests with embedded HTTP server testing StreamingDecoder integration

**API:**
```rust
impl HttpStreamReader {
    pub fn new(content_length: Option<u64>) -> Self;
    pub fn download_handles(&self) -> DownloadHandles;
    pub fn cancel(&self);
    pub fn is_complete(&self) -> bool;
    pub fn bytes_downloaded(&self) -> usize;
    pub fn progress(&self) -> (usize, Option<usize>);
}

impl MediaSource for HttpStreamReader { ... }
impl Read for HttpStreamReader { ... }
impl Seek for HttpStreamReader { ... }

pub async fn start_http_stream(url: &str) -> Result<HttpStreamReader, HttpStreamError>;
```

---

## Remaining Implementation Plan

### ~~Phase 5: HTTP Streaming Integration~~ — **COMPLETE**
**Files:** `src/cache/disk.rs` (modify), `src/cache/http_stream.rs` (new)

**Goal:** Stream HTTP downloads directly to `StreamingDecoder` so playback can begin
before the full file downloads.

**Challenge:** Symphonia's `MediaSource` trait requires `Read + Seek`. HTTP streams
don't support seeking. Two approaches:

1. **Buffer-based (recommended):** Accumulate downloaded bytes in a growing buffer.
   Implement `Seek` by returning to buffered positions only. Forward seeks beyond
   buffer wait for more data. This is simpler and works with Symphonia unchanged.

2. **Fork Symphonia:** Modify to not require seeking. More complex, maintenance burden.

**Proposed Implementation:**

```rust
/// HTTP stream adapter that implements MediaSource.
/// Buffers downloaded data to support limited seeking within buffered region.
pub struct HttpStreamReader {
    /// Accumulated downloaded bytes
    buffer: Vec<u8>,
    /// Current read position
    position: usize,
    /// Total content length (from header, if known)
    content_length: Option<u64>,
    /// Whether download is complete
    complete: bool,
    /// Channel to receive more bytes from background download task
    receiver: mpsc::Receiver<Bytes>,
    /// Handle to background download task
    download_handle: JoinHandle<Result<(), reqwest::Error>>,
}

impl MediaSource for HttpStreamReader {
    fn is_seekable(&self) -> bool { true }  // Within buffered region
    fn byte_len(&self) -> Option<u64> { self.content_length }
}

impl Read for HttpStreamReader {
    // Read from buffer, block/wait if position beyond buffered data
}

impl Seek for HttpStreamReader {
    // Seek within buffered region OK
    // Forward seek beyond buffer: wait for data
    // Backward seek always OK (data is buffered)
}
```

**Tasks:**
1. Create `HttpStreamReader` implementing `MediaSource` (Read + Seek)
2. Use reqwest streaming with `bytes_stream()` in background task
3. Buffer bytes as they arrive, notify waiters
4. Handle Content-Length for progress estimation
5. Handle chunked transfer encoding (no Content-Length)
6. Handle network errors mid-stream (mark buffer as error state)
7. Update `DiskCache::download_and_cache()` or add streaming variant
8. Write tests with embedded HTTP server (see `benches/test_support/mod.rs`)

**Key Files to Reference:**
- `src/cache/disk.rs` - Current download logic (line ~150, `download_and_cache()`)
- `benches/test_support/mod.rs` - Embedded HTTP server for testing
- `src/audio/streaming_decoder.rs` - Uses `MediaSource` trait

**Testing Strategy:**
- Unit tests for buffer management and seeking
- Integration test with embedded HTTP server
- Test slow downloads (throttled server)
- Test connection failures mid-download

### Phase 6: Mixer Integration — **COMPLETE** (done with Phase 2)

See Phase 2 completion notes above. Key changes:
- `ActiveSample.buffer` changed from `Arc<DecodedBuffer>` to `SampleBuffer`
- `mix_sample_into_output()` uses `get_sample_or_silence()` for streaming safety
- Pitch correction falls back to normal mixing for streaming buffers
- `MIN_BUFFER_FRAMES = 2560` (~53ms at 48kHz) constant defined

### Phase 7: Cache Manager Updates
**Files:** `src/cache/mod.rs`, `src/cache/memory.rs`

Orchestrate streaming loads and cache management:

```rust
impl CacheManager {
    // Existing
    pub async fn get_or_load(&mut self, path: &str, rate: u32) -> Result<Arc<DecodedBuffer>, Error>;

    // New
    pub async fn get_or_load_streaming(&mut self, path: &str, rate: u32) -> Result<SampleBuffer, Error>;
    pub fn is_loading(&self, path: &str) -> bool;
    pub fn loading_progress(&self, path: &str) -> Option<(usize, Option<usize>)>;  // (frames, total)
}
```

**Tasks:**
1. Track both loading (`StreamingBuffer`) and complete (`DecodedBuffer`) buffers
2. Promote streaming→complete on finish (convert to immutable)
3. Start playback after minimum buffer threshold reached
4. Return `SampleBuffer` from new method, keep old method for precache
5. Handle concurrent requests for same file (share single load)

### ~~Phase 8: LRU Eviction~~ — **COMPLETE**
**Files:** `src/cache/memory.rs`, `src/config.rs`, `src/cache/mod.rs`, `src/main.rs`

Added memory management with LRU eviction:

**Implementation:**
- `MemoryCacheEntry` struct with `buffer`, `last_access: Instant`, `size_bytes`
- `MemoryCache.with_max_size(bytes)` constructor for configurable limits
- LRU eviction on `put()` when limit exceeded
- `mark_playing()` / `mark_not_playing()` / `is_playing()` for eviction protection
- `cache.max_memory_mb` config option (default: 512 MB, 0 = unlimited)
- `--max-cache-mb` CLI option
- 11 new unit tests covering eviction behavior

**Tasks completed:**
1. ✅ Add access timestamps to cache entries
2. ✅ Track currently-playing samples (never evict)
3. ✅ Add configurable memory limit (`cache.max_memory_mb` in config)
4. ✅ Evict LRU entries when limit exceeded
5. ✅ Never evict mid-load streaming buffers (via playing set)
6. ✅ Add CLI option `--max-cache-mb`

### Phase 9: Seek Support
**Files:** `src/main.rs`, `src/audio/mixer.rs`

Handle seeking within streaming buffers:

| Seek Type | Behavior |
|-----------|----------|
| Forward to loaded region | Immediate |
| Forward to loading region | Wait via Notify |
| Backward to loaded region | Immediate |
| Backward to unloaded region | Output silence, trigger priority reload |

**Tasks:**
1. Add seek support to `SampleBuffer`
2. Use `Notify` to wake seekers when data becomes available
3. Handle backward seek to unloaded region (rare case)
4. Document precache benefit for seeking-heavy use cases

### Phase 10: Integration & Polish

**Tasks completed:**
1. ~~Feature flag `--features streaming`~~ - Not needed, streaming is stable
2. ✅ Play handler uses `get_or_load_streaming()` for fast startup
3. ✅ MQTT precache command uses `precache_streaming()` (always non-blocking)
4. ✅ Added `cache.precache_blocking` config option (default: true)
5. ✅ Updated configuration documentation with streaming behavior
6. ✅ Benchmarks verified: cold start ~65ms, time to first sample ~62ms
7. ✅ Integration tests for precache/play interaction (5 new tests)

---

## Critical Files to Modify

| File | Status | Changes |
|------|--------|---------|
| `src/audio/streaming.rs` | **DONE** | SampleBuffer enum, StreamingBuffer struct |
| `src/audio/chunked_resampler.rs` | **DONE** | Incremental resampling for streaming |
| `src/audio/streaming_decoder.rs` | **DONE** | Iterator-based decoder with ChunkedResampler |
| `src/audio/mixer.rs` | **DONE** | Uses SampleBuffer, get_sample_or_silence() |
| `src/cache/http_stream.rs` | **DONE** | HttpStreamReader implementing MediaSource |
| `src/cache/disk.rs` | **DONE** | HTTP streaming download via start_streaming_download() |
| `src/cache/mod.rs` | **DONE** | get_or_load_streaming, active load tracking |
| `src/cache/memory.rs` | **DONE** | LRU eviction, access tracking, playing protection |
| `src/main.rs` | **DONE** | Uses SampleBuffer methods |
| `src/http/handlers.rs` | **DONE** | Uses SampleBuffer methods |

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Lock contention in audio callback | `try_read()`, never block, silence fallback |
| Resampler state corruption | Rubato handles state across calls (proven in input.rs) |
| Symphonia network buffering | Wrap HTTP stream in buffered adapter |
| Memory pressure during multi-load | Limit concurrent loads (e.g., max 3) |
| Backward seek performance | Accept latency, document precache benefit |

---

## Benchmark Results

| Metric | Target | Result |
|--------|--------|--------|
| Cold start (30s local file) | <100ms | **~6ms** ✓ |
| Cold start (60s local file) | <100ms | **~14ms** ✓ |
| Cold start (300s local file) | <100ms | **~65ms** ✓ |
| Cold start (30s HTTP) | <200ms | **~10ms** ✓ |
| Time to first sample (300s cold) | <100ms | **~62ms** ✓ |
| Hot load (cache hit) | <5ms | **~99ns** ✓ |
| Precache (60s file) | N/A | ~13ms |
| Precache (300s file) | N/A | ~73ms |
| Precache (900s file) | N/A | ~175ms |

---

## Notes for Implementers

**All implementation complete:**
1. ✅ **RwLock try_read() is key** - Implemented in `get_sample_or_silence()`
2. ✅ **AtomicUsize for frames_available** - Uses Acquire/Release ordering
3. ✅ **Don't break existing tests** - All 370 library tests + 8 integration tests pass
4. ✅ **ActiveSample.buffer** - Changed to `SampleBuffer` with backwards-compatible constructors
5. ✅ **Rubato chunk size** - ChunkedResampler uses 1024 sample chunks
6. ✅ **Memory cache key** - Uses file path/URL as key
7. ✅ **Precache = full load** - Precache uses `get_or_load` for blocking full load
8. ✅ **Play = streaming** - Play handler uses `get_or_load_streaming` for fast startup

**Refactoring opportunity (not blocking):**
- `src/audio/mixer.rs` is ~2300 lines and should be split up
- Extract `FadeState`, `ActiveSample`, and tests to separate modules

---

## Test Commands

```bash
# Run all tests
cargo test --lib --quiet

# Run benchmarks (quick)
cargo bench --bench loading_benchmark -- --quick

# Run performance exploration
cargo test --release --test perf_exploration -- --nocapture

# Run deep dive tests
cargo test --release --test perf_deep_dive -- --nocapture
```
