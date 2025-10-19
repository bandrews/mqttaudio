# Performance Requirements and Optimization

## Real-Time Audio Constraints

### The Golden Rule

**The audio callback must complete before the next buffer is needed.**

For a 512-frame buffer at 48kHz:
- Buffer duration: 512 / 48000 = **10.67 milliseconds**
- Safe callback duration: **< 5 milliseconds** (50% headroom)
- Critical callback duration: **< 8 milliseconds** (75% - getting risky)

**If callback takes > 10.67ms → Buffer underrun → Audio glitch**

### Why Headroom Matters

Real-world factors add latency:
- Thread scheduling jitter
- CPU frequency scaling
- Cache misses
- Interrupts from other processes
- Garbage collection (not applicable to Rust, but general principle)

**Conservative target: Callback completes in < 30-40% of buffer duration.**

## Performance Budget

### Per-Sample Processing Cost

For 10 simultaneous samples, 512-frame buffer, stereo output:

| Operation | Cost (μs) | Budget | Notes |
|-----------|-----------|--------|-------|
| Read from buffer | 50 | 10% | Memory bandwidth |
| Apply fade | 100 | 20% | Vec multiply or lookup table |
| Apply volumes | 50 | 10% | Simple multiply |
| Channel mapping | 200 | 40% | Inner loop, most expensive |
| Mix into output | 50 | 10% | Additive blend |
| Saturation | 50 | 10% | Clamp operation |
| **Total** | **500 μs** | **100%** | Target < 5000 μs |

With 10 samples, we have 500 μs × 10 = 5000 μs = **5 ms total** (at budget limit).

### Scaling with Sample Count

| Samples | Est. Time (μs) | Budget Usage | Status |
|---------|----------------|--------------|--------|
| 1 | 500 | 10% | Plenty of room |
| 5 | 2500 | 50% | Comfortable |
| 10 | 5000 | 100% | At limit |
| 20 | 10000 | 200% | **OVERRUN** |
| 50 | 25000 | 500% | **SEVERE OVERRUN** |

**Conclusion:** Set maximum simultaneous samples to **20** with optimizations, **10** conservatively.

## Optimization Strategies

### 1. Pre-Computation

**Fade Envelopes:**

Instead of computing fade per-frame:
```rust
// SLOW - allocates and computes every callback
let fade_mult = calculate_fade_multipliers(&fade, frame_count);
```

Do this:
```rust
// FAST - pre-computed lookup table or inline calculation
for i in 0..frame_count {
    let fade = fade_state.get_multiplier(i); // Inline math or table lookup
}
```

**Channel Calibration:**

Pre-multiply volumes:
```rust
// Instead of:
output[idx] = sample * sample_vol * voice_vol * channel_cal;

// Pre-compute:
let total_vol = sample_vol * voice_vol * channel_cal;
output[idx] = sample * total_vol;
```

### 2. SIMD Optimization

Rust's `std::simd` or explicit SIMD for mixing:

```rust
use std::simd::*;

// Process 4 samples at once (f32x4)
fn mix_simd(source: &[f32], dest: &mut [f32], volume: f32) {
    let vol_vec = f32x4::splat(volume);

    for (src_chunk, dest_chunk) in source.chunks_exact(4).zip(dest.chunks_exact_mut(4)) {
        let src = f32x4::from_slice(src_chunk);
        let dst = f32x4::from_slice(dest_chunk);
        let mixed = dst + src * vol_vec;
        mixed.copy_to_slice(dest_chunk);
    }

    // Handle remainder
    // ...
}
```

**Potential speedup:** 2-4x for mixing operations

### 3. Cache-Friendly Data Layout

**Bad layout:**
```rust
struct ActiveSample {
    buffer: Arc<DecodedBuffer>,
    metadata: SampleMetadata,
    volume: f32,
    // ... scattered data
}
```

**Better layout:**
```rust
// Separate hot and cold data
struct HotSampleData {
    buffer_ptr: *const f32,  // Direct pointer, no Arc deref
    position: usize,
    volume: f32,
    channel_map_ptr: *const [(usize, usize)],
}

struct ColdSampleData {
    id: u64,
    buffer_arc: Arc<DecodedBuffer>, // Keep Arc alive
    metadata: SampleMetadata,
}
```

Access hot data in tight loop, cold data only when needed.

### 4. Minimize Branching

**Instead of:**
```rust
for sample in active_samples {
    if sample.fade_state.is_active() {
        let fade = calculate_fade(sample.fade_state);
        // ...
    } else {
        // ...
    }
}
```

**Do:**
```rust
// Fade always returns 1.0 if not active - no branch
for sample in active_samples {
    let fade = sample.fade_state.get_multiplier(frame);
    // ...
}
```

### 5. Lock-Free State Updates

**Option A: RwLock (simple, might work)**
```rust
let state = mixer_state.read().unwrap();
// Audio callback only reads, low contention
```

