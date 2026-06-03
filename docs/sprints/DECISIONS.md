# Locked Decisions

Every open decision in this program is **resolved here, upfront**, so no sprint stalls waiting on a human.
Treat each as the **default ruling**: build the locked choice and **do not stop to ask**. Wherever a sprint
file says "partner sign-off", "decide with the partner", "stop and ask", or similar, **this document
supersedes it** — the decision is already made.

**You have authority to overrule a locked decision — but only when implementation uncovers new information**
(a measurement, a code reality, a dependency or correctness problem, a test result) that makes a different
choice clearly better. These are strong, reasoned recommendations, not handcuffs. If new evidence shows a
locked decision is wrong, **change it** — that is expected senior engineering, not a violation. When you do:
(1) override because the *evidence* demands it, not preference; (2) record what you discovered and what you
chose instead in `docs/bugs.md` (and the changelog if user-visible) and update the relevant sprint file;
(3) keep it consistent with the program's intent — RT-safety, backward-compatible/open-by-default behavior,
YAGNI, no shortcuts. Absent new evidence, follow the locked choice and keep moving.

Rationale is given so you understand intent and can judge whether new evidence truly outweighs it.

## Sprint 1 — Device & format

- **D1 · Channel-count fallback.** If the exact requested channel count isn't available, open the
  **next-larger** available count and zero-fill the extra outputs; error only if none ≥ requested.
  *Why:* a 6-ch request on a 2/8-ch device should play, not abort.
- **D2 · Sample-rate selection.** Pick the **nearest device-supported** rate (prefer the requested; among
  supported, minimize |Δ|), validated against `supported_output_configs` before building — never an arithmetic
  clamp to an unsupported value. *Why:* discrete-rate DACs (44.1-only, 48/96) must not get an invalid rate.
- **D3 · Buffer size.** Honor `audio.buffer_size` via `BufferSize::Fixed(n)` when within the device's supported
  range; fall back to `Default` if unsupported (log it). *Why:* it's the only latency/xrun knob.
- **D4 · Device-error recovery.** On a fatal stream error, rebuild the stream with exponential backoff
  (250 ms → 5 s cap, ~6 attempts); if still failing, **exit non-zero** so a supervisor (systemd) restarts.
  *Why:* unattended installs must self-heal or hard-fail visibly, not go silently dead.

## Sprint 2 — Control-plane reliability

- **D5 · Interim lock primitive.** Use `parking_lot::Mutex` for control-plane shared state (non-poisoning,
  faster). The audio-callback lock is *removed entirely* in Sprint 5, so this is the bridge, not the endgame.
  *Why:* a non-RT panic must never poison a lock the callback touches.
- **D6 · Command backpressure.** Keep the bounded `mpsc(100)`; make the consumer **non-blocking** (decode via
  `spawn_blocking`, no lock held across `.await`). Do **not** drop commands. *Why:* once the consumer can't
  stall, the channel won't fill, so the MQTT poll loop never blocks — without losing commands.

## Sprint 3 — Security (defaults stay open; everything is opt-in)

- **D7 · Allowlist.** Enforce `allowed_directories` **only when non-empty**; empty (default) = allow-all +
  one-time startup warning. *Why:* preserve today's trusted-network behavior; protect those who opt in.
- **D8 · MQTT transport.** Plain TCP default **always** (including 8883); TLS strictly opt-in via `mqtt.tls`;
  non-fatal warning on cleartext creds to a non-loopback broker. *Why:* never silently break a legacy broker.
- **D9 · HTTP auth.** Open by default; opt-in `http.require_auth` gates **all** routes (incl. status/ws);
  non-fatal exposure warning on a non-loopback bind without a token; keep the `?token=` convenience;
  constant-time token compare; CORS stays configurable (default unchanged). *Why:* open mode is a feature.
- **D10 · Cache revalidation.** **Implement** conditional revalidation (If-None-Match/If-Modified-Since) gated
  by the existing `revalidate_after_seconds` (default 300). *Why:* the config + stored validators already
  promise it; cost is one cheap 304 after TTL. (Do not delete the knob.)

## Sprint 4 — Streaming & cache

- **D11 · Loop-on-stream.** Defer loop wrap-around until the streaming buffer `is_complete()` (wrap on the true
  total). A looping Play of a still-downloading file plays forward (silence past the loaded edge) until
  complete, then loops. *Why:* removes the buzz without adding a prebuffer wait.
