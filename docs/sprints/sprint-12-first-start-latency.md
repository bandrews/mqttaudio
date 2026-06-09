# Sprint 12 — First-Start Latency

| Field | Value |
|-------|-------|
| Status | Done (Lane A via documented host approximation — see tracker note; Lane B pending partner) |
| Depends on | 11 |
| Effort | L |
| Lanes | A (Docker) + B (native macOS) |
| Subagents | Optional (F4/F5/F6 are independent of F1 after the F1 design lands) |

## Goal

Make cold first-play latency independent of file length. Unify the three cold full-load paths so
local files and disk-cached HTTP decode progressively into a `StreamingBuffer` and start playing
on the next callback — exactly as uncached HTTP already does — and close the secondary latency
costs around it (poll-quantized prebuffer gate, repeated local probes, serialized HTTP header
open). Fix the cache-invalidation race with in-flight streaming loads, including the new
generation hazard the unification itself introduces. Prove every win against the Sprint 11
baselines.

## Why

The owner's report — *"it felt like the whole sample was having to load before the first frames
were pumped"* — is the literal behavior for a cold local file or a disk-cached HTTP file in
full-load mode: `get_or_load_streaming_with_freshness` awaits a one-shot decode of the whole
file before returning, so a 60-second background bed can take seconds to start on Pi-class
hardware. The progressive machinery (`StreamingDecoder` → `StreamingBuffer` →
`active_loads` → promotion) already exists, is tested, and serves the uncached-HTTP path; the
mixer emits silence for not-yet-decoded frames and the play handler treats streaming buffers
generically. This is a wiring unification, not new machinery — the elegant-simplification kind
of fix: one load path instead of three.

## Scope

**In scope**
- F1: progressive decode for local-file and disk-cached-HTTP full-loads (D51), with the pitch
  exception, the `start_position` gate, freshness stat recording, and generation-guarded
  promotion.
- F2: `invalidate`/`cache_reload` vs `active_loads` race (D52).
- F3: event-driven prebuffer gate (D53).
- F4: probe-result cache (D54).
- F5: overlapped HTTP header open (D55).
- F6: latency tuning documentation (docs-only; defaults unchanged).

**Out of scope** (owned by other sprints — coordinate, do not duplicate)
- The latency metric/benches themselves (Sprint 11 — must already be `Done`).
- The control-side warning when pitch correction targets a streaming buffer (Sprint 13 F6).
- RT-thread allocation residuals (Sprint 13).
- Persisting the HTTP full-load streaming path to disk (documented as accepted in
  `docs/bugs.md`).
- The windowed (`StreamedSource`) pipeline's design — D42 restrictions stand; only its gate's
  wakeup mechanism changes here (F3).

## Findings addressed

All citations re-verified against the current tree during Sprint 10.

### F1 — Cold local and disk-cached-HTTP full-loads block on a complete decode before any sound
- **Statement:** Two of the three cold full-load paths await a one-shot `decoder::decode_file`
  of the entire file before the play command proceeds; only uncached HTTP returns a progressive
  `SampleBuffer::Streaming` immediately. Cold first-start latency therefore scales with file
  length for the most common sources.
- **Verified at:** `src/cache/mod.rs:296-301` (local: `spawn_blocking(decode_file)` awaited),
  `:252-274` (disk-cached HTTP: same), vs `:312-366` (`start_streaming_load`: spawns
  `decode_streaming`, returns `SampleBuffer::Streaming` immediately). The doc comment at
  `:210-213` promises immediate playback for all paths. Mixer/handler readiness:
  `src/audio/streaming.rs:189-235` (`get_sample_or_silence` semantics), `src/main.rs:1664-1704`
  (streaming branch is logging-only). Local progressive decode already exists for the windowed
  path: `src/audio/streamed_source.rs:55-64` (`StreamingDecoder::new` over `std::fs::File`).
