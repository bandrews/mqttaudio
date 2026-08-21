# Troubleshooting

Common issues and solutions for mqttaudio.

## No Audio Output

### Check Audio Device

List available devices:
```bash
./mqttaudio --list-devices
```

Specify the correct device:
```bash
./mqttaudio --device "Your Device Name" --topic audio/commands
```

### Check Volume

Make sure volume is set in your play command:
```json
{
  "command": "play",
  "file": "/sounds/test.wav",
  "volume": 1.0
}
```

Also check system volume and that the device isn't muted at the OS level.

### Check Sample Rate

Some devices only support specific sample rates:
```bash
./mqttaudio --sample-rate 44100 --topic audio/commands
```

### Check Logs

Run with verbose logging:
```bash
./mqttaudio --verbose --topic audio/commands
```

Look for errors about devices, files, or playback.

---

## MQTT Connection Issues

### Verify Broker is Running

Test with mosquitto tools:
```bash
# Terminal 1: Subscribe
mosquitto_sub -t test -v

# Terminal 2: Publish
mosquitto_pub -t test -m "hello"
```

### Check Server and Port

```bash
./mqttaudio --server mqtt.example.com --port 1883 --topic audio/commands
```

### Check Topic

Make sure you're publishing to the same topic mqttaudio is subscribed to:
```bash
# mqttaudio subscribes to:
./mqttaudio --topic "audio/commands"

# You must publish to:
mosquitto_pub -t "audio/commands" -m '...'
```

### Network Issues

Try connecting to localhost first to rule out network problems:
```bash
./mqttaudio --server localhost --topic audio/commands
```

---

## Files Not Playing

### Local Files: Check Permissions

If `security.allowed_directories` is configured, the file must live inside
one of the listed directories - a rejected play logs "Play rejected" with
the offending path:

```json
{
  "security": {
    "allowed_directories": ["/opt/sounds", "/home/user/audio"]
  }
}
```

With no `security` section (or an empty list), any file the daemon can read
is playable.

### HTTP Files: Check Network

Test the URL directly:
```bash
curl -I https://example.com/sound.wav
```

### Check File Format

Supported formats: WAV, MP3, OGG, FLAC

Verify the file is valid:
```bash
ffprobe /path/to/file.wav
```

### Check Logs for Decode Errors

```bash
./mqttaudio --verbose --topic audio/commands
```

Look for messages like:
- "Failed to decode..."
- "Unsupported format..."
- "Error opening file..."

---

## Audio Glitches and Dropouts

### Increase Buffer Size

Larger buffers reduce glitches at the cost of latency:

```json
{
  "audio": {
    "buffer_size": 1024
  }
}
```

Common values: 256 (low latency), 512 (default), 1024 (stable), 2048 (very stable)

### Reduce Simultaneous Samples

If you're playing many sounds at once, reduce the count. mqttaudio handles 20+ samples, but your system may have limits.

### Check CPU Usage

Monitor CPU during playback. High CPU usage causes glitches.

```bash
top -p $(pgrep mqttaudio)
```

### Check for Other Audio Applications

Other applications using the audio device can cause conflicts. Close other audio software if possible.

---

## Cache Issues

### Cache Directory Permissions

Check write access:
```bash
ls -la ~/.mqttaudio/cache/
```

Create if needed:
```bash
mkdir -p ~/.mqttaudio/cache
```

### Clear Corrupted Cache

```bash
rm -rf ~/.mqttaudio/cache/*
```

Or via MQTT:
```json
{"command": "cache_clear"}
```

### File Not Updating

If a file on your server changed but mqttaudio plays the old version:

```json
{
  "command": "cache_invalidate",
  "file": "https://example.com/updated.wav"
}
```

---

## Microphone/Input Issues

### Find Your Device

```bash
./mqttaudio --list-inputs
```

### Check Device Name

Device names must match exactly (case-sensitive):
```json
{
  "inputs": [
    {
      "device": "USB Microphone",  // Must match exactly
      ...
    }
  ]
}
```