- **D12 · Eviction vs playing buffers.** Rely on `Arc` keep-alive; in `evict_one`, **skip entries whose buffer
  `Arc::strong_count() > 1`** (still playing). Fix `current_size_bytes` to track resident bytes accurately.
  **Delete** the dead `mark_playing`/`mark_not_playing`/`playing`-set design. *Why:* correct and simpler — no
  Play/finish bookkeeping to thread through.
- **D13 · Stream completion.** Promote a finished streaming decode to a **Complete** memory-cache entry and
  drop the `active_loads` entry, so replays hit the cache. *Why:* fixes the leak + redundant re-streaming.
- **D14 · `MIN_BUFFER_FRAMES`.** **Remove** the dead constant + its misleading doc comment; rely on
  silence-until-loaded (already how the mixer behaves). *Why:* YAGNI; brief leading silence is self-correcting.
  If it later proves audible, add a prebuffer then — note in `docs/bugs.md`, don't build it now.

## Sprint 5 — Lock-free real-time engine (NO upfront consult — build this design)

- **D15 · Handoff primitive.** Use the **existing `ringbuf`** dependency for a lock-free SPSC **command ring**
  (control → audio) and a separate SPSC **return/graveyard ring** (audio → reaper). **No new dependency.**
- **D16 · State ownership.** The **audio thread owns `MixerState`**. Each callback first drains up to a bounded
  N commands from the ring (apply to its state), then mixes. No mutex in the callback.
- **D17 · Voice pool.** A fixed-capacity `Vec` reserved to `audio.max_voices` (**default 256**, configurable);
  voices live in slots, removed via `swap_remove`. Never grows at runtime. *Why:* "20+ simultaneous" with
  generous headroom; slot structs are cheap (buffers are `Arc`-shared from cache).
- **D18 · Pool-exhaustion policy.** When full, **steal the oldest non-looping voice**; if all are looping,
  **reject** the new Play with a warning. *Why:* predictable, never blocks or allocates.
- **D19 · Graveyard.** Finished voices (and any owned `Vec`/`Arc`/`PitchCorrector`) are pushed to the return
  ring; a dedicated **low-priority reaper thread** drains and drops them. *Why:* zero frees on the RT thread.
- **D20 · Status snapshot (and voice-activity bookkeeping).** The **control thread** owns the authoritative
  "currently playing / active voices" view, built from commands it sent + completion events received over the
  return ring, and exposes it to HTTP via a control-side `RwLock<StatusSnapshot>`. The RT thread never builds
  status and never touches `HashSet`s. **No `arc-swap`/new dep needed.** *Why:* moves all the per-buffer
  `HashSet` churn and ducking-notify off the callback for free.
- **D21 · DSP unchanged.** `mix_audio` and the per-frame DSP are **wrapped, not rewritten** (per CLAUDE.md).
  Pitch scratch buffers are pre-allocated per voice. Only ownership/handoff changes.
- **D22 · Phasing.** Land 5a (rings + control-side status, callback still functions, remove the `active_voices`
  callback lock) before 5b (move `MixerState` ownership + pool + graveyard, delete the remaining callback lock
  and per-buffer `HashSet`s). Each phase must keep the render-harness output within tolerance.
- **D22a · OVERRIDE of D16's "callback owns `MixerState`, no lock" (decided with the partner, 2026-06-03).**
  *New evidence:* mixing must run inside cpal's callback (cpal is the clock), but the supervisor **rebuilds that
  callback closure on device errors** (Sprint 1 recovery). A recreated closure cannot carry persistent *owned*
  state — the `MixerState` and the command-ring **consumer** would be lost on every rebuild, orphaning the
  control-side producer. The strictly-lock-free alternatives are a dedicated mixing thread feeding cpal via an
  output ring (you then build your own audio clock — real underrun/latency risk) or an `unsafe` raw pointer in
  the hot path. *Decision:* keep `Arc<Mutex<AudioCallbackState>>` (bundling `MixerState` + the command-ring
  consumer + the graveyard producer), but the **control thread never locks it** — all mutations go through the
  command ring, all status through the control-side snapshot. Only the callback locks it (uncontended,
  allocation-free) and the supervisor during a rebuild. This eliminates the real targets of D16 (priority
  inversion, lock poisoning, per-buffer `HashSet` allocation, RT-thread frees) and passes the allocation
  harness; it keeps one uncontended lock, so acceptance boxes 1–2 are read as **"no *contended* locks; the
  control plane never touches RT state; the callback does no allocation or free."** Partner-approved.

