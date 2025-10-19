# mqttaudio Documentation

Complete architecture and design documentation for mqttaudio v2.0.

## Documentation Index

### Getting Started

- **[quick-reference.md](quick-reference.md)** - Cheat sheet for commands, config, and common tasks
- **[implementation-roadmap.md](implementation-roadmap.md)** - Step-by-step development guide

### Architecture & Design

- **[architecture.md](architecture.md)** - Overall system architecture, threading model, data flow
- **[audio-engine.md](audio-engine.md)** - Audio mixer implementation, real-time constraints
- **[caching.md](caching.md)** - Cache strategy, invalidation, performance

### Reference

- **[commands.md](commands.md)** - Complete JSON command reference with examples
- **[configuration.md](configuration.md)** - Config file format, CLI arguments, defaults
- **[libraries.md](libraries.md)** - Library choices, dependencies, rationale

### Development

- **[testing.md](testing.md)** - Test strategy, unit tests, integration tests, benchmarks
- **[performance.md](performance.md)** - Performance requirements, optimization, profiling
- **[bugs.md](bugs.md)** - Known issues and bug tracking

## Quick Links by Task

### "I want to understand the system"
1. Start with [architecture.md](architecture.md) - high-level overview
2. Read [audio-engine.md](audio-engine.md) - core mixing logic
3. Check [quick-reference.md](quick-reference.md) - see it in action

### "I want to implement this"
1. Read [implementation-roadmap.md](implementation-roadmap.md) - step-by-step plan
2. Refer to [libraries.md](libraries.md) - dependencies and why
3. Check [testing.md](testing.md) - write tests as you go
4. Monitor [performance.md](performance.md) - meet performance targets

### "I want to use this"
1. See [quick-reference.md](quick-reference.md) - common commands
2. Read [commands.md](commands.md) - full command reference
3. Check [configuration.md](configuration.md) - set up config file

### "I want to understand caching"
1. Read [caching.md](caching.md) - complete cache design

### "I want to optimize performance"
1. Read [performance.md](performance.md) - constraints and strategies
2. Check [audio-engine.md](audio-engine.md) - mixer implementation details

### "I want to add tests"
1. Read [testing.md](testing.md) - comprehensive test strategy

## Key Design Decisions

### Why Rust?
- Memory safety without garbage collection
- Zero-cost abstractions
- Excellent audio ecosystem (cpal, symphonia, rubato)
- Cross-platform support
- No runtime pauses in audio callback

### Why Custom Mixer?
- Need multichannel routing (not available in rodio/kira)
- Full control over channel mapping
- Simpler than anticipated with right libraries
- Libraries handle hard parts (decoding, resampling)

### Why This Threading Model?
- Audio callback must never block (hard real-time requirement)
- Expensive work (decode, download) off critical path
- Lock-free communication where possible
- Proven pattern for audio software

### Why This Cache Strategy?
- Balance freshness and performance
- Simple implementation (no complex invalidation)
- HTTP standard validation (ETag, Last-Modified)
- Lazy validation keeps playback responsive

## Critical Constraints

### Real-Time Audio
- Audio callback has ~10ms budget (512 frames @ 48kHz)
- Target < 5ms (50% headroom for jitter)
- **NEVER** allocate in callback
- **NEVER** block in callback
- **NEVER** hold locks in callback (except brief lock-free)

### Memory Safety
- Rust enforces at compile time
- Arc for zero-copy buffer sharing
- Lock-free structures where needed

### Cross-Platform
- macOS (CoreAudio via cpal)
- Linux (ALSA/PulseAudio via cpal)
- Windows (WASAPI via cpal)

## Implementation Phases

1. **Basic audio output** (sine wave)
2. **Audio decoding** (WAV playback)
3. **Sample rate conversion** (any rate → device rate)
4. **Multi-sample mixing** (polyphonic playback)
5. **MQTT integration** (receive commands)
6. **Voice management** (group control)
7. **Channel routing** (multichannel mapping)
8. **Caching** (HTTP + disk)
9. **Fading** (smooth transitions)
10. **Configuration** (JSON config file)
11. **Polish & testing** (comprehensive tests)
12. **Release** (v0.1.0)

## Dependencies Summary

**Core:**
- `cpal` - Audio I/O
- `symphonia` - Audio decoding
- `rubato` - Sample rate conversion
- `rumqttc` - MQTT client
- `reqwest` - HTTP client
- `tokio` - Async runtime

**Supporting:**
- `serde`/`serde_json` - JSON parsing
- `clap` - CLI parsing
- `dasp` - DSP utilities
- `tracing` - Logging
- `ringbuf` - Lock-free buffers

See [libraries.md](libraries.md) for complete details.

## Performance Targets

| Metric | Target | Critical |
|--------|--------|----------|
| Audio callback | < 5 ms | < 10 ms |
| Cached playback | < 10 ms | < 50 ms |
| HTTP first play | < 300 ms | < 1 s |
| Max simultaneous | 20 samples | 10 samples |
| Underruns | < 0.01% | < 0.1% |

See [performance.md](performance.md) for complete analysis.

## Testing Strategy

- Unit tests: >80% coverage target
- Integration tests: All command workflows
- Format tests: WAV, OGG, MP3, FLAC
- Performance tests: Callback timing
- Stress tests: 20+ simultaneous samples
- Platform tests: macOS, Linux, Windows

See [testing.md](testing.md) for complete strategy.

## Future Enhancements (Post v0.1.0)

- MQTT authentication
- GUI control panel (optional)
- Advanced DSP (EQ, filters)
- Metrics export (Prometheus)
- Hot config reload
- LRU cache eviction
- File system watching

## Notes for Future Self

### When Debugging Audio Issues
1. Check callback timing (see [performance.md](performance.md))
2. Verify zero allocations in callback
3. Check lock contention
4. Profile with flamegraph
5. Test with minimal sample count first

### When Adding Features
1. Does it affect the audio callback? (Be very careful)
2. Write tests first (TDD for core features)
3. Update documentation
4. Check performance impact
5. Test on multiple platforms

### When Performance Degrades
1. Profile first (don't guess)
2. Check for allocations in callback
3. Look for lock contention
4. Review recent changes
5. Benchmark against baseline

### Common Pitfalls
- Allocating in audio callback (glitches)
- Blocking in audio callback (underruns)
- Not handling edge cases (empty files, network errors)
- Forgetting to clamp mixed audio (clipping)
- Not testing on real hardware early enough

## Documentation Maintenance

When updating this documentation:

1. Keep quick-reference.md in sync with commands.md
2. Update implementation-roadmap.md if architecture changes
3. Add bugs to bugs.md as discovered
4. Update performance.md with actual benchmarks
5. Keep this README index current

## Version History

- **v0.1.0-design** (Current) - Complete architecture documentation
- **v0.1.0** (Planned) - First release

## Contact

See main README.md for project information and contact details.
