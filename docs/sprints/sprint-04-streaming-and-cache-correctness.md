# Sprint 4 — Streaming & Cache Correctness

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A |
| Subagents | optional |

## Goal

Make progressive (HTTP-streamed) playback and the in-memory/disk cache behave correctly over a file's
*whole* lifecycle: a looping stream must not buzz on a still-growing buffer, a finished stream must be
promoted to the memory cache (and not re-streamed on replay), `active_loads` must not grow unboundedly,
the LRU eviction-protection and size accounting must actually work, and the dead `MIN_BUFFER_FRAMES`
prebuffer must either be enforced or removed along with its misleading documentation. This sprint fixes
the streaming/cache layer; it does **not** touch disk-atomic-write or HTTP revalidation (those belong to
Sprint 3 — reference them, do not duplicate them here).

## Why

The audit (STREAMING-CACHE + the FEATURE-CONFLICTS `loop:true` item) found that the streaming buffer's
"frames available so far" count is used as if it were the file's total length in three places, that the
cache's promotion / cleanup / eviction-protection machinery is all `#[allow(dead_code)]` and never wired
into the running daemon, and that a documented prebuffer threshold is never enforced. The net effect:

- A `loop:true` Play of an HTTP URL that starts before the download finishes wraps against an ever-growing
  tiny prefix, replaying a tens-of-millisecond window in a tight buzz until the stream completes.
- Every streamed URL's full decoded PCM is retained in `active_loads` for the life of the process (an
  unbounded leak), and replaying a finished URL rejoins the stale streaming buffer instead of a cached
  `Complete` buffer — so it never enters the LRU/size accounting a normal cache entry would.
- The "never evict a playing buffer" guarantee is inert and `current_size_bytes` can under-report, so a
  bounded cache (`max_memory_mb`) can silently exceed its configured limit.
- A constant and doc comment promise a startup prebuffer that the code does not implement, producing
  leading silence and a false impression of behavior.

## Scope

**In scope**
- Fix looping against the streaming buffer's growing loaded length (defer the wrap until the buffer is
  complete, or wrap against the true total and emit silence for not-yet-loaded frames).
- Promote a finished streaming load into the memory cache and drop its `active_loads` entry, so replays
  hit the cache and retention is bounded.
- Wire the LRU eviction-protection (`mark_playing` / `mark_not_playing`) into the Play / sample-finish
  paths **or** deliberately drop that design in favor of Arc keep-alive — either way fix size accounting.
- Decide `MIN_BUFFER_FRAMES`: enforce it (gate stream start with a timeout) **or** delete the dead
  constant and the misleading `get_or_load_streaming` doc comment.

**Out of scope**
- Disk-cache atomic write (temp-file + rename) and load-time size verification — **Sprint 3**.
- HTTP conditional revalidation (`If-None-Match` / `If-Modified-Since` / `revalidate_after_seconds`) —
  **Sprint 3**.
- The chunked-vs-one-shot resampler PCM divergence is acknowledged below as a Caveat / low-priority item;
  do not undertake a resampler rewrite here. If a clean fix falls out of the promotion work, take it;
  otherwise log it in `docs/bugs.md` and leave it for Sprint 9 cleanup.
- The std-`Mutex`-held-across-`.await` issue on `cache_manager` (Play/Precache) — that is **Sprint 2**
  (control-plane reliability); do not refactor the locking here.
- Any RT-thread/lock-free redesign — **Sprint 5**. Keep changes within the existing locking model.

## Findings addressed

### F1 — HIGH (confirmed): looping a still-streaming buffer wraps against the growing loaded count, not the total
- **Statement:** A `loop:true` Play of an HTTP/streaming buffer wraps `position % frames()` where, for a
  `Streaming` buffer, `frames()` is the *frames decoded so far*, so the loop replays an ever-growing tiny
  prefix until the download completes.