**Option B: Triple Buffering (if RwLock contends)**
```rust
struct MixerState {
    buffers: [MixerData; 3],
    read_idx: AtomicUsize,
    write_idx: AtomicUsize,
}

// Audio callback reads from read_idx (no lock)
// Engine writes to write_idx, then atomically swaps
```

**Option C: Lock-Free Queue (most complex)**
```rust
// Engine pushes state updates to queue
// Callback pops updates each iteration
let updates = update_queue.try_pop();
```

## Performance Monitoring

### Built-In Metrics

Track in audio callback:
```rust
struct CallbackMetrics {
    total_calls: AtomicU64,
    total_duration_us: AtomicU64,
    max_duration_us: AtomicU64,
    underruns: AtomicU64,
}

fn audio_callback(...) {
    let start = Instant::now();

    // ... mixing code ...

    let duration = start.elapsed().as_micros();
    metrics.total_duration_us.fetch_add(duration, Ordering::Relaxed);
    metrics.max_duration_us.fetch_max(duration, Ordering::Relaxed);
    metrics.total_calls.fetch_add(1, Ordering::Relaxed);

    if duration > BUFFER_DURATION_US {
        metrics.underruns.fetch_add(1, Ordering::Relaxed);
    }
}
```

**Log periodically (not in callback!):**
```rust
// Every 10 seconds, log stats
let avg_us = total_duration_us / total_calls;
let max_us = max_duration_us;
let underrun_pct = (underruns as f64 / total_calls as f64) * 100.0;

tracing::info!(
    "Audio callback stats: avg={:.1}μs, max={}μs, underruns={:.2}%",
    avg_us, max_us, underrun_pct
);
```

### Profiling Tools

**cargo flamegraph:**
```bash
cargo install flamegraph
sudo cargo flamegraph --bin mqttaudio
# Generates flamegraph.svg
```

**perf (Linux):**
```bash
cargo build --release
perf record --call-graph dwarf ./target/release/mqttaudio
perf report
```

**Instruments (macOS):**
```bash
cargo build --release
instruments -t "Time Profiler" ./target/release/mqttaudio
```

**tracy (cross-platform):**
```rust
// Add tracy instrumentation
#[tracy::profile]
fn audio_callback(...) {
    tracy::zone!("audio_callback");
    // ...
}
```

## Benchmarking

### Criterion Benchmarks

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_channel_mapping(c: &mut Criterion) {
    let source = vec![0.5f32; 512 * 2]; // 512 frames, stereo
    let mut dest = vec![0.0f32; 512 * 8]; // 8 channels
    let map = vec![(0, 2), (1, 5)];

    c.bench_function("channel_map_512_frames", |b| {
        b.iter(|| {
            apply_channel_mapping(
                black_box(&source),
                black_box(&mut dest),
                black_box(&map),
                2,
                8
            )
        })
    });
}

fn bench_mixing_10_samples(c: &mut Criterion) {
    let state = create_mixer_state_with_n_samples(10);
    let mut output = vec![0.0f32; 512 * 2];

    c.bench_function("mix_10_samples_512_frames", |b| {
        b.iter(|| {
            mix_all_samples(
                black_box(&state),
                black_box(&mut output)
            )
        })
    });
}

criterion_group!(benches, bench_channel_mapping, bench_mixing_10_samples);
criterion_main!(benches);
```

### Target Benchmarks

| Benchmark | Target Time | Notes |
|-----------|-------------|-------|
| Channel map (512 frames, 2→8) | < 50 μs | Memory copy + indexing |
| Mix 1 sample (512 frames) | < 200 μs | All operations for one sample |
| Mix 10 samples (512 frames) | < 2000 μs | Linear scaling |
| Fade calculation (512 frames) | < 50 μs | Should be trivial |
| Saturation (512 frames × 8 ch) | < 100 μs | Clamp operation |

## Memory Performance

### Allocation-Free Callback

**Forbidden in audio callback:**
- `Vec::new()`, `Vec::push()`, `Vec::reserve()`
- `Box::new()`
- `Arc::new()`, `Rc::new()`
- `String` operations
- `HashMap` insert/remove
- `format!()` or any string formatting

**Allowed:**
- Reading from pre-allocated buffers
- Writing to pre-allocated buffers
- Stack-allocated arrays (small, fixed size)
- Atomic operations
- Simple arithmetic

**Verify with:**
```rust
#[cfg(test)]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[test]
fn test_callback_no_allocations() {
    let _profiler = dhat::Profiler::new_heap();

    // Run audio callback
    audio_callback(...);

    // Assert no allocations
    let stats = dhat::HeapStats::get();
    assert_eq!(stats.total_blocks, 0);
}
```

### Memory Layout Optimization

**Alignment:**
Ensure buffers are aligned for SIMD:
```rust
#[repr(align(32))]
struct AlignedBuffer {
    data: Vec<f32>,
}
```

**Prefetching:**
Hint to CPU to prefetch next sample's data:
```rust
use std::intrinsics::prefetch_read_data;