- **Severity:** high (the program's headline issue).
- **Evidence:** owner report + code trace; `decode_streaming` (`mod.rs:368-430`) is already
  generic in spirit — `StreamingDecoder::new` accepts any `symphonia` `MediaSource`, and `File`
  is one.
- **Fix (D51):**
  1. Generalize the streaming-load spawn: factor `start_streaming_load`/`decode_streaming` so
     the decode source is any `MediaSource + Send` (the `HttpStreamReader` today; a
     `std::fs::File` for the two new paths). Keep one `ActiveLoad` registry and one promotion
     path.
  2. Local full-load (`mod.rs:284-308`): after the existing `is_path_allowed` check (keep it
     **before** any spawn), `record_local_stat` (see below), open the `File`, build the
     `StreamingBuffer` with the probe-known frame estimate when available, register in
     `active_loads`, spawn the decode, return `SampleBuffer::Streaming` immediately.
  3. Disk-cached HTTP (`mod.rs:252-274`): same, opening the cached file path; keep the
     `revalidate_if_due` call ahead of the open.
  4. **Pitch exception:** pitch correction requires `Complete` buffers
     (`src/audio/mixer.rs:1243-1246`, `:1408-1416`). The play handler knows
     `pitch_correction`/speed at dispatch; when a play requests pitch correction, call a
     full-decode variant (the current blocking path, kept as a named method) so behavior is
     unchanged for pitch plays. Plumb this as an explicit argument to the cache call, not a
     global.
  5. **Freshness:** `record_local_stat` (`mod.rs:107`) must capture the stat **at load start**,
     not at promotion — if the file is edited mid-decode, the start-time stat then mismatches
     the on-disk file and the next play re-decodes (the safe direction). Add a test for exactly
     this ordering.
  6. **Generation-guarded promotion:** `cleanup_completed_loads` (`mod.rs:462-492`) must not
     publish a load whose source changed underneath it. Give `ActiveLoad` a generation
     (local: the start-time mtime+size stat; disk-cached HTTP: the disk entry's validator
     (etag/last-modified) or file stat at open). At promotion, re-check; on mismatch, drop the
     load without promoting (next play decodes fresh). This also covers the
     revalidation-swaps-the-disk-file-mid-decode hazard the unification introduces (sprint-10
     trace).
  7. **`start_position` gate:** today a full-decode play can start at any position instantly;
     a progressive buffer would emit silence until decode reaches a deep `start_position_ms`
     (`src/main.rs:1814+`). Preserve perceived behavior: when `start_position` lands beyond the
     currently loaded frames, wait for `frames()` to reach it before pushing `AddSample`, with
     the same deadline-then-start-anyway shape as the windowed prebuffer gate (reuse F3's
     notify). Local decode is fast (IO-bound, sequential), so this wait is short in practice.
- **TDD (`tests/streaming_test.rs` style, plus `tests/latency_test.rs` from Sprint 11):**
  - Failing test first: a cold local play of a generated long WAV returns a buffer with
    `is_complete() == false` within a tight deadline (e.g. < 200 ms), and the mixer can produce
    its first frames before `mark_complete`.
  - Replay after completion is served `Complete` from the memory cache (promotion intact).
  - Pitch-corrected play still returns `Complete` (blocking path preserved).
  - Edit-during-decode: start a load, modify the file, complete the decode → no promotion
    (generation guard); next play re-decodes.
  - `start_position` beyond the loaded edge gates until loaded (paused-time test) and respects
    the deadline.
  - Render-harness parity: a fully-loaded progressive buffer mixes bit-identically to the same
    file decoded by the one-shot path **per chunked-resampler caveat below** — where the rates
    differ, assert against the existing `matches_full_decode` tolerance instead of equality.

### F2 — `invalidate`/`cache_reload` leaves in-flight streaming loads alive → stale re-promotion
- **Statement:** `invalidate` clears the memory and disk caches but not `active_loads`; a
  concurrent streaming load of the same URL survives, later **promotes stale content** into the
  freshly invalidated cache, and concurrent plays join the stale stream.
- **Verified at:** `src/cache/mod.rs:605-611` (`invalidate`: memory + local_stats + disk only),
  `:244-246` (join of `active_loads`), `:462-492` (unconditional promotion).
- **Severity:** medium (real today; window is the duration of a streaming load).
- **Evidence:** sprint-10 read-only trace. Note the related suspicion about
  `revalidate_stale_http` was **refuted** — see Caveats.
- **Fix (D52):** `invalidate` marks/removes the matching `ActiveLoad` (an `abandoned` flag or
  bumped generation; the decode task keeps filling its buffer for any current listener, but
  completion skips promotion and the entry no longer joins new plays). TDD: failing test —
  start a streaming load, `invalidate` the URL mid-load, complete it, assert the memory cache
  does **not** contain the URL and a fresh play re-loads.

### F3 — The windowed prebuffer gate polls on a 5 ms sleep
- **Statement:** The gate that holds a windowed play until `prebuffer_frames` are buffered polls
  `frames_buffered` every 5 ms, adding average ~2.5 ms (worst ~5 ms) of quantization to every
  windowed start, and after F1 the same pattern would be reused for `start_position` gating.
- **Verified at:** `src/main.rs:1245-1264` (Acquire load + `tokio::time::sleep(5ms)` loop).
- **Severity:** low (small constant), but cheap and on the user-priority path.
- **Fix (D53):** the producer signals a `tokio::sync::Notify` (or a `watch` channel) when
  crossing the threshold (and on completion/error); the gate becomes
  `tokio::time::timeout(deadline, notified())` preserving the deadline-start-anyway semantics
  exactly. TDD with `tokio::time::pause()`: threshold crossing releases without consuming the
  deadline; a stalled producer still starts at the deadline.

### F4 — `probe_local_file` re-runs a header parse on every uncached and every windowed play
- **Statement:** The windowing decision probes the local file's header on each play; windowed
  plays never enter the memory cache, so every replay of a large auto-windowed file pays the
  ~5–20 ms open+parse again.
- **Verified at:** `src/cache/strategy.rs:56-87` (`probe_local_file`, full
  `StreamingDecoder::new` construction); call site `should_window_local` in `src/main.rs`
  (`:1635` region).
- **Severity:** low-medium (per-play constant on the hot path).
- **Fix (D54):** a small bounded probe cache in `CacheManager` keyed
  `(canonical_path, mtime, size)` → `Probe` (HashMap + simple FIFO/LRU cap, e.g. 256 entries).
  A changed mtime/size misses naturally. TDD: count file opens via a probe-fn seam (or assert
  via timing-free behavior: probe twice, second hit returns the identical `Probe` without
  reopening — inject an open counter in tests).

### F5 — The HTTP header open is serialized ahead of unrelated setup
- **Statement:** For HTTP plays, `open_http_stream` (DNS + TLS + headers, ~100–300 ms on cold
  connections) completes before any local setup work (voice resolution, bookkeeping) begins;
  the two are independent and can overlap.
- **Verified at:** `src/cache/http_stream.rs:362` (`open_http_stream`), call sites in the HTTP
  play decision path (`try_windowed_http_play`, `src/main.rs:1615-1627` region).
- **Severity:** low-medium (network-bound path only).
- **Fix (D55):** start the header open as a task (`tokio::spawn`/`join!`) at the top of the
  HTTP play path and await it only where its result (status/Content-Length) is needed for the
  windowing decision; preserve D49's single-request property (the opened response is *the*
  download — do not issue a second GET). TDD: a stub HTTP server asserting exactly one request,
  and the Sprint 11 t2 stage showing overlap (header wait no longer strictly additive).

