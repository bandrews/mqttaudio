# Streaming Audio Loading & Performance Optimization Plan

## Current Status

| Phase | Status | Notes |
|-------|--------|-------|
| Phase 1: Benchmark Infrastructure | **COMPLETE** | Criterion benchmarks, synthetic audio, HTTP server |
| Quick Win: Configurable Resampler | **COMPLETE** | 4x speedup with Fast default |
| Phase 2: StreamingBuffer Foundation | Pending | Next up |
| Phase 3: Chunked Resampler | Pending | |
| Phase 4: Streaming Decoder | Pending | |
| Phase 5: HTTP Streaming | Pending | |
| Phase 6: Mixer Integration | Pending | |
| Phase 7: Cache Manager Updates | Pending | |
| Phase 8: LRU Eviction | Pending | |
| Phase 9: Seek Support | Pending | |
| Phase 10: Integration & Polish | Pending | |

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

---

## Remaining Implementation Plan

### Phase 2: StreamingBuffer Foundation
**Files:** `src/audio/streaming.rs` (new), `src/audio/types.rs`

Create the core data structures for streaming playback:

```rust
pub enum SampleBuffer {
    Complete(Arc<DecodedBuffer>),           // Cached, immutable
    Streaming(Arc<RwLock<StreamingBuffer>>), // Loading, growing
}

pub struct StreamingBuffer {
    data: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    frames_available: AtomicUsize,  // Updated by loader
    total_frames: Option<usize>,    // Estimated from Content-Length
    state: LoadingState,
    data_available: tokio::sync::Notify,  // For seek waiters
}

pub enum LoadingState {
    Loading,
    Complete,
    Error(String),
}
```

**Tasks:**
1. Implement `StreamingBuffer` struct with thread-safe append
2. Implement `SampleBuffer` enum with unified read interface
3. Add atomic frame counting with Acquire/Release ordering
4. Add `Notify` for seek waiters
5. Unit tests for append/read correctness under contention

**Key insight:** Audio callback uses `try_read()` on RwLock - never blocks, falls back to silence on rare contention.

### Phase 3: Chunked Resampler
**Files:** `src/audio/streaming_resampler.rs` (new)

Wrap Rubato for incremental chunk processing:

```rust
pub struct ChunkedResampler {
    resampler: SincFixedIn<f32>,
    input_buffer: Vec<Vec<f32>>,  // Per-channel accumulator
    chunk_size: usize,            // e.g., 1024 frames
    latency_samples: usize,       // ~256 for our settings
}

impl ChunkedResampler {
    pub fn push(&mut self, samples: &[f32], channels: usize) -> Option<Vec<f32>>;
    pub fn flush(&mut self) -> Vec<f32>;  // Get remaining samples at end
}
```

**Tasks:**
1. Wrap Rubato's `SincFixedIn` for chunk processing
2. Accumulate input until chunk_size reached
3. Handle resampler latency (~256 samples for Fast quality)
4. Handle de-interleave/re-interleave per chunk
5. Verify output matches full-file resampling (bit-exact not required, but close)

**Note:** Rubato already handles state across calls - proven in existing `input.rs` code.

### Phase 4: Streaming Decoder
**Files:** `src/audio/decoder.rs` (modify), `src/audio/streaming_decoder.rs` (new)

Create an iterator-based decoder that yields chunks:

```rust
pub struct StreamingDecoder<R: Read> {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    resampler: Option<ChunkedResampler>,
    target_sample_rate: u32,
}

impl StreamingDecoder {
    pub fn new(reader: R, target_rate: u32, quality: ResamplerQuality) -> Result<Self, DecodeError>;
    pub fn channels(&self) -> usize;
    pub fn sample_rate(&self) -> u32;
    pub fn estimated_frames(&self) -> Option<usize>;
}

impl Iterator for StreamingDecoder {
    type Item = Result<Vec<f32>, DecodeError>;  // Chunk of interleaved samples
}
```

**Tasks:**
1. Extract packet-decode loop from `decode_file()` into reusable iterator
2. Create `StreamingDecoder` that yields decoded+resampled chunks
3. Integrate with `ChunkedResampler`
4. Handle format probing (Symphonia needs some data before it knows the format)
5. Expose channel count and sample rate after probing

### Phase 5: HTTP Streaming Integration
**Files:** `src/cache/disk.rs` (modify), `src/cache/streaming.rs` (new)