for (i, sample) in samples.iter().enumerate() {
    // Prefetch next sample's buffer
    if i + 1 < samples.len() {
        prefetch_read_data(samples[i + 1].buffer.as_ptr(), 3);
    }

    // Process current sample
    // ...
}
```

## Latency Optimization

### Buffer Size Trade-offs

| Buffer Size | Latency | CPU Usage | Glitch Risk |
|-------------|---------|-----------|-------------|
| 128 frames | 2.67 ms | High | High |
| 256 frames | 5.33 ms | Medium | Medium |
| 512 frames | 10.67 ms | Low | Low |
| 1024 frames | 21.33 ms | Very Low | Very Low |

**Recommendation:**
- **Default: 512 frames** (good balance)
- **Low-latency mode: 256 frames** (for responsive systems)
- **Embedded/Raspberry Pi: 1024 frames** (reduce CPU load)

### Startup Latency

**Decode worker optimization:**
```rust
// Don't wait for entire file, start with first chunk
async fn decode_streaming(file: &str) -> Result<DecodedBuffer> {
    let decoder = symphonia::decoder::create(file)?;

    // Decode first N frames
    let initial_chunk = decode_n_frames(&decoder, INITIAL_FRAMES)?;

    // Return immediately, continue decoding in background
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let rest = decode_remaining(&decoder).await;
        tx.send(rest);
    });

    Ok(DecodedBuffer::Partial { initial_chunk, rest_rx: rx })
}
```

**Pre-connection:**
```rust
// Start MQTT connection during initialization
// Don't block on audio setup
tokio::spawn(async move {
    mqtt_client.connect().await;
});

audio_engine.initialize(); // Runs in parallel
```

## Platform-Specific Optimizations

### Linux (ALSA/PulseAudio)

**Thread priority:**
```rust
use libc::{sched_setscheduler, sched_param, SCHED_FIFO};

fn set_realtime_priority() {
    unsafe {
        let param = sched_param { sched_priority: 80 };
        sched_setscheduler(0, SCHED_FIFO, &param);
    }
}
```

**CPU affinity:**
```rust
use libc::{cpu_set_t, sched_setaffinity};

fn pin_to_cpu(cpu: usize) {
    unsafe {
        let mut set: cpu_set_t = std::mem::zeroed();
        CPU_SET(cpu, &mut set);
        sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &set);
    }
}
```

### macOS (CoreAudio)

**Thread time constraint:**
```rust
// Request real-time thread scheduling from CoreAudio
// cpal handles this automatically on macOS
```

### Windows (WASAPI)

**MMCSS (Multimedia Class Scheduler Service):**
```rust
// Request Pro Audio thread priority
// cpal handles this automatically on Windows
```

## Worst-Case Scenarios

### 20 Simultaneous Samples

If optimizations allow:
- Set max limit: `const MAX_SAMPLES: usize = 20;`
- Reject new play commands when limit reached
- Or use priority queue (remove oldest/quietest)

### High Channel Count

For 16-channel output:
- More memory bandwidth
- More channel mapping work
- Consider SIMD for channel interleaving

### Network Stalls

If HTTP download stalls during streaming decode:
- Buffer underrun in decode worker
- Audio callback outputs silence for that sample
- Log warning, continue other samples
- Retry download in background

### CPU Frequency Scaling

On laptops, CPU may downclock:
- Set CPU governor to "performance" mode (Linux)
- Request high performance mode (macOS Energy Saver)
- Monitor callback duration, warn if exceeding budget

## Performance Testing Checklist

- [ ] Benchmark mixing with 1, 5, 10, 20 samples
- [ ] Profile audio callback with flamegraph
- [ ] Verify zero allocations in callback
- [ ] Test on minimum-spec hardware (e.g., Raspberry Pi)
- [ ] Measure startup latency (first play)
- [ ] Test with various buffer sizes (128, 256, 512, 1024)
- [ ] Monitor callback duration over 1-hour stress test
- [ ] Test with all CPU cores busy (background load)
- [ ] Verify graceful degradation under extreme load

## Performance Goals Summary

| Metric | Target | Critical |
|--------|--------|----------|
| Audio callback time | < 5 ms | < 10 ms |
| Startup latency (cached) | < 50 ms | < 100 ms |
| Startup latency (HTTP) | < 300 ms | < 1 s |
| Max simultaneous samples | 20 | 10 |
| Underrun rate | < 0.01% | < 0.1% |
| Memory allocations in callback | 0 | 0 |
| CPU usage (10 samples, 48kHz) | < 10% | < 25% |

**If metrics exceed critical values → Performance bug, must fix before release.**
