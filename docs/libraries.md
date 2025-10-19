# Library Choices and Rationale

## Core Dependencies

### Audio I/O: cpal

**Crate:** `cpal = "0.15"`

**Purpose:** Cross-platform audio input/output

**Why:**
- Pure Rust, no C dependencies
- Supports macOS (CoreAudio), Linux (ALSA/PulseAudio), Windows (WASAPI)
- Multichannel support via interleaved buffers
- Low-level control over audio stream
- Actively maintained by RustAudio

**Alternatives considered:**
- **SDL2 bindings** - Legacy app used this, but FFI overhead and less Rust-idiomatic
- **portaudio-rs** - FFI to C library, less reliable cross-platform
- **rodio** - Built on cpal but hides multichannel control

**Key features we use:**
- `Stream::build_output_stream()` for device output
- Device enumeration
- Interleaved sample format (f32)
- Configurable buffer size and sample rate

### Audio Decoding: symphonia

**Crate:** `symphonia = { version = "0.5", features = ["all"] }`

**Purpose:** Decode audio files (WAV, OGG, MP3, FLAC, etc.)

**Why:**
- Pure Rust implementation
- Excellent format support (all our target formats)
- Streaming decoder (can start playback before file fully downloaded)
- Good performance (~85-100% of FFmpeg)
- Modern API design
- Actively maintained

**Alternatives considered:**
- **lewton** - OGG only, not comprehensive enough
- **minimp3** - MP3 only, FFI to C
- **hound** - WAV only
- **FFmpeg bindings** - Complex C dependencies, overkill

**Key features we use:**
- Format detection and demuxing
- Codec registry for all formats
- Streaming decode
- Metadata extraction (sample rate, channels, bit depth)

### Sample Rate Conversion: rubato

**Crate:** `rubato = "0.14"`

**Purpose:** High-quality sample rate conversion (resampling)

**Why:**
- Pure Rust
- High quality (sinc interpolation)
- Handles arbitrary rate conversions (e.g., 44.1kHz → 48kHz)
- Multiple algorithms (synchronous, asynchronous)
- Good performance

**Alternatives considered:**
- **libsamplerate (SRC) bindings** - FFI to C, but high quality
- **dasp::signal::interpolate** - Lower quality, simpler algorithms
- **Roll our own** - Too complex, quality issues

**Key features we use:**
- `SincFixedIn` for fixed input chunk size (audio file)
- `SincFixedOut` for fixed output chunk size (device callback)
- Configurable quality vs performance trade-off

### MQTT Client: rumqttc

**Crate:** `rumqttc = "0.24"`

**Purpose:** MQTT 3.1.1 client library

**Why:**
- Pure Rust
- Async/await with tokio
- Reliable reconnection logic
- Good documentation
- Active development
- Modern API design

**Alternatives considered:**
- **paho-mqtt** - FFI to Paho C library, mature but less idiomatic
- **mqtt-async-client** - Less popular, fewer features

**Key features we use:**
- `AsyncClient` for async operations
- `EventLoop` for receiving messages
- Automatic reconnection
- QoS support
- Topic wildcards

### HTTP Client: reqwest

**Crate:** `reqwest = { version = "0.11", features = ["stream"] }`

**Purpose:** Download audio files from HTTP/HTTPS

**Why:**
- Standard Rust HTTP client (most popular)
- Async/await support
- Streaming response bodies (critical for large audio files)
- Supports ETag, Last-Modified headers
- Connection pooling

**Alternatives considered:**
- **ureq** - Synchronous only, would block
- **hyper** - Lower level, more complex
- **curl bindings** - FFI to C

**Key features we use:**
- Streaming downloads (`.bytes_stream()`)
- HEAD requests for cache validation
- Header parsing (ETag, Last-Modified)
- TLS support

## Supporting Libraries

### JSON Parsing: serde & serde_json

**Crates:**
- `serde = { version = "1.0", features = ["derive"] }`
- `serde_json = "1.0"`

**Purpose:** Parse MQTT commands and config files

**Why:**
- Industry standard in Rust
- Type-safe deserialization
- Derive macros for easy struct mapping
- Excellent error messages

**Usage:**
```rust
#[derive(Deserialize)]
struct PlayCommand {
    file: String,
    voice: Option<String>,
    volume: Option<f32>,
}
```

### CLI Parsing: clap

**Crate:** `clap = { version = "4.5", features = ["derive"] }`

**Purpose:** Parse command line arguments

**Why:**
- Modern derive API
- Automatic help generation
- Validation built-in
- Widely used

**Usage:**
```rust
#[derive(Parser)]
struct Args {
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[arg(short, long)]
    topic: Option<String>,
}
```

### Async Runtime: tokio

**Crate:** `tokio = { version = "1", features = ["full"] }`

**Purpose:** Async runtime for MQTT, HTTP, decode workers

**Why:**
- Required by rumqttc and reqwest
- Industry standard async runtime
- Excellent performance
- Great ecosystem

**Key features we use:**
- `tokio::spawn()` for background tasks
- `tokio::sync::mpsc` for channels
- `tokio::time` for delays/timeouts
- `tokio::fs` for async file I/O