## Sprint 6 — Mixer DSP

- **D23 · Play volume.** Clamp to `[0,1]` at construction (matches the runtime `Volume` command). *Behavior
  change* — changelog it.
- **D24 · Auto voice id.** Use a monotonic atomic counter: `_auto_<n>` (not millis). *Behavior change* — anyone
  relying on same-millisecond plays sharing a voice loses that accident; that was a bug.
- **D25 · Ducking restore.** Restore uses the triggering rule's `fade_duration_ms` (tracked per voice), not a
  fixed 2000 ms. *Behavior change* — changelog it.
- **D26 · Limiter.** Replace the hard clamp with a **soft-knee / true-peak limiter**, configurable ceiling
  **default −1.0 dBFS**, plus a master gain. A `tanh` soft-clip is an acceptable first cut, but the target is a
  short-look-ahead limiter. Expose a clip/over `AtomicU64` counter in `/status`.
- **D27 · Speed interpolation.** Switch the non-pitch path to **cubic/Hermite** interpolation now (cheap, fixes
  most slow-speed artifacts). Keep the `±100` speed range; **document** that speeds > 1 alias on the fast path.
  Do not route speed through the resampler (YAGNI for now).
- **D28 · Crossfade curve.** **Equal-power** (cos/sin) for the loop crossfade, with correct overlap-on-wrap
  (advance past the overlapped head). Leave fade-in/out **linear** (fine for short fades).
- **D29 · Downmix gain (F7).** Add an **optional** per-route gain to the channel map (default 1.0). Don't change
  existing 1:1 routing behavior. *Why:* lets users tame downmix clipping without breaking current configs.

## Sprint 7 — Bass management

- **D30 · `remove_bass_from_sources` default.** Default **`true`** when bass management is enabled (proper bass
  management high-passes the mains). The additive "LFE+Main" mode remains available by setting it `false`.
  *Behavior change* for existing bass-mgmt users — changelog + README it.
- **D31 · Crossover.** **4th-order Linkwitz-Riley** (cascade two identical Butterworth biquads for LP and HP).
- **D32 · LFE gain compensation.** Normalize the summed LFE by the **active source count** so sub level is
  count-independent; expose an `lfe_gain` trim (default 1.0). Add a one-time warning when
  `lfe_channel >= output_channels` (bass silently dropped). No final-LFE low-pass for now (YAGNI; note it).

## Sprint 8 — Live input

- **D33 · Drift handling.** Always run async SRC steered by ring-buffer fill (target ~half-full via
  `set_resample_ratio`), **even when nominal input==output rate**. *Why:* independent device clocks always drift.
- **D34 · Input mute.** Store the pre-mute volume and **restore it** on unmute (not hardcoded 1.0). *Behavior
  change.*
- **D35 · Input `voice_volume`.** Make `voice_volume` affect matching live inputs even when no sample-backed
  voice exists (treat a matching input as success). *Behavior change* (was silently inert).
- **D36 · Input ducking trigger.** Mark configured input voices active so they can be ducking primaries (gate on
  signal presence is fine; "always active while the stream is open" is acceptable v1). *Behavior change* (was
  inert). Pairs with the Sprint-6 ducking notify path.
- **D37 · Non-f32 input.** Build a typed input stream matching the device format and convert to f32 (mirror
  Sprint 1's output dispatch).

## Sprint 9 — Cleanup / observability

- **D38 · Hot-reload.** **Skip it** (YAGNI). Config is read at startup only.
- **D39 · Dev scaffolding.** Remove `init_test_sine_wave`/`play_file`/`test_mixer`, the `--test-tone/--file/
  --test-mixer` flags, the hardcoded `/Users/bandrews` paths, dead constants, and the stale
  `#![allow(dead_code)]` "Phase 10" banner. The build must be warning-free **without** blanket `allow(dead_code)`.
- **D40 · Observability.** Add `/version` and `/metrics` (uptime, active voices, clip/over count, xrun/dropout
  count, per-voice ducking state) and structured (JSON) logging as an opt-in; ship a systemd unit + README.
- **D41 · Small feature-conflict fixes.** `seek` clamps against `total_frames_or_estimate` (consistent with
  `start_position`); warn when `crossfade_ms` is set without `loop:true`; document the `channel_map`→LFE
  bypass. All low-risk; just do them.
