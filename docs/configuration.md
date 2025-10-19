# Configuration File

mqttaudio can be configured via a JSON configuration file, command line arguments, or a combination of both. Command line arguments override configuration file settings.

## Configuration File Location

Default search paths (in order):
1. Path specified via `--config <file>` command line argument
2. `./mqttaudio.json` (current directory)
3. `~/.config/mqttaudio/config.json`
4. `/etc/mqttaudio/config.json` (Linux/macOS)

If no config file is found, defaults are used.

## Complete Configuration Example

```json
{
  "mqtt": {
    "server": "localhost",
    "port": 1883,
    "topic": "audio/commands",
    "client_id": null,
    "reconnect_delay_seconds": 10
  },
  "audio": {
    "device": null,
    "sample_rate": 48000,
    "buffer_size": 512,
    "channel_names": {
      "0": "front_left",
      "1": "front_right",
      "2": "center",
      "3": "lfe",
      "4": "rear_left",
      "5": "rear_right",
      "6": "side_left",
      "7": "side_right",
      "8": "ceiling_1",
      "9": "ceiling_2"
    },
    "channel_volumes": {
      "lfe": 0.7,
      "ceiling_1": 0.85,
      "ceiling_2": 0.85
    }
  },
  "cache": {
    "enabled": true,
    "directory": "~/.mqttaudio/cache",
    "revalidate_after_seconds": 300
  },
  "security": {
    "allowed_directories": [
      "/opt/sounds",
      "/home/user/audio",
      "~/sounds"
    ]
  },
  "logging": {
    "level": "info",
    "verbose": false
  }
}
```

## Configuration Sections

### MQTT Settings

```json
"mqtt": {
  "server": "localhost",
  "port": 1883,
  "topic": "audio/commands",
  "client_id": null,
  "reconnect_delay_seconds": 10
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server` | string | `"localhost"` | MQTT broker hostname or IP |
| `port` | integer | `1883` | MQTT broker port |
| `topic` | string | **REQUIRED** | MQTT topic to subscribe to (supports wildcards: `audio/#`) |
| `client_id` | string/null | `null` | MQTT client ID. If null, generates `mqttaudio_<pid>` |
| `reconnect_delay_seconds` | integer | `10` | Seconds to wait before reconnecting after disconnect |

**Note:** Authentication is not yet supported. Connect only to trusted MQTT brokers.

### Audio Settings

