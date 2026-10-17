# Sprint 5 — Transport, Speed & Windowed Gating

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0, 2 |
| Effort | M |
| Lanes | A, B |
| Subagents | Optional (transport / speed / strips parallelize) |

## Goal

Give the operator direct, per-sample manipulation of audio that is already playing: a **seek scrubber**
over the sample's duration, a **speed control** with a 1.0 detent, reverse, and a pitch-correction toggle,
plus the **voice** and **input control strips** (live volume, fade-out, mute, stop). Every control honors
the daemon's exact constraints — streamed (windowed) voices are forward-only, and the speed range differs
depending on whether pitch correction is on. "Done and correct" means each active sample card exposes a
scrubber over `total_ms` that emits `seek`, a speed control spanning −100..100 with a snap-to-1.0 detent
and reverse, a pitch toggle that re-clamps the range to 0.05..8.0 and disables reverse, a "streamed" badge
that disables the unsupported transport controls on windowed voices, and voice/input strips wired to
`voice_volume`/`voice_fade_out`/`voice_stop` and `input_volume`/`input_mute`. Against a live daemon, moving
any of these audibly changes playback and the dashboard reflects it within a poll.

This builds on the typed API client and the per-command validation from Sprint 0, and on the now-playing
board, voices rack, and inputs rack from Sprint 2 (`/status/samples`, `/status/voices`, `/status/inputs`).
It must not break those reads: the scrubber renders over the existing `total_ms` field and stays **set-only**
because there is no live playhead yet (`position`/`position_ms`/`progress_percent` are hard-coded `0` until
Sprint W6 telemetry — API-CONTRACT §5). It must not regress the Sprint 2 "live position unavailable"
messaging by faking a position from the scrubber's own value.

## Why

Sprint 2 made playback *observable*; Sprint 3's bench can *launch* commands; but neither gives an operator
the live, direct controls a show needs — scrub to a moment, re-speed or reverse a bed, duck a voice, mute a
mic. Those `seek`/`speed`/`voice_*`/`input_*` commands exist in the daemon and in the Sprint 0 client, but
nothing in the UI surfaces them on a per-sample, per-voice, per-input basis, and nothing enforces their
real constraints. The constraints are exact and easy to get wrong: streamed/windowed voices are
**forward-only** — the daemon rejects `seek` on `mode:stream` and the streamed path cannot reverse or
loop-crossfade — and the speed range *changes* when pitch correction is toggled (no pitch → −100..100 with
negative meaning reverse; with pitch → 0.05..8.0 and no reverse), clamped at `mixer.rs:369-379`. A UI that
lets the operator drag a streamed voice's scrubber, or request a reverse speed under pitch correction, is
lying about what the daemon will do.

A *real* live playhead needs the Sprint W6 position atomics, so this sprint deliberately ships the scrubber
as **set-only** (emit `seek` on release; no live tracking thumb). Closing the playhead gap is W6's job; this
sprint closes the *control* gap. It also surfaces a discovered hole — `/status/samples` carries no
per-sample "is-windowed" flag today — which forces a client-side inference here and is properly fixed by
Sprint W6 F4.

## Scope

**In scope**

- **Per-sample seek scrubber (set-only).** A slider over `total_ms` that emits `seek {position_ms}` against
  the sample's selector. No live playhead until Sprint W6.
- **Speed control.** A −100..100 range with a snap-to-1.0 detent and reverse (negative); a pitch-correction
  toggle that re-clamps the visible range to 0.05..8.0 and disables reverse. Near-zero speeds are coerced
  server-side (`mixer.rs:369-379`); the UI mirrors the clamp, the daemon is the authority.
- **Windowed gating.** A "streamed" badge on windowed voices that disables `seek`, `speed`, reverse, and
  loop-crossfade controls (forward-only). Because `/status/samples` has **no** is-windowed flag today, infer
  from the play mode at launch, track it client-side, and document the inference; Sprint W6 F4 adds the real
  flag.
- **Voice strips.** Per-voice `voice_volume` (0..1), `voice_fade_out` (by `time_ms` on the typed REST body),
  and `voice_stop`.
- **Input strips.** Per-input `input_volume` and `input_mute` (mute restores the prior level, not a
  hard-coded 1.0); `input` is a string id/index.

**Out of scope** (named owner — coordinate, do not duplicate)

