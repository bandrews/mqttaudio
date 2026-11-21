# Sprint 5 — Lock-free Real-time Engine (the redesign)

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | XL |
| Lanes | A (Docker), B (native macOS) |
| Subagents | YES (ring / voice pool / graveyard / status snapshot / integration) |

> **The design for this redesign is locked in `DECISIONS.md` (D15–D22) — proceed without consulting.** The
> handoff primitive (existing `ringbuf`), `MixerState` ownership, voice-pool cap (default 256) + over-cap
> policy, graveyard reaper, and status mechanism (control-side `RwLock<StatusSnapshot>`, no new dep) are all
> decided. You may overrule a locked choice only if implementation uncovers new evidence (record it per
> `DECISIONS.md`). If something *truly* needs a human and has no other resolution, log it in `NEEDS-HUMAN.md`
> and continue the rest of the sprint — do not halt.
>
> **Do NOT throw away the mixer DSP.** `CLAUDE.md`: "YOU MUST NEVER throw away or rewrite implementations
> without EXPLICIT permission." This sprint **wraps** `mix_audio` and changes *how state reaches the
> callback and who owns it* — it does **not** rewrite the per-frame mixing math. `mix_audio`'s body stays.

## Goal

Make the cpal output callback a true real-time citizen: **never lock, never allocate, never free, never do
unbounded work.** Today the production callback (`src/main.rs:555-582`) locks two contended
`std::sync::Mutex` every buffer, builds and drops two `HashSet<String>` per buffer, and runs the ducking
state machine (which allocates) inline. The redesign moves `MixerState` ownership onto the audio thread and
feeds it through lock-free channels: an SPSC command ring (control → audio), a graveyard return channel
(finished payloads dropped off the RT thread), and a published status snapshot (audio → HTTP). The audible
mix is **unchanged** — proven by the Sprint 0 render harness — and a new allocation-counting harness proves
zero allocs/frees across the callback for a representative scene.

## Why

