# Architecture

This document describes mqttaudio's internal architecture for developers and contributors.

## Overview

mqttaudio is a multi-threaded audio daemon with real-time constraints. It receives commands via MQTT, decodes audio files, and outputs mixed audio to hardware.

```
┌─────────────────────────────────────────────────────────────────┐
│                     MQTT Messages (JSON)                         │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ MQTT Client Thread (async/tokio)                                 │
│  - rumqttc event loop                                            │
│  - Receives messages, forwards to command handler                │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Command Handler (async/tokio)                                    │
│  - Parses JSON commands                                          │
│  - Validates file paths (security)                               │
│  - Sends to Audio Engine                                         │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Audio Engine (coordinator)                                       │
│  - Manages sample cache                                          │
│  - Tracks voices and active samples                              │
│  - Spawns decode workers                                         │
│  - Updates mixer state                                           │
└─────────┬─────────────────────────┬─────────────────────────────┘
          │                         │
          ▼                         ▼
┌──────────────────────┐   ┌──────────────────────────────────────┐
│ Decode Workers       │   │ Audio Callback (cpal)                │
│ (tokio tasks)        │   │ REAL-TIME - NEVER BLOCKS             │
│                      │   │                                      │
│ - HTTP download      │   │ - Reads active sample list           │
│ - Decode (symphonia) │   │ - Mixes samples into output buffer   │
│ - Resample (rubato)  │   │ - Applies fades, volumes, ducking    │
│ - Store in cache     │   │ - Routes to output channels          │
└──────────────────────┘   └──────────────────────────────────────┘
```

## Threading Model

### Thread 1: MQTT Client (tokio async)

- Runs rumqttc event loop
- Receives MQTT messages
- Forwards to command handler
- Can block on network I/O

### Thread 2: Command Handler (tokio async)

- Parses JSON commands
- Validates security (file paths)
- Manages cache metadata
- Sends to audio engine
- Can allocate, can block

### Thread 3: Audio Engine Coordinator

- Maintains sample cache
- Manages voice state
- Receives commands via channel
- Spawns decode tasks
- Updates mixer state
- Can allocate, can block

### Thread 4+: Decode Workers (tokio tasks)

- Download files (HTTP)
- Decode audio (symphonia)
- Resample to output rate (rubato)
- Write to disk cache
- Store decoded PCM in memory
- Can allocate, can block

### Thread N: Audio Callback (cpal)

This is the critical real-time thread.

**HARD CONSTRAINTS:**
- **NEVER allocate memory** (no Vec::push, no String, no Box)
- **NEVER block on I/O**
- **NEVER hold locks** (except very brief lock-free operations)
- **NEVER call syscalls**
- Must complete within buffer duration (~10ms for 512 frames at 48kHz)

The audio callback:
1. Zeros the output buffer
2. Iterates active samples
3. For each sample: reads from decoded buffer, applies volume/fade/ducking, maps channels
4. Mixes into output buffer
5. Clamps to prevent clipping
6. Returns

If anything goes wrong, it outputs silence rather than blocking.

## Key Data Structures

### DecodedBuffer

Pre-decoded audio ready for playback:

```rust
struct DecodedBuffer {
    data: Vec<f32>,      // Interleaved PCM samples
    channels: usize,     // Number of channels
    sample_rate: u32,    // Always matches output device
    frames: usize,       // Total frames
}
```

Shared via `Arc<DecodedBuffer>` for zero-copy access.

### ActiveSample

A currently playing sound:

```rust
struct ActiveSample {
    buffer: Arc<DecodedBuffer>,
    position: usize,           // Current playback position (frames)
    volume: f32,               // Sample volume
    voice_id: String,          // Voice group
    channel_map: Vec<(usize, usize)>,  // src → dest routing
    fade_state: FadeState,     // Fade in/out state
    loop_mode: bool,
    // ...
}
```

### Voice

A group of samples controlled together:

```rust
struct Voice {
    id: String,
    sample_ids: Vec<u64>,
    volume: f32,
    fade_state: Option<FadeState>,
}
```