- **A live playhead / real sample position** — owned by **Sprint W6** (Telemetry I). The scrubber is set-only
  here; W6 lights up the live thumb. Do not fake a position from the scrubber value.
- **Output / input meters** — owned by **Sprint W7** (Telemetry II). The input strip shows volume/mute only,
  no capture-level meter.
- **Config editing** (channel aliases, calibration, ducking rules, limiter, input device definitions) —
  owned by **Sprint W8**. The input *strip* changes only the genuinely-live per-input volume/mute (DW8); it
  does not edit config-only state.
- **The channel-map matrix** — owned by **Sprint W4**; this sprint adds no routing UI.

## Work items

### F1 — Seek scrubber (set-only)

- **Statement:** Each active sample card carries a horizontal slider spanning `0..total_ms` that, on commit,
  emits `seek {position_ms, <selector>}`. The thumb does **not** track live position (none exists yet); it is
  a set-only control with the Sprint 2 "live position unavailable" note retained.
- **Where:** `webui/src/features/transport/SampleTransport.tsx` (new component, rendered per active sample on
  the now-playing board). Emits via the Sprint 0 API client's `seek` command. Daemon side: `seek` clamps and
  is **unsupported on `mode:stream`** (API-CONTRACT §2; selector OR-logic §3).
- **Priority:** high — the most-requested live control with no coverage yet.
- **Rationale:** Sprint 2 shows `total_ms` but offers no way to act on it; `seek` exists in the client and the
  daemon but is unreachable from the UI. Because `position_ms` is hard-coded `0` (`handlers.rs:721-730`), a
  live tracking thumb would be a fabrication — DW3/the Charter forbid faking unavailable data.
- **Approach:** Render an MUI `Slider` bound to a local committed value, max = `total_ms`. Emit on
  `onChangeCommitted` (drag-release), not on every `onChange`, so we send one `seek` per gesture. Resolve the
  selector from the sample's `internal_id` (stringified per §3). Label the control "Seek (set-only — live
  position arrives in telemetry)". Disable entirely when the sample is windowed (see F3). Do **not** derive or
  display a progress percentage from the slider value.

### F2 — Speed control

- **Statement:** A speed control whose range and capabilities are gated by a pitch-correction toggle. With
  pitch **off**: range −100..100, a snap-to-1.0 detent, negative values mean reverse. With pitch **on**:
  range re-clamps to 0.05..8.0, reverse is disabled, negative input is not selectable. Emits
  `speed {speed, pitch_correction, <selector>}`.
- **Where:** `webui/src/features/transport/SpeedControl.tsx` (new, rendered on the same sample card as F1).
  Daemon clamp authority at `mixer.rs:369-379` — pitch branch clamps `0.05..8.0` and **rejects** negative
  speed (returns false, logs "Negative speed … not supported with pitch correction"); no-pitch branch clamps
  `-100.0..100.0` and coerces `|speed| < 0.01` to ±0.01.
- **Priority:** high.
- **Rationale:** The two ranges are a real, code-verified divergence (API-CONTRACT §2; `mixer.rs:369-379`). A
  single fixed-range control would either forbid reverse (wrong when pitch is off) or offer reverse under
  pitch correction (which the daemon silently rejects). The 1.0 detent matters because passing through unity
  is the common operation and an analog slider makes it hard to hit exactly.
- **Approach:** Compose an MUI `Slider` plus a `Switch`/`Checkbox` for `pitch_correction`. Drive the slider's
  `min`/`max`/`marks` from the toggle: `{min:-100, max:100}` with a mark + snap at `1.0` when pitch is off;
  `{min:0.05, max:8.0}` with the reverse affordance removed when pitch is on. Implement the detent by snapping
  committed values within a small epsilon of `1.0` to exactly `1.0`. Emit `pitch_correction` alongside `speed`
  every time so the daemon picks the correct branch. Mirror the daemon's near-zero coercion in the displayed
  value but treat the daemon as the source of truth — do not block submission on the client clamp. When pitch
  flips on while a negative speed is showing, re-clamp the local value into `0.05..8.0` before the next emit so
  the UI never sends a value the daemon will reject. Disable entirely on windowed voices (F3).

### F3 — Windowed gating

- **Statement:** Streamed/windowed voices (mode `stream`, auto-windowed large plays, or live HTTP without a
  `Content-Length`) are **forward-only**. Such samples show a "streamed" badge and have `seek`, `speed`,
  reverse, and loop-crossfade controls disabled.
