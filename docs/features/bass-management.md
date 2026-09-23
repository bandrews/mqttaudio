# Bass Management

Bass management sends the low frequencies of chosen output channels to a subwoofer channel, so small
main speakers do not have to reproduce deep bass. It works on the finished mix, after every sound and
live input has been routed.

## Turning it on

```json
"audio": {
  "channel_aliases": {"front_left": 0, "front_right": 1, "center": 2, "lfe": 3, "rear_left": 4, "rear_right": 5}
},
"bass_management": {
  "enabled": true,
  "lfe_channel": "lfe",
  "source_channels": ["front_left", "front_right", "center", "rear_left", "rear_right"],
  "crossover_frequency_hz": 80
}
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `enabled` | `false` | Turn bass management on |
| `lfe_channel` | `3` | The subwoofer output (number or alias) |
| `source_channels` | `[]` | Outputs to take bass from; required. No duplicates, and not the LFE channel |
| `crossover_frequency_hz` | `80` | Where bass ends, `10`–`200` Hz |
| `remove_bass_from_sources` | `true` | Also filter the bass out of the source channels |
| `lfe_gain` | `1.0` | Level of the extracted bass on the subwoofer, `0.0`–`8.0` |

There are no command-line options for bass management.

## What it does

For every audio block:

1. Each source channel passes through a low-pass filter at the crossover frequency.
2. The filtered bass of all source channels is added up, divided by the number of source channels,
   multiplied by `lfe_gain`, and added to the LFE channel.
3. With `remove_bass_from_sources` on, each source channel is replaced by its high-passed version, so
   the bass plays only from the subwoofer. Off, the mains stay full-range and the subwoofer adds to
   them.

The filters are 4th-order Linkwitz-Riley (24 dB per octave), so the low and high halves add back up
flat across the crossover.

After bass management, `audio.channel_volumes` trims each output, including the LFE channel.

### Level on the subwoofer

Because the sum is divided by the number of source channels, bass that is on every source channel
(a full-range mix) reaches the subwoofer at its original level, however many channels there are. Bass
on only some of them arrives quieter: a sound playing on one channel of five reaches the subwoofer at
a fifth of its level (-14 dB), and with `remove_bass_from_sources` on it is also removed from that
channel. If your content often plays on a few channels, list only those channels, set
`remove_bass_from_sources` to `false`, or raise `lfe_gain`.

### Content routed straight to the LFE channel

A sound or input routed directly to the LFE channel reaches the subwoofer full-range: it bypasses
the crossover, and the extracted bass is added on top. Route there only content made for the
subwoofer. A configured input route to the LFE channel logs a warning at startup.

### Channels the device does not have

If `lfe_channel` is not on the device, bass management does nothing and a warning is logged at
startup. Source channels beyond the device's channel count are skipped, also with a warning.

## Examples

**5.1** with the standard layout: the configuration above, which is also the
[Configuration](../configuration.md#examples) example.

**Stereo with a subwoofer** on output 2, mains kept full-range:

```json
"bass_management": {
  "enabled": true,
  "lfe_channel": 2,
  "source_channels": [0, 1],
  "remove_bass_from_sources": false
}
```

**Small satellites** that need help higher up:

```json
"bass_management": {
  "enabled": true,
  "lfe_channel": 3,
  "source_channels": [0, 1, 2, 4, 5],
  "crossover_frequency_hz": 120,
  "lfe_gain": 1.4
}
```

## Choosing a crossover

| Crossover | Suits |
|-----------|-------|
| 60 Hz | Large main speakers |
| 80 Hz | Most installations (the usual starting point) |
| 100–120 Hz | Small or satellite speakers |

The config editor's device picker plays a 50 Hz tone through the real signal path, so you can check
that bass on a source channel reaches the subwoofer before going live.

## Troubleshooting

- **No bass from the subwoofer:** check that `enabled` is `true`, that `lfe_channel` is the output
  wired to the subwoofer, and that the startup log has no warning about the LFE channel being out of
  range.
- **Thin mains:** lower the crossover, or set `remove_bass_from_sources` to `false`.
- **Too much or too little bass:** adjust `lfe_gain`, or the subwoofer's own level. A boost in
  `audio.channel_volumes` for the LFE channel also raises content routed there directly.
- **Bass sounds hollow at the crossover:** try reversing the subwoofer's polarity at the amplifier.