### DSP Utilities: dasp

**Crate:** `dasp = "0.11"`

**Purpose:** Digital audio signal processing utilities

**Why:**
- Format conversions (i16 → f32, etc.)
- Sample iterators
- Frame manipulation
- Pure Rust

**Usage:**
- Convert decoded samples to f32
- Interleave/deinterleave channels
- Sample math utilities

### Logging: tracing

**Crate:**
- `tracing = "0.1"`
- `tracing-subscriber = "0.3"`

**Purpose:** Structured logging

**Why:**
- Modern logging framework
- Async-aware
- Flexible filtering
- Spans for context
- Better than `log` crate for async code

**Usage:**
```rust
tracing::info!("Playing file: {}", file);
tracing::warn!("Cache validation failed: {}", error);
```

### Lock-Free Structures: ringbuf or crossbeam

**Crate:** `ringbuf = "0.3"` or `crossbeam = "0.8"`

**Purpose:** Lock-free communication between threads

**Why:**
- Avoid lock contention in audio callback
- Ring buffers for streaming decode
- Lock-free queues for commands

**Usage:**
- Ring buffer between decode worker and audio callback
- Potentially for mixer state updates

## Development Dependencies

### Testing: cargo test + custom test files

**Built-in:** Rust's test framework

**Additional:**
- Generate test audio files (WAV, OGG, MP3)
- Various channel counts, sample rates, bit depths

### Benchmarking: criterion

**Crate:** `criterion = "0.5"` (dev dependency)

**Purpose:** Performance benchmarking

**Why:**
- Statistical analysis of benchmarks
- Detect performance regressions
- Measure mixing performance

**Usage:**
```rust
fn bench_mixing(c: &mut Criterion) {
    c.bench_function("mix_10_samples", |b| {
        b.iter(|| {
            // Mix 10 simultaneous samples
        })
    });
}
```

## Dependency Graph

```
mqttaudio
├── cpal (audio I/O)
├── symphonia (decode)
│   ├── symphonia-core
│   ├── symphonia-codec-*
│   └── symphonia-format-*
├── rubato (resample)
├── rumqttc (MQTT)
│   └── tokio
├── reqwest (HTTP)
│   ├── tokio
│   └── hyper
├── serde + serde_json (parsing)
├── clap (CLI)
├── dasp (DSP utils)
├── tracing (logging)
└── ringbuf or crossbeam (lock-free)
```

## Version Pinning Strategy

- **Patch versions:** Allow updates (e.g., `0.15.2` → `0.15.3`)
- **Minor versions:** Review carefully, may break
- **Major versions:** Manual upgrade, test thoroughly

**Cargo.toml example:**
```toml
cpal = "0.15"           # Allows 0.15.x
symphonia = "0.5"       # Allows 0.5.x
rumqttc = "0.24"        # Allows 0.24.x
```

## Build Considerations

### Compile Time

- `symphonia` with all features is slow to compile (~2-3 minutes first build)
- Enable incremental compilation for development
- Use `cargo build --release` for production

### Binary Size

- Release binary with all features: ~5-10 MB
- Can reduce with `strip = true` in Cargo.toml
- Further reduce with `opt-level = "z"` (size optimization)

### Platform-Specific Dependencies

**Linux:**
- May need ALSA development libraries: `libasound2-dev`
- Or PulseAudio: `libpulse-dev`

**macOS:**
- No additional dependencies (CoreAudio is system framework)

**Windows:**
- No additional dependencies (WASAPI is system API)

## Future Dependency Considerations

### Potential Additions

**GUI (future):**
- `egui` - Immediate mode GUI for optional control panel
- `iced` - Declarative GUI framework

**Advanced DSP (if needed):**
- `fundsp` - For complex audio graphs, effects
- More `dasp` features

**Metrics/Monitoring:**
- `prometheus` - Metrics export
- `sysinfo` - System resource monitoring

**Configuration Hot Reload:**
- `notify` - File system watching

### Dependencies to Avoid

- **FFmpeg bindings** - Complex, C dependencies
- **GStreamer bindings** - Overkill, hard to deploy
- **Heavy GUI frameworks** - Keep CLI-focused
- **Database** - Keep stateless (except simple file cache)

## Dependency Audit

Regularly audit dependencies for:
- Security vulnerabilities: `cargo audit`
- Outdated versions: `cargo outdated`
- License compatibility: `cargo license`

Run before each release:
```bash
cargo update           # Update within version constraints
cargo audit            # Check for vulnerabilities
cargo test             # Ensure nothing broke
cargo build --release  # Test release build
```

## Alternatives for Constrained Environments

If deploying to embedded/low-resource systems:

**Minimal feature set:**
```toml
symphonia = { version = "0.5", default-features = false, features = ["wav", "ogg"] }
```

**Smaller async runtime:**
```toml
# Replace tokio with:
async-std = "1"
# Or even:
smol = "1"
```

**Static linking:**
```toml
[profile.release]
lto = true              # Link-time optimization
codegen-units = 1       # Slower compile, smaller binary
strip = true            # Remove debug symbols
```

This can reduce binary from ~8 MB to ~3 MB.