## Data Flow: Play Command

### Happy Path (cached file)

1. MQTT message: `{"command": "play", "message": {"file": "..."}}`
2. Command handler parses, validates
3. Sent to Audio Engine
4. Engine checks memory cache → HIT
5. Creates ActiveSample, adds to mixer state
6. Next audio callback plays it
7. **Latency: ~5-10ms**

### Uncached HTTP File

1. MQTT message arrives
2. Command handler parses, validates
3. Sent to Audio Engine
4. Memory cache MISS, disk cache MISS
5. Spawns decode worker
6. Worker downloads file (streaming)
7. Worker decodes, resamples
8. Stores in memory cache
9. Engine adds to active samples
10. Audio callback plays
11. **Latency: ~100-300ms**

## Key Components

### Mixer (`src/audio/mixer.rs`)

The real-time mixing core. Called from cpal audio callback.

Responsibilities:
- Zero output buffer
- Iterate active samples
- Read from decoded buffers
- Apply volumes, fades, ducking multipliers
- Map channels to outputs
- Clamp output

### Audio Engine (`src/audio/engine.rs`)

Coordinates all audio operations.

Responsibilities:
- Command processing
- Sample cache management
- Voice tracking
- Spawning decode workers
- Updating mixer state

### Decoder (`src/audio/decoder.rs`)

Decodes audio files using symphonia.

Supports: WAV, MP3, OGG, FLAC

### Resampler (`src/audio/resampler.rs`)

Converts sample rates using rubato.

All decoded audio is resampled to match the output device.

### Ducking Engine (`src/audio/ducking.rs`)

Manages automatic volume ducking.

- Tracks active voices
- Evaluates ducking rules
- Calculates smooth fade multipliers

### Cache (`src/cache/`)

Two-tier caching:
- **Memory cache:** Decoded PCM for instant playback
- **Disk cache:** Downloaded files for persistence

## Dependencies

| Crate | Purpose |
|-------|---------|
| cpal | Cross-platform audio I/O |
| symphonia | Audio decoding |
| rubato | Sample rate conversion |
| rumqttc | MQTT client |
| reqwest | HTTP downloads |
| tokio | Async runtime |
| serde/serde_json | JSON parsing |
| clap | CLI parsing |
| tracing | Logging |

## Performance Targets

| Metric | Target | Critical |
|--------|--------|----------|
| Audio callback time | < 5 ms | < 10 ms |
| Cached playback latency | < 10 ms | < 50 ms |
| HTTP first-play latency | < 300 ms | < 1 s |
| Simultaneous samples | 20+ | 10+ |
| Underrun rate | < 0.01% | < 0.1% |

## Platform Support

- **macOS:** CoreAudio via cpal
- **Linux:** ALSA/PulseAudio via cpal
- **Windows:** WASAPI via cpal

## Project Structure

```
src/
├── main.rs              # Entry point, CLI
├── config.rs            # Configuration loading
├── audio/
│   ├── mod.rs
│   ├── engine.rs        # Audio engine coordinator
│   ├── mixer.rs         # Real-time mixer
│   ├── ducking.rs       # Ducking engine
│   ├── bass_management.rs
│   ├── pitch_correction.rs
│   ├── input.rs         # Microphone input
│   ├── decoder.rs       # Audio decoding
│   ├── resampler.rs     # Sample rate conversion
│   └── types.rs
├── mqtt/
│   ├── mod.rs
│   ├── client.rs        # MQTT connection
│   └── commands.rs      # Command parsing
├── cache/
│   ├── mod.rs
│   ├── disk.rs          # Disk cache
│   └── memory.rs        # Memory cache
└── voice.rs             # Voice management
```

## Contributing

1. Run tests: `cargo test`
2. Run benchmarks: `cargo bench`
3. Format code: `cargo fmt`
4. Check lints: `cargo clippy`
5. Build release: `cargo build --release`

Key areas for contribution:
- Performance optimization
- Additional audio format support
- Platform-specific improvements
- Documentation