The index number printed by `--list-inputs` also works: `"device": "0"`.

### Device Listed But "Input device not found"

Input enumeration only shows devices that can be opened for capture at that
moment, so a device visible in an interactive `--list-inputs` can still be
missing when the daemon starts (typically under systemd). The error message
lists the devices that were available and, on Linux, why a direct ALSA capture
open of the requested name fails:

- **Device or resource busy** - another process holds the capture side: a
  sound server (PipeWire, PulseAudio), another capture application, or a
  second copy of the daemon. `fuser -v /dev/snd/*` shows the holder
- **Permission denied** - the daemon's user cannot open the device. For a
  systemd service, add the service user to the `audio` group:
  `sudo usermod -aG audio USER`, then restart the service
- **No such device** - the name does not exist; compare against `arecord -L`

To see exactly what the daemon sees, run the listing as the service user:
```bash
sudo -u SERVICE_USER ./mqttaudio --list-inputs
```

### Check Routing

Verify routes point to valid output channels:
```json
"routes": [
  {"source_channel": 0, "dest_channel": 0}
]
```

### Sample Rate Mismatch

If input and output sample rates differ, you'll see a warning:
```
WARN Input device sample rate differs from output - resampling will add latency
```

This works but adds latency. For best results, use matching sample rates.

On a shared-clock interface (one USB card doing both directions), capture
cannot be forced to a rate the card is not running at. If capture keeps
opening at the wrong rate despite `sample_rate`, something else is holding
the card at that rate - a `dmix`/`dsnoop` device from asound.conf, a sound
server, or another application. Free the card and capture will follow the
output rate. Choppiness with both overruns *and* starvation in the input
health log, alongside ALSA `underrun occurred` messages, points at the
output side stalling (typically a dmix chain) rather than at the mic.

---

## Ducking Not Working

### Check Voice Names

Voice names are case-sensitive. These are different:
- `"voice": "Narration"`
- `"voice": "narration"`

### Verify Rules

Check your config has the right voice names:
```json
{
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music"],
      ...
    }
  ]
}
```

Then play audio with matching voice names:
```json
{
  "command": "play",
  "file": "/sounds/speech.wav",
  "voice": "narration"
}
```

### Check That Primary Voice is Playing

Ducking only triggers when the primary voice has active samples. The voice must be playing something.

---

## Channel Routing Issues

### Check Output Channel Count

```bash
./mqttaudio --list-devices
```

If your device has 2 channels, you can't route to channel 4.

### Verify Channel Map Format

```json
"channel_map": [
  {"src": 0, "dest": 2},
  {"src": 1, "dest": 3}
]
```

Both `src` and `dest` are 0-indexed integers.

### Check Source Channel Exists

A stereo file has channels 0 and 1. You can't route `src: 3` from a stereo file.

---

## Performance Issues

### Check Callback Timing

With `--verbose`, look for timing warnings:
```
WARN Audio callback exceeded budget: 12ms (limit: 10.67ms)
```

### Reduce Load

1. Reduce simultaneous samples
2. Increase buffer size
3. Disable pitch correction (uses more CPU)
4. Use simpler channel routing

### Profile

Build with release optimizations:
```bash
cargo build --release
./target/release/mqttaudio --topic audio/commands
```

Debug builds are significantly slower.

---

## Getting Help

### Gather Information

When reporting issues, include:

1. **mqttaudio version:** `./mqttaudio --version`
2. **Operating system and version**
3. **Audio device:** Output of `--list-devices`
4. **Configuration:** Your config file (remove sensitive data)
5. **Logs:** Output with `--verbose`
6. **Commands:** The MQTT commands you're sending
7. **Expected vs actual behavior**

### Log Locations

mqttaudio logs to stderr by default. Capture logs:
```bash
./mqttaudio --verbose --topic audio/commands 2> mqttaudio.log
```

### Report Issues

File issues at: https://github.com/bandrews/mqttaudio/issues

Include the information above for faster resolution.
