# mqttaudio Architecture

## Overview

mqttaudio is a cross-platform (macOS, Linux, Windows) MQTT-controlled audio daemon for interactive entertainment systems. It receives JSON commands via MQTT to play audio with sophisticated multichannel routing and voice grouping capabilities.

## Core Requirements

### Performance
- **Low latency**: Play commands must have near-instant response (< 50ms for cached files)
- **Glitch-free playback**: Audio thread must never block or underrun
- **Smooth playback**: Support 20+ simultaneous sounds without glitches

### Audio Features
- **Multichannel routing**: Map source audio channels to arbitrary output channels (10+ channels supported)
- **Channel naming**: Optional human-readable names for output channels
- **Voice grouping**: Named groups of samples that can be controlled together
- **Per-channel calibration**: Global volume adjustments per output channel
- **Fading**: Fade in/out support per voice
- **Format support**: WAV, OGG, MP3, FLAC - all sample rates, bit depths, channel counts

### Reliability
- **Cross-platform**: macOS, Linux, Windows
- **File security**: Whitelist allowed directories for local file access
- **Cache management**: Intelligent HTTP caching with staleness detection
- **Graceful degradation**: Handle network issues, missing files, etc. without crashing

## System Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                     MQTT Messages (JSON)                      │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ MQTT Client Thread (async/tokio)                             │
│  - rumqttc event loop                                         │
│  - Receives commands                                          │
│  - Sends to Command Handler                                   │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Command Handler (async/tokio)                                │
│  - Parses JSON (serde_json)                                   │
│  - Validates file paths (security check)                      │
│  - Resolves channel names → numbers                           │
│  - Sends commands to Audio Engine via mpsc channel            │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Audio Engine (coordinator thread)                            │
│  - Sample cache (Arc<DecodedBuffer> instances)               │
│  - Voice manager (active voices, fades)                      │
│  - Disk cache manager                                         │
│  - Sends decode requests to worker pool                      │
│  - Updates mixer state (lock-free or mutex, analyzed later)  │
└─────────┬────────────────────────┬───────────────────────────┘
          │                        │
          │                        │
          ▼                        ▼
┌─────────────────────┐   ┌──────────────────────────────────┐
│ Decode Worker Pool  │   │ Audio Callback Thread (cpal)     │
│ (tokio tasks)       │   │ REAL-TIME - NEVER BLOCKS         │
│                     │   │                                  │
│ - Download (reqwest)│   │ - Reads active sample list       │
│ - Decode (symphonia)│   │ - For each sample:               │
│ - Resample (rubato) │   │   - Read from decoded buffer     │
│ - Write to cache    │   │   - Apply fade envelope          │
│ - Store in memory   │   │   - Apply sample volume          │
│ - Cache validation  │   │   - Map channels                 │
│   (async HTTP HEAD) │   │   - Mix to output                │
│                     │   │ - Apply channel calibration      │
└─────────────────────┘   │ - Write to device buffer         │
                          └──────────────────────────────────┘