- **Where:** `webui/src/features/transport/SampleTransport.tsx` (the badge + disabled state) and the sample
  model the now-playing board builds from `/status/samples` (the `isWindowed` derivation). **GAP:**
  `/status/samples` exposes no per-sample is-windowed flag today (API-CONTRACT §5 lists
  `internal_id,id,voice,file,total_frames,total_ms,sample_rate,volume,voice_volume,speed,loop_mode` — no
  windowed/mode field), so the flag must be inferred client-side from the play mode at launch.
- **Severity:** high (correctness — without it the UI offers controls the daemon rejects).
- **Evidence:** `seek` is "Unsupported on `mode:stream` (forward-only)" and the streamed path drops features
  the full-load path keeps (API-CONTRACT §2, §4 — e.g. per-route gain is ignored on `mode:stream`). There is
  no status field to read the windowed state back from, confirmed against the `/status/samples` payload list.
- **Approach:** When the UI launches a play (Sprint 3 bench / cue launcher) it already knows the requested
  `mode` and window options; record an `isWindowed` hint keyed by the play `id` (or the returned
  `internal_id` once observed on the next `/status/samples` poll) in view-local state, and join it onto the
  active-sample model. Treat a play as windowed when `mode === "stream"`, when auto-windowing thresholds were
  hit, or when it is a live HTTP source without `Content-Length`. Render a "streamed" `Chip`/badge on the card
  and pass a `windowed` prop that disables F1's scrubber and F2's speed/reverse controls and any
  loop-crossfade affordance. **Document the inference** inline (this is a heuristic, not authoritative) and
  **log the gap in `docs/bugs.md` as `Sprint W5 F3`** — `/status/samples` needs a real `is_windowed` flag,
  added by **Sprint W6 F4**. Until then, samples the UI did not launch (no client-side hint) default to
  *not* windowed; surface that limitation in the badge's tooltip rather than guessing.

### F4 — Voice strips

- **Statement:** Each voice on the voices rack gets a control strip exposing volume (`voice_volume`, 0..1),
  fade-out, and stop (`voice_stop`). Fade-out uses the **typed REST key `time_ms`** even though the wire key
  is `time`.
- **Where:** `webui/src/features/voices/VoiceStrip.tsx` (new, rendered per voice from `/status/voices`).
  Commands: `voice_volume`/`voice_fade_out`/`voice_stop` via the Sprint 0 client. Transport quirk:
  `voice_fade_out` wire key is `time`; the typed `/voice/fade_out` body key is **`time_ms`**
  (API-CONTRACT §5; `handlers.rs:550`).
- **Priority:** high.
- **Rationale:** Sprint 2's voices rack *shows* per-voice volume and the ducking multiplier but offers no way
  to change them; the `voice_*` commands are reachable in the client but not surfaced per voice. Getting the
  `time_ms` vs `time` key wrong is exactly the kind of silent transport bug the API contract calls out.
- **Approach:** Render per voice: an MUI `Slider` (0..1) emitting `voice_volume {voice, volume}` on commit; a
  small fade-out control (duration input in ms → emit `voice_fade_out {voice, time_ms}` using the typed key,
  which the Sprint 0 client maps to the wire `time`); and a `voice_stop {voice}` button. Use the voice `id`
  from `/status/voices` directly as the `voice` selector. Keep the existing "ducked" indicator from Sprint 2
  intact — do not overwrite `ducking_multiplier` display with the strip's volume.

### F5 — Input strips

- **Statement:** Each live input gets a control strip exposing volume (`input_volume`) and mute
  (`input_mute`). Mute restores the input's **prior** level on unmute (not a hard-coded 1.0). `input` is a
  string id/index.
- **Where:** `webui/src/features/inputs/InputStrip.tsx` (new, rendered per input from `/status/inputs`).
  Commands: `input_volume`/`input_mute` via the Sprint 0 client. Daemon: mute holds volume at 0.0 and
  restores `pre_mute_volume` (the calibrated level the input had when muted), not 1.0 (`mixer.rs:670` region,
  D34). `input` is a string — index `"0"` or a `voice_id`; a bare number errors (API-CONTRACT §2).
- **Priority:** high.
- **Rationale:** Sprint 2's inputs rack shows volume + `muted` but offers no control; `input_*` are reachable
  in the client but unsurfaced. The mute-restores-prior-level behavior is daemon-side (D34) — the UI must not
  imply unmute jumps to full scale, and must not send a bare numeric `input`.