- **Evidence:**
  - `SampleBuffer::frames()` returns `frames_available()` (the growing `AtomicUsize`) for the `Streaming`
    variant — `src/audio/streaming.rs:185-192` (`Streaming(buf) => buf.try_read().map(|b| b.frames_available())`).
    `frames_available` only grows as `append()` runs (`streaming.rs:74`, set at `92-94`).
  - `advance_position()` recomputes `let buffer_frames = self.buffer.frames();` every callback and wraps
    `new_pos % buffer_frames` when `loop_mode && buffer_frames > 0` — `src/audio/mixer.rs:397`, wrap sites
    `403-410`.
  - The per-frame wrap in `mix_sample_into_output` captures `buffer_frames` once per callback
    (`src/audio/mixer.rs:607`) and wraps `src_pos % buffer_frames` *inside* the per-frame loop
    (`src/audio/mixer.rs:620-627`); if `buffer_frames` < the callback frame count, the same tiny window is
    replayed several times within one callback — a tight buzz.
  - `is_finished()` returns `false` whenever `loop_mode` is set (`src/audio/mixer.rs:340-341`), so the
    sample never ends and keeps wrapping at the growing length.
  - The Play handler computes `is_streaming` (`src/main.rs:666`) but passes `loop_mode` straight through to
    `ActiveSample::new_with_mapping` / `new_with_id` (`src/main.rs:736`, `749`) with no streaming guard, and
    pushes the sample immediately (`src/main.rs:791`, `796`).
  - `start_streaming_load` returns the `Streaming` buffer immediately (`src/cache/mod.rs:190`) with decode
    running in a spawned `spawn_blocking` task (`src/cache/mod.rs:186-188`); `frames_available` is ~0 at
    play time.
- **Severity:** HIGH. Real audible glitch under realistic conditions (slow / stalled streaming with
  `loop:true`); no safeguard exists.
- **Fix:** Defer the loop wrap until the streaming buffer `is_complete()` — i.e. while a looping sample's
  buffer is still `Streaming` and not complete, do **not** wrap; either wrap against
  `total_frames_or_estimate()` and emit silence for not-yet-loaded frames, or simply do not engage looping
  until `LoadingState::Complete`. Once complete, `frames()` (Complete path) returns the true total and the
  existing wrap is correct. The chosen approach must be applied consistently in both `advance_position`
  (`mixer.rs:399-414`) and the per-frame wrap (`mixer.rs:620-627`).
- **Caveat (do not chase a ghost):** This bites **only when playback outruns decode** — when the playback
  position reaches the currently-loaded frame count before the next decode chunk arrives. If the background
  decoder outpaces realtime, `buffer_frames` grows faster than `position` and looping only engages at the
  true end. So "buzz at the start of *every* looped streaming play" is **over-stated**; describe and test
  the worst case (a first decode chunk smaller than the callback, or a network stall mid-loop) accurately.
  Cached (`Complete`) and local files are unaffected because `frames()` returns the true total.

### F2 — MEDIUM (confirmed): `active_loads` never cleaned up; finished streams never promoted; replay rejoins stale streaming buffer
- **Statement:** `start_streaming_load` inserts into `active_loads` but nothing ever removes the entry or
  promotes the finished decode into the memory cache, so streamed PCM is retained for the process lifetime
  and a second Play of the same URL re-shares the old streaming buffer instead of a `Complete` cache entry.
- **Evidence:**
  - Insert into `active_loads` at `src/cache/mod.rs:173`; the entry holds `Arc<RwLock<StreamingBuffer>>`
    (full decoded PCM) — `src/cache/mod.rs:22-28`, `94-103`.
  - `get_or_load_streaming` checks `active_loads` *before* any disk/network work and returns
    `SampleBuffer::Streaming(Arc::clone(&active.buffer))` for a join — `src/cache/mod.rs:100-102`.
  - `cleanup_completed_loads` (which removes finished loads and promotes them to `memory_cache` via
    `DecodedBuffer::new(guard.data().to_vec(), ...)` + `memory_cache.put`) is `#[allow(dead_code)]` and has
    no production caller — `src/cache/mod.rs:289-322` (attribute `291`; promotion `308-318`).
  - The decode task itself only calls `guard.mark_complete()` at `src/cache/mod.rs:252-254`; it does not
    touch `active_loads` or the memory cache.
- **Severity:** MEDIUM. Unbounded retention + redundant re-stream + the advertised promotion never happens.
- **Fix:** On decode-task completion (where `mark_complete()` is called, `src/cache/mod.rs:251-254`), remove
  the `active_loads` entry for that URL and insert the finished `DecodedBuffer` into the memory cache. The
  decode task only holds an `Arc<RwLock<StreamingBuffer>>`, not `&mut CacheManager`, so the cleanest wiring
  is to give the decode task a handle that lets it perform the promotion (e.g. pass the `MemoryCache`/an
  `active_loads` removal capability into the spawned task, or — simpler and lower-risk — have the command
  loop call `cleanup_completed_loads()` opportunistically, e.g. on the next Play/Precache, after acquiring
  the cache lock). Prefer the decode-task-side promotion so a replay that arrives after completion gets a
  `Complete` buffer; pick whichever is the smallest reasonable change that makes the acceptance tests pass.
  Reuse the existing `cleanup_completed_loads` body — do not write a second promotion path.