### F6 — Latency tuning guidance is undocumented
- **Statement:** `stream_prebuffer_ms`, its deadline, `audio.buffer_size`, and resampler quality
  jointly determine windowed start latency, but no doc explains the trade-offs or gives
  Pi-class recommendations.
- **Verified at:** config fields in `src/config.rs`; `docs/configuration.md` has no latency
  section.
- **Severity:** docs.
- **Fix:** a "Tuning first-start latency" section in `docs/configuration.md` with
  bench-derived numbers from Sprint 11. **Defaults unchanged** (owner decision: docs-only
  unless baselines show a default is clearly wrong — if one is, raise it in the PR, do not
  change it silently).

## Caveats (refuted / over-stated — do not chase ghosts)

- **`revalidate_stale_http` is NOT racy today — refuted.** It only touches URLs both
  disk-cached and memory-resident (`src/cache/mod.rs:624`); an `active_loads` entry exists only
  for URLs that were neither (path order `:229-282`; the HTTP full-load streaming path never
  writes the disk cache). Do not "fix" the revalidation tick itself; the real items are F2
  (invalidate) and the F1 generation guard (post-unification).
- **The playing voice is already eviction-safe.** `memory_cache.remove` drops the map slot;
  a playing `ActiveSample` holds its own `Arc` (D46). Do not add keep-alive machinery.