- **Approach:** Render per input: an MUI `Slider` (0..1) emitting `input_volume {input, volume}` on commit;
  and a mute toggle emitting `input_mute {input, mute}` (typed `/input/mute` defaults `mute:false`, so always
  send the explicit boolean — API-CONTRACT §5). Build the `input` selector as the **string** index
  (`String(index)`) or the `voice_id` from `/status/inputs`; never a raw number. Reflect the daemon's
  `muted` flag (computed as `volume == 0.0`) from the next poll rather than tracking mute state purely
  locally — the restore-to-prior-level is the daemon's job, so let the poll report the restored volume.

## Caveats (do not chase ghosts / do not break)

- **The scrubber is set-only — there is NO live playhead yet.** `position`/`position_ms`/`progress_percent`
  are hard-coded `0` until Sprint W6 (`handlers.rs:721-730`). Do **not** synthesize a live thumb from the
  slider value, and do **not** replace Sprint 2's "live position unavailable" messaging with a fake percent.
- **Streamed = forward-only — gate the controls, don't just hide the badge.** A windowed voice must have
  `seek`, `speed`, reverse, and loop-crossfade actually **disabled**, not merely visually flagged. The daemon
  rejects `seek` on `mode:stream` and the streamed path cannot reverse/loop-crossfade.
- **The windowed flag is INFERRED, not read.** `/status/samples` has no is-windowed field today. The
  inference is a heuristic keyed by the play the UI launched; samples the UI did not launch default to
  not-windowed. Do **not** present the inference as authoritative, and **do** log the gap to `docs/bugs.md`
  (`Sprint W5 F3`). The real flag is Sprint W6 F4 — do not build it here.
- **`voice_fade_out` uses `time_ms` on the typed REST body, `time` on the wire.** Emit `time_ms` through the
  client; do not send `time` to the typed endpoint and do not send `time_ms` over a raw/flattened path.
- **`input` is a string.** Send `"0"` / a `voice_id`, never a bare number — a numeric `input` errors.
- **Mute restores the prior level (D34), not 1.0.** Do not hard-code an unmute target; let the daemon restore
  `pre_mute_volume` and read it back from the poll.
- **Speed ranges are NOT symmetric across the toggle.** Pitch on ⇒ 0.05..8.0, no reverse; pitch off ⇒
  −100..100, reverse. Do not reuse one slider config for both states, and never emit a negative speed with
  `pitch_correction:true`.
- **Selector OR-logic + empty-selector no-op.** Every emit must carry a real selector (the sample's
  `internal_id`, or the voice/input id). An empty selector is a silent no-op (API-CONTRACT §3) — never emit a
  transport command with no selector.
- **No daemon changes this sprint.** This drives existing commands only; do not touch the RT path, the mixer,
  or any Rust handler. (The is-windowed flag is W6's Rust work, not this sprint's.)

## Tasks (ordered, TDD-first)

Subagents may split along F1/F2 (transport), F4 (voice strips), and F5 (input strips), with F3 (windowed
gating) landing last because it depends on the F1/F2 components existing to disable. Each behavior task writes
a **failing Vitest + React Testing Library** test first, confirms it fails, writes the minimum component code
to pass, then confirms green. The Lane B item is a real-daemon check, not a unit test.

1. **Seek scrubber emits one `seek` per gesture (F1).** Write a failing RTL test that renders
   `SampleTransport` for an active sample with `total_ms`, drags/commits the slider, and asserts exactly one
   `seek {position_ms, internal_id}` is emitted on commit (not on intermediate `onChange`), and that no
   progress percent is derived from the slider. Confirm it fails, implement the `Slider` +
   `onChangeCommitted` wiring, confirm green.

2. **Speed control: no-pitch range + 1.0 detent + reverse (F2).** Write a failing test asserting that with
   pitch off the slider spans −100..100, a commit near 1.0 snaps to exactly `1.0`, a negative commit emits a
   negative `speed` with `pitch_correction:false`. Confirm fail, implement, confirm green.

3. **Speed control: pitch-on re-clamp + no reverse (F2).** Write a failing test asserting that toggling pitch
   **on** re-clamps the range to 0.05..8.0, removes the reverse affordance, re-clamps a currently-negative
   value into range before the next emit, and that every emit carries `pitch_correction:true`. Confirm fail,
   implement, confirm green.