### F3 — MEDIUM (confirmed): LRU eviction-protection inactive; a playing buffer can be evicted; size accounting under-reports
- **Statement:** `evict_one` skips only keys in the `playing` set, but `mark_playing` / `mark_not_playing`
  are `#[allow(dead_code)]` and called only from tests, so `playing` is always empty at runtime and a
  currently-playing buffer can be evicted from the index; on eviction `current_size_bytes` is decremented
  even though an `ActiveSample` still holds an `Arc` to the data, so the resident bytes are no longer
  counted.
- **Evidence:**
  - `evict_one` filters out `self.playing.contains(*key)` and decrements `current_size_bytes` on removal —
    `src/cache/memory.rs:118-134` (filter `122`; decrement `128`).
  - `mark_playing` / `mark_not_playing` / `is_playing` are all `#[allow(dead_code)]` —
    `src/cache/memory.rs:179-197`. A grep shows production never calls them (only the tests at
    `memory.rs:445-509` do).
  - `memory_usage_bytes()` returns `current_size_bytes` — `src/cache/memory.rs:175-177` — so an
    evicted-but-still-resident (Arc-held) buffer's bytes vanish from the reported total while still occupying
    RAM, letting a bounded cache exceed `max_memory_mb` while reporting it is under.
- **Severity:** MEDIUM. No use-after-free (the `Arc` keeps data alive — eviction is *correctness*-safe),
  but the protection guarantee and the size accounting are both broken.
- **Fix:** Two acceptable approaches — pick one and document the choice:
  1. **Wire the protection:** call `mark_playing(key)` when a Play lands a `Complete`-backed sample and
     `mark_not_playing(key)` on sample-finish cleanup; then fix accounting so a protected, resident buffer
     is counted. (Sample-finish cleanup currently happens via the callback `retain()` — driving
     `mark_not_playing` from there crosses into the RT path, which Sprint 5 owns; prefer driving it from the
     command thread when it observes a finished sample, or revisit in Sprint 5 if it cannot be done cleanly
     off-RT.)
  2. **Drop the design:** remove the `playing` set and rely solely on `Arc` keep-alive for
     correctness, and make the size accounting honest (don't claim protection you don't provide).
  Either way, the bounded-cache test below must hold: with one playing buffer, the cache stays at or below
  `max_memory_mb` and `memory_usage_bytes()` reflects reality. Note: `MemoryCache` only ever stores
  `Complete` `DecodedBuffer`s, so protection only applies to cache-backed (non-streaming) playback.

### F4 — LOW (confirmed): `MIN_BUFFER_FRAMES` prebuffer is documented but never enforced
- **Statement:** `MIN_BUFFER_FRAMES` is `#[allow(dead_code)]` with no production reference, yet the
  `get_or_load_streaming` doc comment claims playback begins once that many frames are available — it does
  not, so streaming playback can start before data arrives, producing brief leading silence/underrun.
- **Evidence:**
  - `pub const MIN_BUFFER_FRAMES: usize = 2560;` with `#[allow(dead_code)]` — `src/audio/streaming.rs:267-270`.
  - The misleading doc: `get_or_load_streaming` comment "playback can begin as soon as MIN_BUFFER_FRAMES are
    available" — `src/cache/mod.rs:84-87`. The function returns the `Streaming` buffer immediately
    (`mod.rs:126`, `190`) and the Play handler pushes the sample right away (`src/main.rs:791`, `796`).
- **Severity:** LOW. Leading silence that self-corrects within a few callbacks; not corruption.
- **Fix:** Either gate stream start on `frames_available() >= MIN_BUFFER_FRAMES` (with a timeout so a tiny
  or slow file still plays), **or** delete the dead constant and rewrite the `get_or_load_streaming` doc
  comment so it no longer promises behavior that does not exist. The simplest honest change (removal +
  doc fix) is acceptable per YAGNI unless the prebuffer is wanted; if F1 is fixed by "don't start a looping
  play until complete," a prebuffer is independently low-value for non-looping streams, so removal is the
  likely choice — follow DECISIONS.md (see Behavior-change notes).

