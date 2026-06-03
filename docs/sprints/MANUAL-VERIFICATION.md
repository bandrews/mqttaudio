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

**Expected:** process does **not** panic at startup; logs show the negotiated device format (e.g. I16) and
sample rate; audio plays cleanly with no distortion. Repeat with `--sample-rate 44100` and `--channels 2`.

**Result:** _(record PASS/FAIL + notes + date)_

---

## W-2 — Two-device live input soak (clock drift)  _(added by Sprint 8)_

**Why manual:** requires two independent audio clocks (e.g. a USB mic + a different output interface); the
drift behavior cannot be reproduced with Lane A's synthetic SRC test.

**Steps:**
1. Configure a live input on a separate device from the output (see `docs/features/microphone-input.md`).
2. Run for ≥30 minutes routing the mic into the mix at a steady level.

**Expected:** no periodic clicks/dropouts over the session; ring-buffer fill stays bounded (no monotonic
drift to under/overrun). Also confirm a non-f32-native input device opens and routes audio.

**Result:** _(record PASS/FAIL + notes + date)_

---

## macOS spot-checks (already covered by Lane B; re-confirm only if Lane B was skipped)

- Output format dispatch on the real CoreAudio device (Sprint 1).
- CoreAudio input device opens and routes into the mix (Sprint 8).
- Post-redesign real-device soak (Sprint 5).

**Result:** _(record PASS/FAIL + notes + date)_

---

_Additional checks are appended by sprints as they land. Keep IDs stable (W-3, W-4, …)._