4. **Voice strip emits `voice_volume` / `voice_fade_out` (time_ms) / `voice_stop` (F4).** Write a failing test
   asserting volume commit emits `voice_volume {voice, volume}`, the fade control emits
   `voice_fade_out {voice, time_ms}` (assert the **`time_ms`** key, not `time`), and stop emits
   `voice_stop {voice}`. Confirm fail, implement `VoiceStrip`, confirm green.

5. **Input strip emits string `input` + explicit `mute` (F5).** Write a failing test asserting volume commit
   emits `input_volume {input, volume}` with `input` a **string** (`"0"`), and the mute toggle emits
   `input_mute {input, mute:<bool>}` with the explicit boolean. Assert no bare-number `input` is ever sent and
   the strip reads `muted` back from props rather than hard-coding an unmute level. Confirm fail, implement
   `InputStrip`, confirm green.

6. **Windowed gating disables transport + shows the badge (F3).** Write a failing test that renders
   `SampleTransport` with `windowed=true` and asserts the "streamed" badge is present and the scrubber, speed,
   reverse, and loop-crossfade controls are **disabled**; and a second test asserting a non-windowed sample
   leaves them enabled. Confirm fail, implement the `windowed` prop + badge + disabled wiring, confirm green.

7. **Windowed inference + gap log (F3).** Write a failing test for the client-side `isWindowed` derivation:
   a play launched with `mode:"stream"` (or auto-window thresholds / live-HTTP-no-Content-Length) yields
   `isWindowed=true` on the joined sample model; a normal full-load play yields `false`; a sample with no
   client-side launch hint defaults to `false`. Confirm fail, implement the derivation + the model join,
   confirm green. Add the `Sprint W5 F3` entry to `docs/bugs.md` recording the missing `/status/samples`
   is-windowed flag and pointing to Sprint W6 F4.

8. **Playwright headless smoke (Lane A).** Add a Playwright spec against the mock/fixture backend that opens a
   sample card, exercises seek + speed + pitch toggle, opens a voice strip and an input strip, and asserts the
   captured requests carry the correct command JSON (including `time_ms`, the string `input`, and
   `pitch_correction`). Confirm green headless.

9. **Lane B — live daemon walkthrough.** Against a real `mqttaudio` daemon behind the sidecar, verify
   seek/speed/voice/input controls change playback **audibly** and the dashboard reflects them within a poll,
   and that a windowed play shows the badge with transport disabled. Record steps for `MANUAL-VERIFICATION.md`
   if any cross-browser nuance appears.

## Files to create / touch

**Create**
- `webui/src/features/transport/SampleTransport.tsx` — per-sample card: seek scrubber + windowed badge/gating.
- `webui/src/features/transport/SpeedControl.tsx` — speed slider + pitch-correction toggle + reverse/detent.
- `webui/src/features/voices/VoiceStrip.tsx` — per-voice volume / fade-out (`time_ms`) / stop.
- `webui/src/features/inputs/InputStrip.tsx` — per-input volume / mute (string `input`).
- `webui/src/features/transport/SampleTransport.test.tsx`,
  `webui/src/features/transport/SpeedControl.test.tsx`,
  `webui/src/features/voices/VoiceStrip.test.tsx`,
  `webui/src/features/inputs/InputStrip.test.tsx` — Vitest + RTL specs.
- `webui/e2e/transport-and-strips.spec.ts` — Playwright headless smoke against the mock backend.

**Touch**
- The now-playing board / active-sample model from Sprint 2 (e.g. `webui/src/features/dashboard/…`) to mount
  `SampleTransport` per active sample and to join the `isWindowed` hint onto the sample model.
- The voices rack / inputs rack from Sprint 2 to mount `VoiceStrip` / `InputStrip` per row.
- The view-local launch-hint store (Zustand/Context, per DW4) to record the per-play `mode`/window options
  used by F3's inference.
- `docs/bugs.md` — add `Sprint W5 F3`: `/status/samples` has no is-windowed flag; inference is a heuristic;
  closed by Sprint W6 F4.