## Caveats (refuted / over-stated — do not chase ghosts)

- **F1 universality over-stated.** The loop-buzz is **conditional**, not present on every looped streaming
  play. When `buffer_frames == 0` the loop guard (`buffer_frames > 0`) is false and forward playback hits
  the non-looping break, producing *silence*, not garbage. The buzz manifests only when playback position
  catches up to the loaded frame count before the next decode chunk arrives. Write the failing test to
  reproduce that specific race (small initial chunk / stalled growth), not a generic "every play buzzes"
  claim.
- **Chunked-vs-one-shot resampler divergence (LOW) is consistency-only, not an audible glitch.** The
  streaming path uses `StreamingDecoder` + `ChunkedResampler` (`src/cache/mod.rs:203-249`) which zero-pads
  the final partial chunk and truncates to `expected_output_frames` without compensating rubato's startup
  delay (`src/audio/chunked_resampler.rs:217-235`), while disk-cache hits / local files use the one-shot
  `decoder::decode_file` (`src/cache/mod.rs:113-121`, `131-135`). The existing `matches_full_decode` test
  tolerates the small diff. **Do not** rewrite the resampler this sprint. If F2's promotion naturally makes
  a given file always resolve to one decode path on replay, note it; otherwise log to `docs/bugs.md` for
  Sprint 9 and move on.
- **Disk atomicity and HTTP revalidation are Sprint 3, not here.** They appear in the STREAMING-CACHE dump
  but are explicitly owned by Sprint 3 (security & file safety). Reference, do not duplicate.

## Tasks (ordered, TDD-first)

Use the **Sprint 0 render harness** (`render(state, frames_per_block, blocks)` plus `rms`, `peak`,
`band_energy`, `max_inter_sample_delta`) for all audio-output assertions. Build a **fake incrementally-filled
streaming buffer** in-test (a `SampleBuffer::Streaming(Arc<RwLock<StreamingBuffer>>)`) so no network is
needed — `StreamingBuffer::new`, `append`, and `mark_complete` are already public (`streaming.rs:49-82`).

1. **F1 — failing render-harness test for the loop-buzz race (write first).**
   - In `src/audio/mixer.rs` tests (or a new `tests/streaming_loop_test.rs` using the harness), build a
     `StreamingBuffer` with a known total but append only a *small* prefix (smaller than one callback's
     frame count) of a recognizable non-constant ramp, leave it `Loading`, wrap it in a
     `SampleBuffer::Streaming`, and construct a looping `ActiveSample` (`new_with_id(..., loop_mode=true,
     crossfade_samples=0)`).
   - Push it into a `MixerState` and `render` several callback-sized blocks. **Assert the failure
     symptom:** the output is *not* a tight repetition of the tiny prefix — e.g. `max_inter_sample_delta`
     across the rendered buffer stays below a click threshold (no sawtooth wrap discontinuity), and/or the
     rendered RMS/`band_energy` does not show the high-frequency "buzz" signature a few-ms loop would
     create. Confirm it **fails** against current code.
   - **Fix:** in `advance_position` (`mixer.rs:399-414`) and the per-frame wrap (`mixer.rs:620-627`), only
     wrap when the buffer is complete (or wrap against the true total and emit silence for unloaded frames).
     A helper on `ActiveSample` like `fn loop_boundary(&self) -> Option<usize>` returning `None` while the
     buffer is `Streaming` and not `is_complete()`, else `Some(self.buffer.frames())`, keeps both wrap sites
     consistent. Re-run: the test passes; output is silence/non-looping until complete, correct looping
     after.
   - **Regression guard:** add a second test where the *same* streaming buffer is then `mark_complete()`d
     (full ramp appended) and re-rendered, asserting it now loops cleanly against the true total with no
     inter-sample click at the loop point (reuses the harness click assertion).