Stream HTTP downloads directly to decoder:

```rust
pub struct HttpStreamReader {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>>>>,
    buffer: BytesMut,
    content_length: Option<u64>,
}

impl Read for HttpStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}
```

**Tasks:**
1. Use reqwest streaming with `bytes_stream()`
2. Create buffered adapter implementing `std::io::Read` for Symphonia
3. Estimate duration from Content-Length header
4. Handle chunked transfer encoding (no Content-Length) gracefully
5. Handle network errors mid-stream

### Phase 6: Mixer Integration
**Files:** `src/audio/mixer.rs`

Update mixer to handle both complete and streaming buffers:

```rust
pub struct ActiveSample {
    pub id: u64,
    pub voice: String,
    pub buffer: SampleBuffer,  // Changed from Arc<DecodedBuffer>
    pub position: usize,
    pub volume: f32,
    pub voice_volume: f32,
    // ... rest unchanged
}
```

**Tasks:**
1. Modify `ActiveSample` to accept `SampleBuffer`
2. Add `try_read()` path in `mix_audio()` for streaming buffers
3. Handle partial data: mix what's available, output silence for the rest
4. Track `waiting_for_data` state for logging/debugging
5. **Critical:** No allocations in audio callback - pre-allocate any needed buffers

**Minimum buffer before playback:**
- 48kHz, 512-frame callbacks = ~10.7ms per callback
- Safe minimum: **2560 frames (~53ms)** before starting playback
- Provides ~5 callback buffers of headroom

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

### Phase 8: LRU Eviction
**Files:** `src/cache/memory.rs`

Add memory management with LRU eviction:

```rust
pub struct MemoryCacheEntry {
    buffer: Arc<DecodedBuffer>,
    last_access: Instant,
    size_bytes: usize,
}

pub struct MemoryCache {
    entries: HashMap<String, MemoryCacheEntry>,
    max_size_bytes: usize,
    current_size_bytes: usize,
    playing: HashSet<String>,  // Never evict these
}
```

**Tasks:**
1. Add access timestamps to cache entries
2. Track currently-playing samples (never evict)
3. Add configurable memory limit (`cache.max_memory_mb` in config)
4. Evict LRU entries when limit exceeded
5. Never evict mid-load streaming buffers
6. Add CLI option `--max-cache-mb`

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

**Tasks:**
1. Feature flag `--features streaming` (default on) for easy rollback
2. Update precache to await full load (don't change semantics)
3. Add config option for startup precache blocking
4. Update README documentation with streaming behavior
5. Run full benchmark suite, verify <100ms target
6. Stress test with concurrent loads

---

## Critical Files to Modify

| File | Changes |
|------|---------|
| `src/audio/mixer.rs` | SampleBuffer enum, try_read() in mix_audio |
| `src/audio/types.rs` | StreamingBuffer struct |
| `src/audio/decoder.rs` | Extract streaming decode iterator |
| `src/audio/resampler.rs` | Add chunk-based variant |
| `src/cache/mod.rs` | Streaming load orchestration |
| `src/cache/memory.rs` | LRU eviction, access tracking |
| `src/cache/disk.rs` | HTTP streaming download |
| `src/main.rs` | Accept SampleBuffer in play handler |

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

## Benchmark Targets

| Metric | Target | Current (with Fast resampler) |
|--------|--------|-------------------------------|
| Cold start (local file, no resample) | <100ms | ~12ms for 1 min |
| Cold start (local file, with resample) | <100ms | ~120ms for 1 min |
| Cold start (HTTP, local) | <200ms | Not measured yet |
| Hot load (cache hit) | <5ms | <0.01ms |
| Memory overhead | <10% vs blocking | 0% (same final size) |
| Audio callback (streaming) | <500us | ~50-100us (complete) |

---

## Notes for Implementers

1. **RwLock try_read() is key** - Audio callback must never block
2. **Rubato chunk size** - Use 1024 samples, matches existing input.rs
3. **AtomicUsize for frames_available** - Use Acquire/Release ordering
4. **Don't break existing tests** - Run `cargo test` frequently
5. **Memory cache key** - Keep using file path/URL as key
6. **Precache = full load** - Don't change precache semantics
7. **ActiveSample.buffer** - Change type from `Arc<DecodedBuffer>` to `SampleBuffer`

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