- **Do not extend seek/speed/loop-crossfade restrictions.** F1 returns *full-load* progressive
  buffers (`SampleBuffer::Streaming`), which the mixer already plays with full `ActiveSample`
  semantics except the documented streaming deferrals (loop deferred until complete, D11/F6-S4;
  pitch requires Complete). Windowed `StreamedSource` restrictions (D42) are a different class —
  do not conflate them.
- **Chunked-vs-one-shot resampler divergence is known and tolerated** (`docs/bugs.md`,
  Sprint 4 caveat): when the file rate differs from the device rate, the progressive path's
  chunked resampling differs slightly from one-shot PCM. F1 widens *which plays* take the
  chunked path (their **first, cold** play only — promotion re-serves one-shot-equivalent data?
  No: promotion stores the streamed PCM). State this honestly: after F1, a file whose cold play
  streamed keeps the chunked-resampled PCM in cache. The divergence is inaudible
  (`matches_full_decode` tolerance) and already shipping for uncached HTTP; record the widened
  scope in `docs/bugs.md`. If the executing agent finds an *audible* divergence, stop and
  escalate per the Charter rather than shipping.

## Tasks (ordered, TDD-first)

1. F3 notify-based gate (smallest, and F1's `start_position` gate reuses it).
2. F2 invalidate-vs-active-loads fix (independent, de-risks F1's registry work).
3. F1 unification, in the sub-order given in the finding (factor → local → disk-cached HTTP →
   pitch exception → freshness ordering → generation guard → start_position gate), each step
   with its failing test first.
4. F4 probe cache.
5. F5 HTTP header overlap.
6. F6 docs; re-run the Sprint 11 benches and record the after-table below; update
   `docs/bugs.md` (widened chunked-resample scope; anything discovered).

   | Scenario | Sprint 11 baseline | After Sprint 12 |
   |---|---|---|
   | Warm memory-cache hit | ~148 ns | ~148 ns (unchanged) |
   | Cold local full-load → playable (300 s WAV) | ~211 ms (∝ length: 2.9 ms @ 5 s … 211 ms @ 300 s) | **~223 µs, constant in length** (`cold_streaming_playable/300s_file_cold`) |
   | Cold disk-cached HTTP → playable | ≈ full decode (same blocking path) | same progressive path as local (variant-asserted by `disk_cached_http_full_load_returns_a_progressive_buffer`) |
   | `probe_local_file` warm replay | ~4.4 µs per play | one stat syscall (probe cache hit, D54) |
   | Windowed prebuffer-ready (100 ms prebuffer) | ~5.16 ms (5 ms poll quantum dominated) | **~224 µs** (event gate, D53) |

   Measured 2026-06-09, Lane-A-approximation host (x86, rust 1.94.1). The audible start of a
   cold play additionally waits for the first decoded chunk (a few ms for local files) plus the
   callback period — measured live by the Sprint 11 `/metrics` first-mix probe, whose
   "first *audible* mix" definition makes it honest for progressive plays.

### Implementation deviations (recorded honestly, per the override protocol)

- **D55 was superseded by request reuse.** The header-open "overlap" premise was hollow (the
  local work to overlap totals microseconds). Implemented instead: `windowed=false` reuses the
  probe's open response for a progressive full load on a single request
  (`CacheManager::start_streaming_load_from_reader`), teeing cacheable downloads to the disk
  cache — which also closed the documented HTTP-full-load-never-persists gap. Recorded in
  DECISIONS.md (D55 amendment).
- **The D51 "pitch exception" was vacuous and became the upgrade mechanism.** Play commands
  carry no pitch parameter; pitch arrives via later Speed commands, which no-op on
  `Streaming`-variant buffers forever (the variant never changes after fill). Implemented:
  `AudioCommand::UpgradeSampleBuffer` + a reaper upgrade pass swaps the playing sample onto the
  promoted Complete buffer (identical PCM, RT-side `mem::swap`, displaced buffer dropped
  off-RT), restoring full-decode semantics within ~decode-time of a cold start. Recorded in
  DECISIONS.md (D51 amendment).
- **F4 probe-cache evidence note:** Sprint 11 measured the probe at ~4.4 µs on page-cached fast
  storage — far below the 5–20 ms estimate. The cache was built anyway (D54 locked; the win is
  real on SD-card-class storage and the implementation is ~40 lines), but its priority claim is
  corrected here.
- **Fixed in passing:** errored streaming loads used to linger in `active_loads` forever, so a
  replay of a failed URL silently joined the dead buffer; `cleanup_completed_loads` now drops
  them (test-locked). Logged in `docs/bugs.md` alongside the invalidate-race closure.

## Files to create / touch

- **Touch:** `src/cache/mod.rs` (the centerpiece: path unification, `ActiveLoad` generation,
  `invalidate`, promotion guard), `src/cache/strategy.rs` + `CacheManager` (probe cache),
  `src/cache/http_stream.rs` (only if the header-open seam needs it), `src/main.rs` (pitch
  exception plumb, gates, HTTP overlap), `src/audio/streamed_source.rs` (producer notify),
  `tests/streaming_test.rs`, `tests/revalidation_test.rs` (F2), `tests/latency_test.rs`,
  `docs/configuration.md`, `docs/bugs.md`, `CHANGELOG.md`.

## Verification

### Lane A (Docker / Linux)
- `scripts/validate.sh` green (build/clippy `-D warnings`, fmt, full suite incl. the new
  streaming/invalidate/gate tests, alloc harness untouched-green, render-harness parity).
- Sprint 11 benches re-run; the after-table is filled in and shows cold-local/disk-cached
  startup no longer scaling with file length.

### Lane B (native macOS real device)
- `scripts/validate.sh --native` green; manual: play a long cold local WAV and hear it start
  near-instantly; `/metrics` first-mix latency confirms.

### Lane C (manual Windows)
- None new.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 12)

- [ ] Cold local and disk-cached-HTTP full-loads return a progressive buffer immediately and are audible before decode completes; pitch-corrected plays keep the full decode; promotion, freshness (stat-at-start), and the generation guard are test-covered `[A]`
- [ ] `invalidate`/`cache_reload` abandons in-flight streaming loads (no stale promotion, no stale joins) `[A]`
- [ ] Windowed prebuffer gate is event-driven with deadline semantics preserved (paused-time tests) `[A]`
- [ ] Probe results cached by (path, mtime, size); warm windowed replay skips the header parse `[A]`
- [ ] HTTP header open overlaps local setup on a single request (stub-server test) `[A]`
- [ ] Before/after table recorded against Sprint 11 baselines; cold-start time no longer scales with file length `[A]`
- [ ] Long cold local file audibly starts near-instantly on the real device `[B]`

## Behavior-change / changelog notes

- **Cold full-load plays start before decode completes** (the design's stated intent, now true
  for all three paths). Observable differences, all to changelog: `/status/samples`
  `total_frames` for a just-started cold play may briefly report the loaded-so-far/estimate
  values (as uncached HTTP already does); a `start_position` deep into a cold file waits for
  decode to reach it (bounded by the gate deadline) instead of starting instantly; the cold
  play's cached PCM comes from the chunked path (inaudible divergence, see Caveats).
- **`cache_reload`/`invalidate` now also cancels in-flight loads** of that key — strictly more
  correct; changelog note.
- No config defaults change.

## Definition of Done

Lane A green (full gate + new tests + parity) · Lane B green incl. the audible cold-start check ·
before/after table committed in this doc · behavior changes in `CHANGELOG.md` ·
`docs/configuration.md` tuning section added · `docs/bugs.md` updated ·
out-of-scope discoveries logged · committed atomically · `cargo build --release` warning-free.
