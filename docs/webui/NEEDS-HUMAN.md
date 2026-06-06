# Needs Human — escalation register (last resort)

Add an entry here **only** when something is uncovered that genuinely requires human intervention and has **no
technical resolution available to you** — after exhausting `DECISIONS.md`, the code/`API-CONTRACT.md`, and
reasonable defaults.

Adding an entry does **not** stop the sprint. Log it, skip **only** the blocked item, and complete everything
else possible. Leave the corresponding acceptance box unchecked with a pointer to the entry here. This is a last
resort, not a convenience — picking a defensible default or overruling a locked decision with new evidence (see
`DECISIONS.md`) is almost always the better move than escalating.

If this file has open entries when the program finishes, surface them in the final summary.

## How to add an entry

```
### [Sprint WNN] <short title>
- **Blocked item:** <the specific task / acceptance box>
- **Why it needs a human:** <a decision/credential/design/policy only a human can resolve>
- **What I tried:** <DECISIONS.md refs, code paths, defaults considered, why none suffice>
- **Options for the human:** <A / B / C — with your recommendation>
- **Proceeded without it:** <the rest of the sprint you completed around this>
- **Status:** Open
```

---

### [Sprint 10] Specific device-detection bug repro not supplied
- **Blocked item:** Sprint 10 acceptance box 2 — "The discovered device-detection bug is **reproduced** (a
  failing/asserting test or a documented repro), **root-caused**, and fixed against the new cpal." The
  *reproduced → root-caused* half of that box.
- **Why it needs a human:** The bonus was requested as "bump CPAL to the latest version to resolve a device
  detection bug that has been discovered." The exact failing case (which device, which host/config, observed
  vs expected behavior) was never described to me, so I cannot write a *before* repro that fails on `0.15` and
  passes on `0.17`, nor confirm a specific root cause — that requires the discoverer's reproduction details or
  the hardware that misbehaved.
- **What I tried:** Inspected the whole device-detection surface (`engine.rs` `find_output_device`, `input.rs`
  `get_input_device`, the `--list-devices`/`--list-inputs` paths, `device_select.rs`). The cpal-0.15→0.17 bump
  itself reworks device identity: names now come from `Device::description()` and a stable `Device::id()` is
  available, replacing the unreliable/deprecated `Device::name()` — the most likely class of a "device not
  detected / mis-named" bug. Default considered: invent a plausible repro (rejected — would be fabricating a
  before/after I cannot actually demonstrate, against the honesty bar).
- **What I shipped instead (the assertion branch of the same box):** Real-device (`[RB]`) regression guards in
  `tests/device_smoke_test.rs` — `output_device_detection_round_trips_by_name` /
  `input_device_detection_round_trips_by_name` — assert that the name enumeration reports round-trips back to a
  device through the selection path, i.e. selecting `audio.device` by its enumerated name detects a present
  device on the new cpal. Verified green on the real macOS CoreAudio device, alongside the existing
  open-and-run smoke tests. The acceptance box's documented "**or** a `--list-devices`/`--list-inputs`
  assertion covers it" alternative is therefore satisfied.
- **Options for the human:** (A, recommended) Treat the bump + the round-trip assertions as sufficient and
  close this — the upgrade is the fix vehicle and the detection path is now guarded. (B) Send the exact repro
  (device, host, config, symptom) and I'll add a targeted failing-then-passing test for that specific case.
- **Proceeded without it:** Everything else in Sprint 10 is complete — cpal bumped to `0.17`, full API
  migration, warning-free `-D warnings` + clippy, full audio suite + alloc harness green, real-device
  enumeration + open-and-run + name round-trip verified, docs updated.
- **Status:** Open