- `MANUAL-VERIFICATION.md` — append any cross-browser/manual notes the Lane B walkthrough surfaces (V-#).

## Verification

### Lane A (CI / headless)
- `pnpm build`, `tsc --noEmit`, and eslint (warnings-as-errors) are clean with the new components.
- **F1:** the scrubber emits exactly one `seek {position_ms, internal_id}` on commit, none on intermediate
  drag, and derives no progress percent from its own value.
- **F2:** pitch-off → range −100..100, commit near 1.0 snaps to `1.0`, negative emits `pitch_correction:false`
  with negative `speed`; pitch-on → range 0.05..8.0, reverse removed, negative value re-clamped, every emit
  carries `pitch_correction:true`.
- **F3:** `windowed=true` renders the "streamed" badge and disables seek/speed/reverse/loop-crossfade;
  `windowed=false` leaves them enabled; the `isWindowed` derivation is true for `mode:"stream"` /
  auto-window / live-HTTP-no-Content-Length and false for full-load and for hint-less samples.
- **F4:** voice strip emits `voice_volume {voice, volume}`, `voice_fade_out {voice, time_ms}` (the **`time_ms`**
  key asserted explicitly), and `voice_stop {voice}`.
- **F5:** input strip emits `input_volume {input, volume}` with `input` a **string**, and
  `input_mute {input, mute:<bool>}` with an explicit boolean; no bare-number `input` is ever sent.
- Playwright headless smoke (task 8) captures the correct command JSON for all of the above against the mock
  backend, green.
- Vitest + RTL suite green; no test disabled or skipped; expected error/empty paths asserted, not ignored.

### Lane B (real browser + live daemon)
- Behind the sidecar against a running daemon: dragging the scrubber **audibly** jumps the sample; the speed
  control changes pitch/tempo and reverse plays backward (pitch off); toggling pitch correction re-clamps and
  preserves pitch; the daemon's near-zero coercion is observed (a ~0 speed does not hang). The dashboard's
  `speed`/`voice_volume`/input `muted` fields reflect the changes within a poll interval.
- A voice strip's volume/fade-out/stop audibly affect the voice; fade-out ramps over the requested ms.
- An input strip's volume changes the live input; mute drops it to silence and **unmute restores the prior
  level** (verified via the next `/status/inputs` poll, not a UI-assumed 1.0).
- A windowed/streamed play shows the "streamed" badge with seek/speed/reverse/loop-crossfade disabled, and the
  daemon does not act on any transport command for it (consistent with `seek` being unsupported on
  `mode:stream`).

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 5)

- [ ] Each active sample card has a seek scrubber over `total_ms` (emits `seek`) and a speed control with a 1.0 detent spanning −100..100, plus a pitch-correction toggle that re-clamps the range to 0.05..8.0 and disables reverse `[A]`
- [ ] Windowed/streamed voices show a "streamed" badge and disable seek, speed, reverse, and loop-crossfade controls `[A]`
- [ ] Voice strips expose volume (`voice_volume`), fade-out (`time_ms`), and stop; input strips expose volume (`input_volume`) and mute (`input_mute`) `[A]`
- [ ] Against a real daemon, seek/speed/voice/input controls change playback audibly and the dashboard reflects them `[B]`

## Behavior-change / changelog notes

None — this is a **pure-frontend** sprint that drives existing daemon commands (`seek`, `speed`,
`voice_volume`, `voice_fade_out`, `voice_stop`, `input_volume`, `input_mute`); no daemon code changes and no
RT path is touched, so there is nothing to changelog for the daemon. The one discovered gap — `/status/samples`
carries **no** per-sample is-windowed flag (API-CONTRACT §5), forcing the client-side inference in F3 — is
logged to `docs/bugs.md` as `Sprint W5 F3` and is closed by **Sprint W6 F4**, which adds the real flag (per
DW12's telemetry work). No DW behavior decision is altered by this sprint.

## Definition of Done

Lane A green (`pnpm build` · `tsc --noEmit` · eslint warnings-as-errors · Vitest + RTL · Playwright headless
against the mock backend) · Lane B green (live daemon: seek/speed/voice/input audibly change playback and the
dashboard reflects them; windowed plays gate their transport controls) · all F1–F5 tests added with no
coverage reduction and no skipped/disabled tests · scrubber stays set-only with Sprint 2's "live position
unavailable" messaging intact (no faked playhead) · `voice_fade_out` uses `time_ms`, `input` is a string,
mute restores the prior level · windowed inference documented inline and the `/status/samples` is-windowed gap
logged to `docs/bugs.md` (`Sprint W5 F3`, pointing to Sprint W6 F4) · no daemon/RT change · committed on a
branch with a clear message.