```

## Threading Model

### Thread 1: MQTT Client (tokio async)
- Runs rumqttc event loop
- Receives MQTT messages
- Minimal processing - just forward to command handler
- Can block on network I/O

### Thread 2: Command Handler (tokio async)
- Parses JSON commands
- Validates security constraints
- Manages cache metadata
- Sends requests to audio engine
- Can allocate, can block

### Thread 3: Audio Engine Coordinator (std::thread or tokio task)
- Maintains sample cache (memory)
- Manages voice state
- Receives play/stop/fade commands via channel
- Spawns decode tasks
- Updates shared mixer state for audio callback
- Can allocate, can block (not on critical path)

### Thread 4+: Decode Worker Pool (tokio async tasks)
- Downloads files (HTTP)
- Decodes audio (symphonia)
- Resamples to output rate (rubato)
- Writes to disk cache
- Stores decoded PCM in memory (Arc for zero-copy sharing)
- Background cache validation (HTTP HEAD requests)
- Can allocate, can block

### Thread N: Audio Callback (cpal, highest priority)
- **REAL-TIME CONSTRAINTS**
- **NEVER allocates**
- **NEVER blocks**
- **NEVER locks** (or only very briefly with lock-free structures)
- Reads from pre-decoded buffers
- Performs mixing math
- Writes to output buffer
- If anything goes wrong, outputs silence rather than blocking

## Data Flow: Play Command

### Happy Path (cached file)
1. MQTT message arrives: `{"command": "play", "message": {"file": "..."}}`
2. Command handler parses, validates
3. Command sent to Audio Engine
4. Audio Engine checks memory cache - **HIT**
5. Audio Engine adds sample to active playback list
6. Next audio callback reads from buffer and plays
7. **Latency: ~5-10ms**

### First Play (uncached HTTP file)
1. MQTT message arrives
2. Command handler parses, validates
3. Command sent to Audio Engine
4. Audio Engine checks memory cache - **MISS**
5. Audio Engine checks disk cache - **MISS**
6. Audio Engine spawns decode worker
7. Decode worker downloads file (streaming)
8. As soon as first chunk arrives, decode begins
9. Decode worker produces first 50-100ms of PCM
10. Audio Engine adds sample to active list with partial buffer
11. Audio callback begins playback
12. Decode worker races to stay ahead
13. **Latency: ~100-300ms (network dependent)**

### Subsequent Play (disk cached)
1. Command arrives
2. Audio Engine checks memory - MISS (was freed)
3. Audio Engine checks disk cache - **HIT**
4. Decode worker reads from disk (fast)
5. Decodes to PCM
6. Sample ready to play
7. **Latency: ~20-50ms**

## Component Breakdown

### Sample Cache
- **Memory cache**: `HashMap<String, Arc<DecodedBuffer>>`
  - Key: File URL/path
  - Value: Fully decoded PCM data ready to play
  - Shared via Arc (zero-copy)
- **Disk cache**: See caching.md for details

### Voice Manager
```rust
struct Voice {
    id: String,              // User-provided name
    samples: Vec<SampleId>,  // Active samples in this voice
    volume: AtomicF32,       // Voice-level volume
    fade: FadeState,         // Current fade envelope
}
```

### Mixer State
Shared between Audio Engine and Audio Callback. Options:
1. **Arc<Mutex<MixerState>>** - Simple, might work if lock contention is low
2. **Arc<RwLock<MixerState>>** - Audio callback only reads, engine writes
3. **Lock-free queue** - Audio callback polls for state updates
4. **Double buffering** - Swap pointers atomically

**Decision**: Start with Arc<RwLock>, measure contention, optimize if needed.

### Decoded Buffer Format
```rust
struct DecodedBuffer {
    data: Vec<f32>,          // Interleaved PCM samples
    channels: usize,         // Number of channels
    sample_rate: u32,        // Always matches output device
    frames: usize,           // Total frames
}
```

## Error Handling

### Audio Callback Errors
- **Buffer underrun**: Output silence for that sample, continue others
- **Invalid state**: Log error, skip sample, don't crash

### Command Errors
- **Invalid JSON**: Send error response (future feature?), log
- **File not found**: Log error, ignore command
- **Security violation**: Log warning, reject command
- **Network error**: Log, retry logic in cache layer

### Graceful Degradation
- If a single sample fails to decode, others keep playing
- If network is down, cached content still works
- If MQTT disconnects, audio continues, auto-reconnect

## Next Steps for Implementation

1. Set up basic Rust project with dependencies
2. Implement cpal audio output (simple sine wave test)
3. Implement symphonia decoder (load one file, play)
4. Implement basic mixer (multiple samples)
5. Add channel routing
6. Add MQTT command handling
7. Add voice management
8. Add caching
9. Add fading
10. Polish and test

## See Also
- audio-engine.md - Detailed mixer implementation
- commands.md - JSON command reference
- configuration.md - Config file format
- caching.md - Cache strategy
- libraries.md - Library choices and rationale
- testing.md - Test strategy
- performance.md - Performance requirements