The audio callback is the one place in this codebase where the usual "just take the lock" reflex is wrong.
A glitch here is audible and unrecoverable within the buffer. The current callback violates every rule of
RT-safe code, and the violating mutexes are *the same ones* the async command loop and HTTP status handlers
contend for — so a slow control-plane operation, or a panic in any holder, directly causes xruns or
permanently dead audio. `mix_audio` even documents the contract it is forced to break by its caller
(`src/audio/mixer.rs:539-545`: "must never: Allocate memory / Block on I/O / Acquire locks / Do expensive
computation"). Sprint 2 added interim *non-poisoning* hardening so a panic can't brick audio via lock
poisoning; this sprint removes the callback locks entirely, which is the real fix.

## Scope

**In scope**
- An SPSC command ring (control thread → audio thread) carrying play/stop/volume/seek/speed/voice-volume
  mutations, with pre-allocated payloads (no per-message heap alloc on the RT side).
- Moving `MixerState` ownership to the audio thread; the callback no longer locks anything.
- A pre-allocated **voice pool** (fixed capacity) so starting a sample is a slot-claim, not a `Vec::push`
  that may reallocate, and stopping a sample returns the slot without freeing on the RT thread.
- A **graveyard** return channel: finished `ActiveSample` payloads (and their `PitchCorrector`, `channel_map`
  `Vec`, etc.) are handed to a non-RT reaper thread that drops them; the callback only moves an owned value
  into a lock-free queue.
- Moving **voice-activity bookkeeping + ducking `notify_voice_active`** off the RT thread. The callback emits
  a lock-free "voice finished" signal; the control thread reconciles activity and drives `update_duck_states`.
- A **status snapshot** maintained by the **control thread** (from commands it sent + completion signals from
  the graveyard ring) in a `RwLock<StatusSnapshot>`, read by HTTP status handlers — they never touch the
  callback's state, and the RT thread never builds status (DECISIONS.md D20; no new dependency).
- A **pre-allocated pitch scratch buffer** owned by `ActiveSample`/`PitchCorrector`, sized to
  `max_frames * channels`, replacing the per-callback `vec![0.0f32; …]` (`src/audio/mixer.rs:797-798`).
- An **allocation-counting test harness** (custom global allocator gated to a test, or an allocation counter)
  proving zero allocs/frees across a callback invocation for a fixed scene.
- An **xrun counter** incremented from the cpal error callback / underrun path, exposed for the soak test.

**Out of scope** (do not touch here)
- Any change to audible mixing math, ducking curve shape, bass-management DSP, or interpolation. Those are
  Sprints 6/7. If the render harness shows a *behavior* difference, you changed too much — revert and narrow.
- `BufferSize::Default` → `BufferSize::Fixed` (knowing `max_frames` for the scratch/pool sizing). **Owned by
  Sprint 1.** This sprint must size scratch/pool from a conservative `max_frames` upper bound (e.g. derived
  from the device's max buffer or a config cap) and assert in the callback rather than assuming Sprint 1 done.
- The non-poisoning lock hardening itself (Sprint 2). This sprint *removes the callback's reliance on those
  locks*; the remaining non-RT locks may keep Sprint 2's hardening.
- Streaming/cache correctness (Sprint 4). The streaming read path is already lock-free (see Caveats).

## Findings addressed

All findings below are RT-safety violations in the production callback unless noted. **Citations verified
against current source.** Severities as given in the brief. Read each cited region before touching it.

### F5-1 — Callback locks two contended `std::sync::Mutex` every buffer · CRITICAL · confirmed
- **Statement:** The output callback acquires `mixer_state` and `active_voices` mutexes on every buffer; the
  same mutexes are locked by the async command loop and the HTTP status handlers, producing priority
  inversion / unbounded blocking → xruns.
- **Evidence:** `src/main.rs:556` `let mut state = mixer_state_clone.lock().unwrap();` and `src/main.rs:580`
  `let mut active = active_voices_clone.lock().unwrap();` run inside the cpal callback. The *same*
  `mixer_state` is locked by the command loop (`src/main.rs:787`, `:795`, `:809`, `:836`, `:862`, `:887`,
  `:967`, `:1002`, `:1043`, `:1076`, `:1110`, `:1140`) and by HTTP status handlers
  (`src/http/handlers.rs:570`, `:601`, `:671`); `active_voices` is locked by the Play handler
  (`src/main.rs:780`). A control-plane thread holding either lock blocks the RT thread for an unbounded time.
- **Fix:** Audio thread **owns** `MixerState`. Control → audio mutations go through an SPSC command ring;
  HTTP/status reads a **control-thread-maintained snapshot** (`RwLock<StatusSnapshot>`, DECISIONS.md D20). The
  callback locks nothing.

### F5-2 — Callback builds two `HashSet<String>` per buffer, cloning every voice_id, and moves one into a mutex · CRITICAL · confirmed
- **Statement:** Each buffer the callback heap-allocates two `HashSet<String>` (cloning each sample's
  `voice_id`) and stores one inside the `active_voices` mutex — heap alloc + free every callback.
- **Evidence:** `src/main.rs:560-562` builds `voices_before` (`.map(|s| s.voice_id.clone()).collect()`),
  `src/main.rs:568-570` builds `voices_after` the same way, and `src/main.rs:581` `*active = voices_after;`
  moves it into the mutex-guarded set (dropping the previous one).
- **Fix:** Move voice-activity bookkeeping **off the RT thread**. The control thread reconciles which voices
  are active from the command stream + the graveyard "voice finished" signals; the callback does no
  set-building and no string cloning.

### F5-3 — Ducking `notify_voice_active` runs inside the callback and allocates · HIGH · confirmed
- **Statement:** The callback calls `engine.notify_voice_active(voice, false)` for voices that went inactive;
  `notify_voice_active` → `update_duck_states` allocates a `HashSet`, builds `Vec`s, and inserts `HashMap`
  entries — all on the RT thread.
- **Evidence:** `src/main.rs:573-577` loops over `voices_before.difference(&voices_after)` calling
  `engine.notify_voice_active(...)` inside the callback. `notify_voice_active`
  (`src/audio/ducking.rs:140-152`) calls `update_duck_states` (`src/audio/ducking.rs:175-251`), which
  allocates `let mut all_potential_voices: HashSet<String> = HashSet::new();` (`:177`), inserts cloned voice
  strings, calls `find_applicable_rules` returning a `Vec<&DuckingRule>` (`:254-264`), and inserts/updates
  `duck_states` `HashMap` entries (`:204-207`, `:245-248`). Note `get_multiplier`
  (`src/audio/ducking.rs:156-163`) is a cheap `HashMap` lookup + `advance_and_get_multiplier` and is fine to
  keep on the RT thread — **only the `notify`/`update_duck_states` path moves off.**
- **Fix:** The callback emits a lock-free "voice finished" signal (via the graveyard / a small event queue);
  `update_duck_states` runs on the **control thread**, which then publishes the updated duck targets into the
  audio-owned `MixerState` (through the command ring / snapshot). `get_multiplier` stays in `mix_audio`.

### F5-4 — Pitch path allocates `vec![0.0f32; frames*channels]` per callback per corrected sample · HIGH · confirmed
- **Statement:** The pitch-correction mix path heap-allocates a fresh stretcher output buffer every callback,
  for every speed-corrected sample.
- **Evidence:** `src/audio/mixer.rs:797-798`: `let output_samples = frames * src_channels;` then
  `let mut stretched = vec![0.0f32; output_samples];`, allocated inside `mix_sample_with_pitch_correction`
  (`src/audio/mixer.rs:763`) which `mix_audio` invokes per sample (`src/audio/mixer.rs:600-602`).
- **Fix:** Pre-allocate a reusable scratch buffer owned by `ActiveSample` (or its `PitchCorrector`), sized to
  `max_frames * channels` once at construction/enable; the callback writes into a sub-slice. No per-callback
  allocation.

### F5-5 — Callback locks use `.lock().unwrap()`; a poisoned mutex permanently kills audio · HIGH · confirmed
- **Statement:** Every callback lock is `.lock().unwrap()`; a panic in *any* holder poisons the mutex and the
  next callback `unwrap()` panics → audio is permanently dead.
- **Evidence:** `src/main.rs:556` and `src/main.rs:580` both `.lock().unwrap()`. Sprint 2 added interim
  non-poisoning hardening for the RT-shared locks (tracker §Sprint 2: "RT-shared state no longer
  poison-bricks audio"). This sprint's redesign **removes the callback locks entirely**, which is the durable
  fix — there is no callback lock left to poison.
- **Fix:** Subsumed by F5-1: the callback owns its state and locks nothing.

### F5-6 — "retain() frees a multi-MB buffer on the RT thread" · REFUTED → low · do NOT chase as critical
- **Statement (as originally raised):** `state.active_samples.retain(|s| !s.is_finished())`
  (`src/main.rs:565`) frees a large decoded buffer on the RT thread when a sample finishes.
- **Why over-stated:** The in-memory cache holds its own `Arc<DecodedBuffer>`; a finished `ActiveSample`
  holds a *cloned* `SampleBuffer` (`Complete(Arc<…>)`, see `src/audio/streaming.rs:11-17`). Dropping it is
  usually an `Arc` refcount **decrement**, not a multi-MB free. So the original "multi-MB free per stop"
  framing is wrong — don't prioritize it as a critical free.
- **Residual (still real, small):** Even a refcount-only drop still frees the per-sample `channel_map`
  `Vec<(usize,usize)>` (`src/audio/mixer.rs:121`) and, if pitch correction was enabled, tears down the
  `signalsmith_stretch::Stretch` inside `PitchCorrector` (`src/audio/pitch_correction.rs:8-12`), whose
  internal buffers are heap-backed. Those frees still must not happen on the RT thread.
- **Fix:** Route finished voices to the **graveyard** queue; a non-RT reaper drains and drops them, so even
  the small `Vec`/`Stretch` frees happen off the callback. The callback's only act is moving an owned value
  into a lock-free queue (an enqueue, not a free).

### F5-7 — (informational) `BufferSize::Default` — owned by Sprint 1
- Sizing the pitch scratch and voice-pool slots needs an upper bound on `frames`. Sprint 1 introduces
  `BufferSize::Fixed`. Until then, size from a conservative `max_frames` cap and **assert** `frames <=
  max_frames` in the callback (a debug-assert + a saturating fallback for release). Do not block on Sprint 1.

## Caveats (refuted / over-stated — don't chase ghosts)

- **F5-6 is NOT a critical RT free.** See above. The dramatic "multi-MB free on the audio thread" claim is
  refuted by the `Arc`-shared cache. Fix the *residual* small frees via the graveyard; don't design around a
  problem that doesn't exist.
- **The streaming read path is already lock-free in the callback.** `mix_audio` reads streaming samples via
  `SampleBuffer::get_sample_or_silence`, which uses `try_read()` and returns silence on contention
  (`src/audio/streaming.rs:197-210`) — non-blocking by design. Do **not** "fix" it by adding a lock or moving
  it; the callback-level locks to remove are the `MixerState`/`active_voices` mutexes (F5-1), not the
  streaming reads.
- **`get_multiplier` stays on the RT thread.** Only `notify_voice_active`/`update_duck_states` (the
  allocating state-machine recompute) move off. `get_multiplier` (`src/audio/ducking.rs:156-163`) is a
  `HashMap` get + arithmetic — leave it in `mix_audio`.
- **`LiveInput` already uses an SPSC ring (`ringbuf::HeapConsumer`).** `mix_live_input_into_output` pops from
  it (`src/audio/mixer.rs:856-870`) without locking. Don't replace that mechanism; the new command ring is a
  *separate* control→audio channel, not a substitute for the live-input ring.

## Tasks (ordered, TDD-first)

Phase the work: **5a** lands the ring + status snapshot **while keeping current audible behavior** (status
reads the snapshot; the `active_voices` callback lock is removed). **5b** moves `MixerState` ownership to the
audio thread, adds the voice pool + graveyard, and deletes the remaining callback lock and the per-buffer
`HashSet`s. Use the Sprint 0 render harness at the end of **each** phase to prove parity.

Subagents (per metadata): **ring**, **voice pool**, **graveyard**, **status snapshot**, **integration**.
Each owns its module + tests; the integration agent wires them into `main.rs` and runs the parity/soak gates.

**0. Allocation-counting harness first (the gate for everything).**
   - Write `tests/support/alloc_counter.rs` (or `src/audio/test_support.rs` under `#[cfg(test)]`): a custom
     `#[global_allocator]` that increments atomic `alloc`/`dealloc` counters, with `arm()/disarm()` so
     counting is scoped to a region. (A custom global allocator is process-wide; gate it to a dedicated test
     binary, or use a thread-local "armed" flag so only the callback region counts.)
   - **Failing test first:** `callback_shim_makes_zero_allocations` — build a representative `MixerState`
     (2 samples + 1 looping + 1 pitch-corrected + ducking enabled, via the Sprint 0 builder), arm the
     counter, run the callback shim for N blocks, assert `allocs == 0 && deallocs == 0`. Confirm it **fails**
     against today's callback (it will: F5-2/F5-3/F5-4 all allocate). This test is the regression anchor.

**1. Pitch scratch pre-allocation (smallest standalone win — F5-4).**
   - **Failing test first** (alloc harness): `pitch_path_allocates_zero` — one pitch-corrected sample,
     arm counter, render M blocks, assert zero allocs. Fails today (`mixer.rs:798`).
   - Add `pitch_scratch: Vec<f32>` to `ActiveSample` (or `PitchCorrector`), sized `max_frames * channels` at
     enable. In `mix_sample_with_pitch_correction`, write into `&mut self.pitch_scratch[..output_samples]`
     instead of `vec![…]`. Assert `output_samples <= scratch.len()`.
   - **Parity:** Sprint 0 render harness — render a fixed pitch-corrected scene before/after, assert
     `rms`, `peak`, and `band_energy` within tolerance and `max_inter_sample_delta` unchanged (no new click).

**2. Status snapshot (5a — F5-1 read side).**
   - Define a plain `StatusSnapshot` (active sample count, per-voice ids/volumes/positions, ducking state —
     whatever `src/http/handlers.rs:570-680` currently reads from the lock).
   - **Failing test first:** `status_snapshot_reflects_published_state` — publish a snapshot, read it from a
     simulated HTTP handler, assert it matches; assert reading does **not** touch `MixerState`.
   - Use `arc-swap` (single new dep) **or** a hand-rolled triple-buffer. Prefer `arc-swap` for simplicity
     (YAGNI vs. hand-rolling); **flag the new dependency per DECISIONS.md.** Publisher = the audio side
     (cheap pointer swap, allocation-free on the swap itself; building the snapshot happens off-RT or is
     pre-sized). Migrate `handlers.rs` status reads to the snapshot.

**3. SPSC command ring (5a — F5-1 write side).**
   - Add an `AudioCommand`-style enum for *mutations the callback's owner applies* (add sample, set
     fade/stop, voice volume target, seek, speed, ducking-target update). Pre-allocate the ring; bound
     capacity; on full, the control thread handles back-pressure (log + retry/await — never block the RT
     thread).
   - **Failing test first:** `ring_delivers_play_then_stop_in_order` — push Play then Stop, drain on the
     "audio" side, assert the resulting `MixerState` matches the current `handle_command` path's result for
     the same inputs (reuse Sprint 0's `handle_command` tests as the oracle).
   - In 5a, keep `MixerState` where it is but route the **Play handoff** and **`active_voices` update**
     through the ring + control-thread reconciliation so the **`active_voices` callback lock
     (`src/main.rs:580`) can be deleted.** Render-harness parity after.

**4. Audio thread owns `MixerState` + voice pool (5b — F5-1, F5-2).**
   - Move `MixerState` ownership into the audio callback closure (or an audio-thread-local the closure
     borrows). The control thread no longer locks it; it sends ring commands. Delete the `mixer_state`
     callback lock (`src/main.rs:556`) and the per-buffer `HashSet` building (`src/main.rs:560-562`,
     `:568-570`, `:581`).
   - **Voice pool:** pre-allocate a fixed-capacity `Vec<ActiveSample>` (or slot array) at startup; "play"
     claims a free slot (no `Vec::push` realloc), "stop/finish" returns it. Size from config (max voices).
   - **Failing test first:** `voice_pool_play_is_allocation_free` (alloc harness) and
     `voice_pool_rejects_when_full_without_panic` (back-pressure, control side).
   - **Parity:** render harness on a fixed multi-voice scene; assert within tolerance vs. pre-redesign.

**5. Graveyard + reaper (5b — F5-6 residual).**
   - Finished samples are moved (owned) into a lock-free `graveyard` queue by the callback; a non-RT reaper
     thread drains and drops them (freeing `channel_map`, `Stretch`, etc. off-RT).
   - **Failing test first:** `finished_sample_is_enqueued_not_dropped_on_rt` (alloc harness: stopping a
     pitch-corrected sample causes zero deallocs in the callback region) and `reaper_drains_graveyard`
     (the reaper eventually drops everything; no leak — assert via the alloc counter that deallocs happen
     off-RT).

**6. Ducking off the RT thread (5b — F5-3).**
   - Callback emits "voice finished" (voice_id) via a lock-free signal; control thread calls
     `notify_voice_active(voice, false)` / `update_duck_states` and publishes updated duck targets back
     through the ring. `get_multiplier` stays in `mix_audio`.
   - **Failing test first:** `ducking_notify_runs_off_rt` (alloc harness: a voice going inactive causes zero
     allocs in the callback region) **and** a behavior test: ducked-voice RMS follows the configured fade the
     same as before, asserted with the render harness (`rms` over time monotone toward target; no
     `max_inter_sample_delta` spike). This proves we moved *where* ducking is computed without changing the
     *audible* curve.

**7. Xrun counter + error path.**
   - Increment an atomic `xruns` from the cpal error callback (`src/main.rs:583-585`) and from any
     callback-detected underrun/over-budget condition. Expose it on the status snapshot for the soak test.
   - **Failing test first:** `error_callback_increments_xrun_metric` — invoke the error path, assert counter
     bumped.

**8. Full allocation gate green + soak.**
   - Re-run task 0's `callback_shim_makes_zero_allocations` on the integrated engine — must now **pass**
     (zero allocs/frees across the callback for the representative scene).
   - Soak test (Lane A): drive many plays/stops (e.g. 10k commands) through the engine over many simulated
     buffers; assert the `xruns` counter stays **0** and memory is bounded (voice pool capacity respected,
     graveyard drains).

## Files to create / touch

- **Create:** `src/audio/rt_engine.rs` (or a small module set: command ring type, voice pool, graveyard,
  status snapshot publisher) — wraps, does not replace, `mix_audio`. `tests/support/alloc_counter.rs`
  (global-allocator counting harness). New tests under `tests/` / `#[cfg(test)]` modules per task above.
- **Touch:**
  - `src/main.rs` — replace the callback body (`:555-582`) so it owns `MixerState`, drains the command ring,
    runs `mix_audio`, routes finished samples to the graveyard, emits voice-finished signals, publishes the
    status snapshot; delete both callback `.lock().unwrap()` calls (`:556`, `:580`) and the per-buffer
    `HashSet` code (`:560-562`, `:568-570`, `:581`); increment `xruns` in the error callback (`:583-585`).
    Convert the command-handler lock sites (`:780-797`, `:809-1143`) to send ring commands instead of locking
    `mixer_state`/`active_voices`.
  - `src/http/handlers.rs` — replace `mixer_state.lock()` status reads (`:570`, `:601`, `:671`) with snapshot
    reads.
  - `src/audio/mixer.rs` — add the pre-allocated pitch scratch to `ActiveSample`/`PitchCorrector`; replace
    the per-callback `vec!` at `:797-798`. (`mix_audio`'s per-frame math at `:546-588` is **unchanged**.)
  - `src/audio/ducking.rs` — no DSP change; ensure `notify_voice_active`/`update_duck_states`
    (`:140-251`) are only invoked from the control thread now.
  - `src/audio/pitch_correction.rs` — add scratch ownership if the scratch lives here.
  - `Cargo.toml` — `arc-swap` (or none if triple-buffer is hand-rolled). **No new dependency needed (DECISIONS.md D15/D20).**

## Verification

- **Lane A (Docker / Linux):** `scripts/validate.sh` green — build `-D warnings`, clippy `-D warnings`,
  `fmt --check`, full test suite including: the **allocation harness** (`callback_shim_makes_zero_allocations`
  passes; zero allocs/frees), **render parity** (within-tolerance `rms`/`peak`/`band_energy`/
  `max_inter_sample_delta` vs. a captured pre-redesign baseline for a fixed scene), and the
  **many-plays/stops soak** (10k commands; `xruns == 0`; memory bounded). The error path increments the xrun
  metric.
- **Lane B (native macOS, this Mac):** `scripts/validate.sh --native` green, plus a **real-device soak smoke**
  — open the default CoreAudio device, run a play/stop sequence for a sustained period, confirm no audible
  dropouts and `xruns == 0`.
- **Lane C (Windows):** none required this sprint (no user-facing behavior change). If the audio-thread
  ownership model needs a Windows note, append it to `MANUAL-VERIFICATION.md`.

## Acceptance criteria (mirrored verbatim from SPRINT-TRACKER.md)

- [ ] No locks, allocations, or frees in the callback path — verified by code audit **and** an allocation-counting harness around `mix_audio`/the callback shim `[A]`
- [ ] Control→audio handoff via SPSC command ring; audio thread owns `MixerState`; pre-allocated voice pool; graveyard reaper drops finished payloads off-RT; status via snapshot for HTTP `[A]`
- [ ] Ducking notify + voice bookkeeping moved off the RT thread; pitch scratch pre-allocated `[A]`
- [ ] Render harness shows within-tolerance output vs pre-redesign for a fixed scene; soak test (many plays/stops) shows no xrun-counter increments `[A]`
- [ ] Real-device soak smoke runs clean on this Mac `[B]`

## Behavior-change / changelog notes

- **No audible behavior change is intended.** The render-harness parity assertion is the proof; if it can't
  stay within tolerance, you've changed mixing behavior and must STOP and narrow the change.
- **Behavior-changing (all locked in DECISIONS.md, D15–D22 — proceed without consulting):**
  - The architectural redesign itself (RT-engine ownership model) — design locked in DECISIONS.md.
  - **No new dependency:** command/return rings use the existing `ringbuf`; the status snapshot is a
    control-side `RwLock<StatusSnapshot>` (DECISIONS.md D15/D20).
  - Voice-pool **fixed capacity** introduces a hard cap on simultaneous voices where today `Vec::push` grows
    unbounded. This is a user-visible behavior change (over-cap plays are rejected/back-pressured instead of
    always accepted). **Cap + over-cap policy per DECISIONS.md (D17/D18); document in `README.md` /
    `CHANGELOG.md`.**
- **New observable:** an `xruns` counter on the status endpoint (additive; document in `README.md`).
- Internal-only (no changelog): callback no longer locks/allocs; ducking recompute relocated to the control
  thread. Note the relocation in your commit message.

## Definition of Done

Lane A green (allocation harness proves zero allocs/frees; render parity within tolerance; soak shows
`xruns == 0`) · Lane B green incl. real-device soak smoke · `mix_audio` DSP unchanged (wrapped, not rewritten) ·
no callback locks remain (`src/main.rs:556`, `:580` deleted) · pitch scratch pre-allocated
(`src/audio/mixer.rs:797-798` replaced) · ducking `notify`/`update_duck_states` invoked only off-RT · no tests
disabled or `#[ignore]`d to pass · `cargo build --release` warning-free · clippy clean · out-of-scope
discoveries logged to `docs/bugs.md` · committed atomically to the branch as units complete.