2. **F2 — failing cache test for promotion + bounded `active_loads` (write first).**
   - In `src/cache/mod.rs` tests, drive a streaming load to completion *without network* by exercising the
     promotion path directly: after a streaming load is tracked in `active_loads` and its buffer is
     `mark_complete()`d, the manager must (a) remove the `active_loads` entry and (b) hold the URL in the
     memory cache as a `Complete` entry. Assert `active_loads.is_empty()`/`len()==0` and
     `memory_cache.contains(url)` (or that a subsequent `get_or_load_streaming(url)` returns
     `is_complete()==true`, not a re-streamed buffer). Confirm it **fails**.
   - **Fix:** wire promotion at decode completion (reuse `cleanup_completed_loads`,
     `src/cache/mod.rs:289-322`): drop the `#[allow(dead_code)]`, call it from the decode-completion path
     (or opportunistically from the command loop after each cache-locking command). Re-run green.
   - **Replay test:** call the streaming-load path twice for the same URL with completion in between; assert
     the second call returns a `Complete` buffer (memory-cache hit), not `Streaming`.

3. **F3 — failing memory-cache test for protection + size accounting (write first).**
   - In `src/cache/memory.rs` tests, set a `max_size` that fits exactly N buffers, fill it, mark one
     **playing**, and add one more so eviction runs. Assert the playing buffer is **not** evicted (it is in
     the runtime path now, not just tests) and that `memory_usage_bytes()` equals the true resident total.
     For the chosen approach, also assert that after an eviction the reported size still matches what is
     actually held. Confirm it **fails** against current (inert) wiring.
   - **Fix:** either wire `mark_playing`/`mark_not_playing` into the Play / sample-finish paths (drop their
     `#[allow(dead_code)]`) and correct accounting, **or** remove the `playing` design and make accounting
     honest. Re-run green. Keep the existing `test_playing_entries_never_evicted` (memory.rs:444-465) passing.

4. **F4 — decide and implement the `MIN_BUFFER_FRAMES` resolution.**
   - If **removing:** delete the constant (`streaming.rs:267-270`) and its test
     (`test_min_buffer_frames_constant`, `streaming.rs:418-425`), and rewrite the `get_or_load_streaming`
     doc comment (`src/cache/mod.rs:84-87`) to describe what actually happens (returns immediately;
     callback emits silence for unloaded frames). Build clean (`cargo build --release`, `-D warnings`,
     clippy).
   - If **enforcing:** write a failing test first that a streaming play does not emit leading silence beyond
     a tolerance, then gate stream start on `frames_available() >= MIN_BUFFER_FRAMES` with a timeout. Given
     YAGNI and that F1 already restricts looping until complete, removal is the likely smallest change —
     **flag per DECISIONS.md** before deleting if there is any doubt the prebuffer is wanted.

5. **Build/lint/format clean.** `cargo build --release` with zero warnings, `cargo clippy --all-targets
   -- -D warnings`, `cargo fmt --check`. Removing `#[allow(dead_code)]` from now-used functions must not
   re-introduce dead-code warnings elsewhere — if wiring leaves something unused, that is a signal the
   wiring is incomplete; do not silence it with a new `allow`.

6. **Update `docs/bugs.md`** with any out-of-scope discovery (e.g. the resampler divergence if not fixed),
   and commit atomically to the branch with a clear message.

## Files to create / touch

- **Touch `src/audio/mixer.rs`** — defer/guard the loop wrap in `advance_position` (lines 394-429) and the
  per-frame wrap in `mix_sample_into_output` (lines 607, 620-627); optional `loop_boundary` helper on
  `ActiveSample`. Add mixer-level render-harness tests.
- **Touch `src/cache/mod.rs`** — wire promotion + `active_loads` removal on decode completion (around
  `decode_streaming`'s `mark_complete`, lines 251-254; reuse `cleanup_completed_loads`, lines 289-322; drop
  its `#[allow(dead_code)]`); fix/clarify the `get_or_load_streaming` doc comment (lines 84-87). Add cache
  tests for promotion / bounded `active_loads` / replay.
- **Touch `src/cache/memory.rs`** — wire or remove `mark_playing`/`mark_not_playing` (lines 179-197); fix
  size accounting; add/adjust tests for protection + accounting.
- **Touch `src/audio/streaming.rs`** — remove (or keep + enforce) `MIN_BUFFER_FRAMES` (lines 267-270) and
  the corresponding test (lines 418-425), per the F4 decision.
- **Touch `src/main.rs`** — only if F2/F3 wiring requires the Play handler to call promotion/`mark_playing`
  (Play dispatch around lines 649-799). Keep changes minimal; do not refactor the cache locking (Sprint 2).
