# Manual Cross-Platform Verification (Windows, with macOS spot-checks)

Automated validation covers **two of three** platforms: Linux (Lane A, Docker) and macOS (Lane B, the dev
machine with a real CoreAudio device). **Windows (WASAPI) has no automated coverage** and must be verified
by the partner. Sprints that change device/format/clock behavior **append** concrete, copy-pasteable steps and
expected results here as they are implemented. The partner runs this whole document once, after the final
sprint, and records pass/fail inline.

> Sprints must not invent new audio-quality claims here — only steps that genuinely require Windows (or real
> multi-device hardware) and cannot be proven in Lane A or Lane B.

## How to run

1. Build on Windows: `cargo build --release` (install the prerequisites in `README.md`).
2. Have a Mosquitto broker reachable (local is fine): `mosquitto -v`.
3. Work top to bottom. For each check, record **PASS/FAIL + notes + date** in the Result line.

---

## W-1 — Output starts on a WASAPI shared-mode (i16/i32) device  _(added by Sprint 1)_

**Why manual:** Windows WASAPI shared mode commonly presents only i16/i32 to applications; the pre-fix code
panicked here. This is the headline compatibility fix and cannot be exercised on Linux/macOS.

**Steps:**
1. `target\release\mqttaudio.exe --list-devices` — confirm the default output device is listed with its
   channels/rate.
2. Start against a broker: `mqttaudio.exe --server localhost --topic audio/commands`
3. Play a known WAV: `mosquitto_pub -t audio/commands -m "{\"command\":\"play\",\"file\":\"C:\\path\\to\\test.wav\"}"`

**Expected:** process does **not** panic at startup; the log line `Sample format: I16` (or `I32`/`F32`)
reports the negotiated device format alongside the channels and rate; audio plays cleanly with no
distortion. Repeat with `--sample-rate 44100` and `--channels 2`. If a second device exposes a different
native format (e.g. I32), repeat against it and confirm the logged `Sample format:` changes accordingly.

**Result:** _(record PASS/FAIL + notes + date)_

---

## W-3 — Output auto-recovers from a device error  _(added by Sprint 1)_

**Why manual:** requires physically removing/re-adding the output device (or switching the default), which
cannot be reproduced in Lane A or Lane B.

**Steps:**
1. Start playback of a looping WAV on a removable output device (USB / Bluetooth / HDMI).
2. While it plays, unplug (or disable) that device, wait ~5 seconds, then re-plug/re-enable it.

**Expected:** the log shows an audio stream error followed by a rebuild attempt ("Audio stream error … /
rebuilding output"); within a few seconds of the device returning, audio resumes on it **without** restarting
the process. If the device stays gone, the process exits non-zero (for a service-manager restart) rather than
hanging silently.

**Result:** _(record PASS/FAIL + notes + date)_

---

## W-2 — Two-device live input soak (clock drift) + non-f32 input  _(added by Sprint 8)_

**Why manual:** the drift correction only has anything to correct when the capture device and the output
device run on **two independent clocks** (e.g. a USB mic plus a *different* output interface). Lane A's
synthetic SRC test drives a single simulated clock pair in-process; it proves the steering law is bounded but
cannot reproduce two real oscillators drifting over tens of minutes. The non-f32 capture path (typed → f32
convert) likewise needs a real device that presents I16/I32, which Windows USB mics commonly do and Lane B's
single CoreAudio device may not.

**Setup — two genuinely separate devices.** Use a USB microphone for the input and a *different* interface
for output (built-in speakers, HDMI, a second USB DAC — anything that is not the same hardware clock as the
mic). Confirm device names with `target\release\mqttaudio.exe --list-inputs` and `--list-devices`. Write a
config (`soak.json`) with **debug logging on** so the ring-overflow diagnostic is visible:

```json
{
  "mqtt": { "server": "localhost", "topic": "audio/commands" },
  "audio": { "device": "<output interface>", "channels": 2 },
  "logging": { "level": "debug" },
  "inputs": [
    {
      "device": "<USB microphone>",
      "volume": 0.7,
      "voice_id": "mic",
      "routes": [
        { "source_channel": 0, "dest_channel": 0 },
        { "source_channel": 0, "dest_channel": 1 }
      ],
      "latency_ms": 25
    }
  ]
}
```

**Steps:**
1. Start: `target\release\mqttaudio.exe --config soak.json`. Confirm the startup log shows the input opening,
   e.g. `Opening input device: <USB microphone> (44100 Hz, 1 channels, I16, 25ms buffer)`. The reported
   **sample format** records what the device presented — if it is `I16` or `I32` (not `F32`), this run also
   covers the non-f32 capture path; if your USB mic reports `F32`, repeat once against any I16/I32 input
   device (most cheap USB mics and interface line-ins are I16/I24→I32) so a non-f32 open is exercised at least
   once.
2. Speak into / play steady audio through the mic and confirm it is audible in the output, panned to both
   channels (the route above) — this is the "opens **and routes**" half of the check.
3. Leave it running, mic into the mix at a steady level, for **≥30 minutes**. Keep the console visible.

**Expected:**
- **No periodic dropouts.** No recurring clicks/gaps every few seconds-to-minutes over the whole session. (A
  drifting ring with no steering would produce a regular tick as it periodically over/underruns; the steered
  converter must prevent that.)
- **Ring fill stays bounded.** The debug log must **not** accumulate `Resampler output overflow: N samples
  dropped` lines — that message fires only if the ring is being driven to overflow (steering failed to hold
  it). A bounded ring logs it zero times (or, at most, a one-off near startup before the fill EMA settles,
  never a steady stream). There is no live ring-fill gauge in `/status`; absence of this line plus no audible
  dropouts is the bounded-fill evidence.
- **Non-f32 input.** The I16/I32 device from step 1 opened without a `Stream error` / `Unsupported input
  sample format` and its audio routed cleanly (no zero-output, no garbled/clipped conversion).

**Result:** _(record PASS/FAIL + notes + date — note the device names and the logged input sample format)_

---

## macOS spot-checks (already covered by Lane B; re-confirm only if Lane B was skipped)

- Output format dispatch on the real CoreAudio device (Sprint 1).
- CoreAudio input device opens and routes into the mix (Sprint 8).
- Post-redesign real-device soak (Sprint 5).

**Result:** _(record PASS/FAIL + notes + date)_

---

_Additional checks are appended by sprints as they land. Keep IDs stable (W-3, W-4, …)._

- **Sprint 13 note:** the existing real-device listening soak now also covers the pitch-toggle path (enable/disable pitch correction on a live voice mid-play while listening for dropouts/clicks) and a scripted burst of >256 plays (`xruns` must stay 0; the oldest non-looping sounds cut out by design).
