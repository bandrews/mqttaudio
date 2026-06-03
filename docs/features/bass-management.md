# Bass Management

Bass management extracts low frequencies from main channels and routes them to a dedicated subwoofer (LFE) channel. This is essential for professional installations with separate subwoofers.

## How It Works

1. Audio plays through the mixer as normal
2. A low-pass filter extracts frequencies below the crossover point
3. Extracted bass is summed and sent to the LFE channel
4. Optionally, bass is removed from the source channels (high-pass filtered)

## Configuration

Add bass management to your config file:

```json
{
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2, 4, 5],
    "remove_bass_from_sources": false
  }
}
```

| Field | Description |
|-------|-------------|
| `enabled` | Enable/disable bass management |
| `lfe_channel` | Output channel for the subwoofer (0-indexed) |
| `crossover_frequency_hz` | Frequency cutoff (typically 80-120 Hz) |
| `source_channels` | Channels to extract bass from |
| `remove_bass_from_sources` | Remove bass from source channels after extraction |

## CLI Options

Override config from the command line:

```bash
./mqttaudio --lfe-channel 5 --crossover-frequency 100
```

## Examples

### Standard 5.1 Setup

Channel layout:
- 0: Front Left
- 1: Front Right
- 2: Center
- 3: LFE (Subwoofer)
- 4: Surround Left
- 5: Surround Right

```json
{
  "bass_management": {
    "enabled": true,
    "lfe_channel": 3,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1, 2, 4, 5],
    "remove_bass_from_sources": true
  }
}
```

This extracts bass from all main channels, sends it to channel 3, and removes bass from the source channels (standard 5.1 bass management).

### Bass Copy (No Removal)

For systems where speakers can handle full-range and you just want additional bass reinforcement:

```json
{
  "bass_management": {
    "enabled": true,
    "lfe_channel": 7,
    "crossover_frequency_hz": 100,
    "source_channels": [0, 1],
    "remove_bass_from_sources": false
  }
}
```

Bass is copied to the subwoofer but left in the main speakers.

### Stereo with Subwoofer

Simple stereo setup with added subwoofer on channel 2:

```json
{
  "bass_management": {
    "enabled": true,
    "lfe_channel": 2,
    "crossover_frequency_hz": 80,
    "source_channels": [0, 1],
    "remove_bass_from_sources": false
  }
}
```

## Crossover Frequency

The crossover frequency determines what counts as "bass":

| Frequency | Use Case |
|-----------|----------|
| 60 Hz | Large full-range speakers, minimal subwoofer use |
| 80 Hz | Standard home theater, most installations |
| 100 Hz | Smaller speakers, more subwoofer contribution |
| 120 Hz | Small satellite speakers, maximum bass redirection |

## Technical Details

- Uses 2nd-order Butterworth filters for smooth response
- Low-pass filter for LFE extraction
- High-pass filter for source channel bass removal (when enabled)
- Processing happens in real-time with minimal latency

## Tips

1. **Match your speakers** — Set crossover based on your main speakers' low-frequency capabilities
2. **Start with 80 Hz** — The industry standard works well for most setups
3. **Test with music** — Use familiar music to verify the blend sounds natural
4. **Check phase** — If bass sounds thin, try flipping subwoofer polarity at the amp

## Troubleshooting

**No bass in subwoofer:**
- Verify `enabled` is `true`
- Check `lfe_channel` matches your physical wiring
- Confirm `source_channels` includes channels with bass content

**Too much bass:**
- Lower the crossover frequency
- Reduce subwoofer volume at the amplifier

**Thin sound from main speakers:**
- Set `remove_bass_from_sources` to `false`
- Or lower the crossover frequency