- **Create (optional):** `tests/streaming_loop_test.rs` (or add to existing mixer test module) using the
  Sprint 0 render harness + a fake incrementally-filled streaming buffer.
- **Touch `docs/bugs.md`** as needed.

## Verification

- **Lane A (Docker / Linux) — the only required lane this sprint:** `scripts/validate.sh` is green:
  `cargo build --release` (`-D warnings`), clippy `-D warnings`, `fmt --check`, and `cargo test` all pass,
  including:
  - the F1 render-harness loop-buzz test (fake incrementally-filled streaming buffer → no tight-loop buzz /
    no wrap-point click; clean loop after `mark_complete`);
  - the F2 cache tests (promotion to memory cache, `active_loads` bounded, replay hits cache not stale
    streaming buffer);
  - the F3 memory-cache tests (`mark_playing` protection active in the runtime path, size accounting
    correct under eviction, bounded cache stays ≤ `max_memory_mb` with a playing buffer);
  - the F4 outcome (constant/test removed + doc corrected, or prebuffer enforced with its test).
  No network is used — all streaming is simulated via `StreamingBuffer` append/`mark_complete`.
- **Lane B (native macOS real device):** not required for this sprint (no device-format or live-input
  change). If the cache/streaming wiring is touched in ways that could affect real playback, a quick
  `scripts/validate.sh --native` smoke is welcome but is not a gate here.
- **Lane C (manual Windows):** nothing to add to `MANUAL-VERIFICATION.md` this sprint.

## Acceptance criteria

Copied verbatim from `SPRINT-TRACKER.md` (Sprint 4):

- [ ] Looping a still-streaming buffer no longer wraps the growing loaded length (loop deferred until complete / wraps on total estimate); render-harness asserts no tight-loop buzz on a fake incrementally-filled buffer `[A]`
- [ ] Completed streams promoted to memory cache; replaying a finished URL hits the cache, not a stale streaming buffer; `active_loads` bounded `[A]`
- [ ] `mark_playing` eviction protection wired; size accounting fixed; bounded cache stays under `max_memory_mb` with a playing buffer `[A]`
- [ ] `MIN_BUFFER_FRAMES` prebuffer enforced or removed (no misleading dead code) `[A]`

## Behavior-change / changelog notes

These are user-visible behavior changes — note them in `CHANGELOG.md` / `README.md`:

- **`loop:true` on an HTTP/streaming source no longer buzzes.** A looping play of a still-downloading URL
  now does not loop until the buffer is complete (or loops against the true total, emitting silence for
  not-yet-loaded frames). Before, it could replay a tiny growing prefix. **Document the new behavior**
  (looping defers until the stream finishes / wraps on the full length) so operators understand why a
  looped stream may take until download-complete to begin its first loop.
- **Replaying a finished streamed URL now serves a cached buffer** instead of re-streaming, and streamed
  audio now counts against the configured memory limit / participates in LRU. This changes memory-usage
  reporting and eviction timing — note it.
- **`MIN_BUFFER_FRAMES` (F4):** per DECISIONS.md **D14** the dead constant + misleading comment are **removed**;
  no prebuffer is added (silence-until-loaded is acceptable). Internal/doc change only — no user-visible behavior
  change. If, while implementing, leading silence proves objectionable, note a follow-up in `docs/bugs.md`.
- **Eviction protection (F3):** per DECISIONS.md **D12**, rely on `Arc` keep-alive (skip eviction when
  `strong_count() > 1`) and fix the size accounting; the dead `mark_playing` design is removed, not wired.

## Definition of Done

Lane A green (build `-D warnings`, clippy clean, `fmt --check`, all tests incl. the four new test groups
above) · F1 loop-wrap fixed and proven by a render-harness test on a fake incrementally-filled streaming
buffer · F2 promotion + bounded `active_loads` + cache-hit-on-replay landed · F3 eviction protection wired
(or design dropped with honest accounting) and size accounting fixed · F4 resolved (enforced or removed,
no misleading dead code/doc) · out-of-scope discoveries logged in `docs/bugs.md` · `CHANGELOG.md`/`README.md`
updated for the behavior changes · decisions taken per DECISIONS.md for any prebuffer-enforcement / dropped
guarantee · committed atomically to the branch as units complete · `cargo build --release` warning-free.
No test disabled, `#[ignore]`d, or weakened to pass.
