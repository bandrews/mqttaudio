# Quick Reference

## Command Cheat Sheet

### Play Audio
```json
{"command": "play", "message": {
  "file": "http://example.com/sound.wav",
  "voice": "ambience",
  "channel_map": [{"src": 0, "dest": 6}, {"src": 1, "dest": 7}],
  "volume": 0.8,
  "loop": false,
  "fade_in": 1000
}}
```

### Voice Control
```json
{"command": "voice_stop", "message": {"voice": "ambience"}}
{"command": "voice_fade_out", "message": {"voice": "ambience", "time": 2000}}
{"command": "voice_volume", "message": {"voice": "music", "volume": 0.5}}
```

### System Commands
```json
{"command": "stopall"}
{"command": "fadeout", "message": {"time": 3000}}
{"command": "precache", "message": {"file": "http://example.com/big.wav"}}
{"command": "cache_clear"}
{"command": "cache_invalidate", "message": {"file": "http://example.com/old.wav"}}
```

## Minimal Config

```json
{
  "mqtt": {
    "topic": "audio/commands"
  }
}
```

## Full Config

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands"
  },
  "audio": {
    "device": null,
    "sample_rate": 48000,
    "channel_names": {
      "0": "front_left",
      "1": "front_right"
    },
    "channel_volumes": {
      "front_left": 0.9
    }
  },
  "cache": {
    "enabled": true,
    "revalidate_after_seconds": 300
  },
  "security": {
    "allowed_directories": ["/opt/sounds"]
  }
}
```

## CLI Usage

```bash
# List devices
mqttaudio --list-devices

# Run with config
mqttaudio --config config.json

# Override topic
mqttaudio --config config.json --topic "my/topic"

# Verbose mode
mqttaudio --config config.json --verbose
```

## Testing Commands (bash + mosquitto)

```bash
TOPIC="audio/commands"

# Play a file
mosquitto_pub -t $TOPIC -m '{"command": "play", "message": {"file": "test.wav"}}'

# Play with routing
mosquitto_pub -t $TOPIC -m '{
  "command": "play",
  "message": {
    "file": "quad.wav",
    "channel_map": [
      {"src": 0, "dest": 6},
      {"src": 1, "dest": 7},
      {"src": 2, "dest": 8},
      {"src": 3, "dest": 9}
    ]
  }
}'

# Stop all
mosquitto_pub -t $TOPIC -m '{"command": "stopall"}'
```

## Performance Targets

| Metric | Target |
|--------|--------|
| Audio callback | < 5 ms |
| Cached file playback | < 10 ms |
| HTTP file first play | < 300 ms |
| Max simultaneous samples | 20 |
| Underrun rate | < 0.01% |

## Key File Locations

- Config: `./mqttaudio.json` or `~/.config/mqttaudio/config.json`
- Cache: `~/.mqttaudio/cache/`
- Logs: stdout/stderr

## Supported Formats

- WAV (all bit depths, sample rates)
- OGG/Vorbis
- MP3
- FLAC

## Troubleshooting

**No sound:**
- Check device: `mqttaudio --list-devices`
- Check volume in commands
- Check MQTT connection

**Glitches/dropouts:**
- Increase buffer size in config
- Reduce simultaneous samples
- Check CPU usage
- Check logs for timing warnings

**File not playing:**
- Check allowed_directories for local files
- Check network for HTTP files
- Check file format support
- Check logs for decode errors

**Cache not working:**
- Check `~/.mqttaudio/cache/` exists and is writable
- Check `cache.enabled` in config
- Check logs for cache errors

## Architecture Overview

```
MQTT → Command Handler → Audio Engine → Mixer → cpal → Device
                              ↓
                        Decode Workers
                              ↓
                         Disk Cache
```

## Thread Model

- **Thread 1:** MQTT (async/tokio)
- **Thread 2:** Command Handler (async/tokio)
- **Thread 3:** Audio Engine Coordinator
- **Thread 4+:** Decode Workers (async/tokio pool)
- **Thread N:** Audio Callback (cpal, real-time)

## Important Constraints

1. **Audio callback MUST NOT:**
   - Allocate memory
   - Block on I/O
   - Acquire locks (except brief lock-free ops)
   - Take longer than buffer duration

2. **File paths:**
   - Must be in allowed_directories
   - No path traversal (`../`)
   - Or use HTTP/HTTPS URLs

3. **Channel mapping:**
   - Source channels must exist in file
   - Dest channels must exist on device
   - Auto-maps if not specified

## Version Compatibility

**v0.1.0:**
- Supports legacy mqttaudio commands (soundPlay, etc.)
- Config format stable
- Command format stable

## See Also

- **architecture.md** - System design
- **commands.md** - Full command reference
- **configuration.md** - Config file details
- **caching.md** - Cache behavior
- **performance.md** - Performance tuning
- **testing.md** - Test strategy
- **implementation-roadmap.md** - Development guide