```json
"audio": {
  "device": null,
  "sample_rate": 48000,
  "buffer_size": 512,
  "channel_names": { ... },
  "channel_volumes": { ... }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `device` | string/null | `null` | Audio device name. If null, uses system default. Use `--list-devices` to see available devices |
| `sample_rate` | integer | `48000` | Output sample rate (Hz). Common values: 44100, 48000, 96000 |
| `buffer_size` | integer | `512` | Audio buffer size (frames). Smaller = lower latency, higher CPU. Common values: 256, 512, 1024 |
| `channel_names` | object | `{}` | Map channel numbers to human-readable names (optional) |
| `channel_volumes` | object | `{}` | Per-channel calibration volumes (0.0 - 1.0) for hardware differences |

#### Channel Names

Optional mapping of channel numbers to names:

```json
"channel_names": {
  "0": "front_left",
  "1": "front_right",
  "6": "rear_left",
  "7": "rear_right"
}
```

Allows play commands to use names instead of numbers:
```json
"channel_map": [
  {"src": 0, "dest": "front_left"}  // Instead of "dest": 0
]
```

#### Channel Volumes

Per-channel calibration to compensate for hardware differences:

```json
"channel_volumes": {
  "front_left": 0.9,   // Can use names (if defined)
  "3": 0.7,            // Or channel numbers
  "ceiling_1": 0.85
}
```

Applied globally to all audio on that channel. Useful for:
- Compensating for different speaker sensitivities
- Balancing volume across a multi-speaker setup
- Reducing volume on specific zones

### Cache Settings

```json
"cache": {
  "enabled": true,
  "directory": "~/.mqttaudio/cache",
  "revalidate_after_seconds": 300
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable disk caching of downloaded files |
| `directory` | string | `"~/.mqttaudio/cache"` | Directory for cache storage. `~` expands to user home |
| `revalidate_after_seconds` | integer | `300` | Seconds before checking if cached file is stale (0 = always check, very high = rarely check) |

**Cache Behavior:**
- If `revalidate_after_seconds = 300` (5 minutes):
  - File cached at 10:00, played again at 10:03 → Uses cache without checking server
  - File cached at 10:00, played again at 10:06 → Uses cache but async checks for updates
- If file is found stale, next play will re-download

See caching.md for detailed cache strategy.

### Security Settings

```json
"security": {
  "allowed_directories": [
    "/opt/sounds",
    "/home/user/audio"
  ]
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_directories` | array | `[]` | Whitelist of directories for local file access. Empty array = no local files allowed |

**Security Notes:**
- Only files within these directories can be played
- Symlinks are resolved and checked
- Path traversal attempts (`../`) are blocked
- HTTP/HTTPS URLs are always allowed (assumes trusted network)

**Example:**
```json
"allowed_directories": [
  "/opt/sounds",
  "~/audio"   // ~ expands to user home directory
]
```

If allowed_directories is empty or not specified, only HTTP/HTTPS URLs can be played.

### Logging Settings

```json
"logging": {
  "level": "info",
  "verbose": false
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `level` | string | `"info"` | Log level: `"error"`, `"warn"`, `"info"`, `"debug"`, `"trace"` |
| `verbose` | boolean | `false` | Legacy verbose mode (logs every play command). Equivalent to `level: "debug"` |

## Command Line Arguments

Command line arguments override configuration file settings.

```
mqttaudio [OPTIONS]

OPTIONS:
  -c, --config <FILE>           Path to configuration file
  -s, --server <HOST>           MQTT server hostname
  -p, --port <PORT>             MQTT server port
  -t, --topic <TOPIC>           MQTT topic to subscribe to
  -d, --device <NAME>           Audio output device name
  -r, --sample-rate <HZ>        Audio sample rate
  -v, --verbose                 Enable verbose logging
      --list-devices            List available audio devices and exit
  -h, --help                    Print help
  -V, --version                 Print version
```

### Examples

**Minimal (using config file):**
```bash
mqttaudio --config /etc/mqttaudio.json
```

**Override MQTT topic:**
```bash
mqttaudio --config config.json --topic "exhibits/audio"
```

**List available audio devices:**
```bash
mqttaudio --list-devices
```

**Full command line (no config file):**
```bash
mqttaudio \
  --server mqtt.example.com \
  --port 1883 \
  --topic "audio/commands" \
  --device "USB Audio Device" \
  --sample-rate 48000 \
  --verbose
```

## Minimal Configuration

The only required setting is the MQTT topic. Minimal config:

```json
{
  "mqtt": {
    "topic": "audio/commands"
  }
}
```

Or via command line:
```bash
mqttaudio --topic "audio/commands"
```

All other settings use sensible defaults.

## Platform-Specific Notes

### Linux
- Default device uses ALSA or PulseAudio (detected automatically)
- For specific ALSA device: `"device": "hw:0,0"` or `"device": "plughw:1,0"`
- Use `aplay -L` to list ALSA devices

### macOS
- Default device uses CoreAudio
- For specific device: Use exact name from Audio MIDI Setup
- Run `mqttaudio --list-devices` to see available devices

### Windows
- Default device uses WASAPI
- For specific device: Use exact name from Windows Sound settings
- Run `mqttaudio --list-devices` to see available devices

## Configuration Validation

On startup, mqttaudio validates the configuration and reports errors:

```
ERROR: Invalid configuration
  - mqtt.topic is required
  - audio.sample_rate must be between 8000 and 192000
  - cache.directory does not exist and could not be created
```

Critical errors will prevent startup. Warnings allow startup but may cause issues.

## Environment Variables

The following environment variables are supported:

| Variable | Description | Default |
|----------|-------------|---------|
| `MQTTAUDIO_CONFIG` | Path to config file | (see search paths above) |
| `MQTTAUDIO_CACHE_DIR` | Override cache directory | `~/.mqttaudio/cache` |
| `RUST_LOG` | Rust logging configuration | `info` |

Example:
```bash
export MQTTAUDIO_CONFIG=/etc/mqttaudio.json
export MQTTAUDIO_CACHE_DIR=/tmp/audio-cache
export RUST_LOG=mqttaudio=debug,rumqttc=warn
mqttaudio
```

## Configuration Hot Reload

**Not supported in v1.0.** If configuration changes, restart mqttaudio.

**Future enhancement:** Watch config file and reload on changes (audio settings may require restart, MQTT settings can reload live).

## Example Configurations

### Simple Stereo Setup
```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "audio/commands"
  }
}
```

### Multi-Channel Exhibition Setup
```json
{
  "mqtt": {
    "server": "exhibit-mqtt.local",
    "topic": "gallery/audio/#"
  },
  "audio": {
    "device": "USB Audio Interface",
    "channel_names": {
      "0": "zone1_left",
      "1": "zone1_right",
      "2": "zone2_left",
      "3": "zone2_right",
      "4": "zone3_left",
      "5": "zone3_right"
    },
    "channel_volumes": {
      "zone3_left": 0.8,
      "zone3_right": 0.8
    }
  },
  "cache": {
    "revalidate_after_seconds": 3600
  },
  "security": {
    "allowed_directories": ["/opt/gallery/sounds"]
  }
}
```

### Development Setup (Fast Cache Revalidation)
```json
{
  "mqtt": {
    "server": "localhost",
    "topic": "dev/audio"
  },
  "cache": {
    "revalidate_after_seconds": 10
  },
  "logging": {
    "level": "debug"
  }
}
```
