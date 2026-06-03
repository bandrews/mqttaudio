# Bass Management

Bass management extracts low frequencies from main channels and routes them to a dedicated subwoofer (LFE) channel. This is essential for professional installations with separate subwoofers.

## How It Works

1. Audio plays through the mixer as normal
2. A 4th-order Linkwitz-Riley low-pass extracts frequencies below the crossover point
3. The extracted bass from every source channel is summed, normalized by the source count, and sent to the LFE channel
4. By default, bass is removed from the source channels (a matching 4th-order Linkwitz-Riley high-pass); set `remove_bass_from_sources: false` to leave the mains full-range

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
| `remove_bass_from_sources` | Remove bass from the source channels after extraction. **Default `true`** (standard bass management); set `false` for the additive "LFE+Main" mode |
| `lfe_gain` | Linear trim applied to the summed LFE (default `1.0`). The LFE is normalized by the source count first, so this just matches sub level to the room |

> **Default change:** `remove_bass_from_sources` now defaults to `true`. The bass routed to the sub is
> removed from the main channels, as in standard bass management. For the older additive behavior —
> full-range mains *and* the same bass duplicated in the sub — set `remove_bass_from_sources: false`
> ("LFE+Main", see the Bass Copy example).

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

## LFE Routing Behavior

A few things to know about how content reaches the LFE channel:

- **Count-normalized level.** The bass extracted from each source channel is summed and then divided by the
  number of active source channels, so the sub level does not scale with how many channels feed it. Feeding
  correlated bass from two channels gives the same sub level as one (it is **not** +6 dB louder). Use
  `lfe_gain` to trim the result.
- **The LFE index is added to, not replaced.** Extracted bass is *summed onto* whatever is already on the
  `lfe_channel`. If another voice routes full-range material directly to that output index (e.g. via a Play
  `channel_map`), the LFE carries that directly-routed content **plus** the extracted bass — the directly
  routed content is not crossed over. This is the additive-LFE behavior; route content to the LFE index
  deliberately.
- **Out-of-range LFE is a no-op (with a warning).** If `lfe_channel` is greater than or equal to the device's
  output channel count, bass management cannot redirect anything and does nothing; a one-time warning is
  logged at startup. The mains are left full-range (no bass is lost, but none is redirected either).

## Technical Details

- 4th-order Linkwitz-Riley crossover (two cascaded Butterworth biquads per filter), so the low- and
  high-pass outputs are in phase and recombine flat through the crossover (24 dB/oct slopes)
- Low-pass filter for LFE extraction; matching high-pass for source-channel bass removal (when enabled)
- The filter state is flushed to zero once it decays below an inaudible threshold, keeping the IIR tail out
  of the (CPU-expensive) floating-point denormal range on the audio thread
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
