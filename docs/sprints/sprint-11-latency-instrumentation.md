# Sprint 11 — Latency Instrumentation & Baselines

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 10 |
| Effort | M |
| Lanes | A (Docker) + B (native macOS) |
| Subagents | Optional (metric plumbing / bench harness can parallelize after task 1) |

## Goal

Make first-start latency measurable: a six-stage play-latency model, an RT-safe
command-receipt → first-mixed-sample metric surfaced on `/metrics`, an offline latency
integration test, and criterion baselines for the cold/warm scenarios the owner cares about
(cold local, cold disk-cached HTTP, uncached HTTP windowed, warm cache hit). Record the baseline
numbers in this document — they are the before/after evidence that gates Sprint 12's acceptance.

## Why

The owner's priority is cold first-play latency, and Sprint 12 will change the cold-play
architecture. Nothing in the tree measures play latency today (no counter, no bench, no test
asserts a startup deadline), so without this sprint the program cannot prove its wins or catch
regressions. Measurement must precede optimization.

## Scope

**In scope**
- Stage-timestamp model (D50) for the play path in `main.rs`, logged per play.
- `AddSample`/`AddStreamedSource` enqueue-time carried to the audio thread; first-mix latency
  published via pre-allocated atomics (the xruns pattern).
- `/metrics` exposure + tests; a new `tests/latency_test.rs`; criterion baseline benches.

**Out of scope** (owned by other sprints — coordinate, do not duplicate)
- Any latency *optimization* (Sprint 12). This sprint must not change play-path behavior.
- RT logging/counter conversions for resample errors etc. (Sprint 13 — this sprint adds new
  counters but does not convert existing `tracing` call sites).
- Per-streamed-source ring-fill gauges (deferred LOW, `docs/bugs.md`).

## Findings addressed

All citations re-verified against the current tree during Sprint 10.

### F1 — No first-start latency measurement exists anywhere
- **Statement:** There is no metric, log span, test, or bench measuring the time from command
  receipt to the first audible/mixed sample, for any of the play paths.
- **Verified at:** grep of `src/` for latency counters returns only DSP-internal uses
  (`pc.output_latency()` etc., `src/audio/mixer.rs:417-421`); `/metrics`
  (`src/http/handlers.rs:82`) exposes xruns/cache/uptime but no play-latency field;
  `benches/loading_benchmark.rs` and `benches/mixer_benchmark.rs` cover decode and mix
  throughput, not command-to-first-sample.
- **Severity:** blocks the program (cannot verify Sprint 12).
- **Evidence:** the owner's cold-start complaint was confirmed by code reading
  (sprint-10 doc), not by any measurement the daemon could have reported.
- **Fix:** D50 stage model + atomics + `/metrics` + tests + benches, below.

### F2 — The proven RT-safe counter pattern to reuse
- **Statement:** Not a defect — the xruns counter is the existing pattern for publishing from
  the RT thread, and the new metric must follow it rather than inventing another mechanism.
- **Verified at:** `src/audio/engine.rs:426-444` (`xruns: Arc<AtomicU64>`,
  `fetch_add(1, Ordering::Relaxed)` in the error callback); created at `src/main.rs:582`;
  surfaced in `handle_status`/`handle_metrics` (`src/http/handlers.rs`).
- **Fix:** same shape: `Arc<AtomicU64>` set with `Ordering::Relaxed`, created control-side,
  threaded into the callback state and `AppState`.

## Caveats (refuted / over-stated — do not chase ghosts)

- **Do not timestamp every frame or every buffer.** The RT-side cost must be one
  `Instant::now()` *only* on the callback that first mixes a given sample (one branch per
  sample per buffer; the sample already tracks whether it has produced output — add a
  `started: bool` if no equivalent exists). `Instant::now()` is a vDSO `clock_gettime` on
  Linux/macOS — no allocation, no syscall trap in practice — and is the same primitive the RT
  thread can already reach via tracing timestamps. Do not add `SystemTime` (not monotonic).
- **Do not box or grow `AudioCommand`.** `AddSample(ActiveSample)` is deliberately a move-in
  variant with an allowed `large_enum_variant` (`src/rt_engine.rs:22-36`). Add the enqueue
  instant and the latency atomics as fields **on `ActiveSample`/`StreamedSource` themselves**
  (built control-side), not as a new wrapper around the command.
- **The control-side stages are logging, not atomics.** Only t4→t5 crosses into the RT thread.
  Do not build a cross-thread struct for t0–t4; one `tracing::info!` with the per-stage
  durations is enough, plus retaining the last/max values control-side for `/metrics`.

## Tasks (ordered, TDD-first)

1. **Alloc-harness guard first.** In `tests/alloc_harness.rs`, extend the callback-step test (or
   add a sibling) asserting that mixing a newly added sample whose first-mix publication fires
   is 0 alloc / 0 free. Write it against the planned field names so it fails to compile, then
   drives the implementation. This is the gate that keeps the metric RT-safe.

2. **First-mix publication (t4→t5).** Add to `ActiveSample` and `StreamedSource`:
   `enqueued_at: Option<std::time::Instant>` and
   `first_mix_latency: Option<Arc<AtomicU64>>` (nanos; `None` for test-constructed samples so
   existing tests are untouched). In the mix path, on the first buffer in which the sample
   produces output, store `enqueued_at.elapsed().as_nanos() as u64` with `Relaxed` and clear the
   gate. Control side (`src/main.rs` play handlers) creates the Arc per play and also clones it
   into a small control-side aggregate: `latency_last_ns` / `latency_max_ns` / `plays_measured`
   (`AtomicU64`s in the struct that already feeds `AppState`). Run the task-1 harness test green.

3. **Stage timestamps t0–t4 (D50).** In `handle_command`'s Play/stream-play paths
   (`src/main.rs`), capture `Instant`s at: t0 command receipt (entry), t1 parsed (already true
   at entry for MQTT — capture at the dispatch boundary so HTTP and MQTT measure the same
   thing), t2 cache decision made (after `should_window_local` / windowing choice), t3
   decode/prebuffer ready (after `get_or_load_streaming_with_freshness` returns, or after the
   prebuffer gate for windowed plays), t4 command pushed onto the ring. Emit one
   `tracing::info!(target: "latency", ...)` per play with the four durations and the play kind
   (cache-hit / cold-local / disk-cached-http / http-streaming / windowed). TDD: a unit test in
   `main.rs`'s test module drives `handle_command` with a tiny WAV and asserts the captured log
   line (use the existing log-capture pattern, e.g. the `CaptureLayer` near `src/main.rs:2653`)
   contains monotone non-negative stages.

4. **`/metrics` exposure.** Extend `handle_metrics` (`src/http/handlers.rs:82`) with
   `latency.play_to_first_mix_ns{last,max}` and `latency.plays_measured`, read from the
   control-side atomics. TDD in `tests/http_api_test.rs`: drive a real play through the test
   router/state, step the offline callback once so the first mix happens, then GET `/metrics`
   and assert `plays_measured >= 1` and `last > 0`. (Real values only — never placeholder
   numbers; follow the existing real-value status tests' style.)

5. **`tests/latency_test.rs` (new).** Render-harness-style integration: build the engine state
   offline (no device), drive `handle_command` for (a) a memory-cache-hit play, (b) a cold local
   play, (c) a windowed play, step the callback, and assert: stages are populated and monotone;
   first-mix latency is recorded; for (a) the t2→t3 stage is ~0. **Do not assert absolute wall
   times** (CI jitter) — assert presence, ordering, and that the cache-hit decode stage is
   bounded by a generous ceiling (e.g. < 250 ms) so a reintroduced blocking decode on the hit
   path fails loudly.

6. **Criterion baselines.** Extend `benches/loading_benchmark.rs` (or add
   `benches/latency_benchmark.rs` mirroring its harness) with: cold local full-load
   (`get_or_load_streaming_with_freshness` on a generated multi-second WAV, fresh cache each
   iter), warm memory-cache hit, `probe_local_file` cost, and windowed
   prebuffer-ready time (spawn + gate at default config). Run on Lane A and record the table
   below.

7. **Record baselines.** Fill in this table (Lane A numbers; add Lane B/Pi numbers when
   available) and commit it with the sprint:

   | Scenario | Baseline (pre-Sprint-12) | Notes |
   |---|---|---|
   | Warm memory-cache hit → buffer returned | _measure_ | |
   | Cold local full-load (60 s WAV) → buffer returned | _measure_ | expected ≈ full decode time |
   | Cold disk-cached HTTP → buffer returned | _measure_ | expected ≈ full decode time |
   | `probe_local_file` (WAV) | _measure_ | |
   | Windowed play → prebuffer-ready | _measure_ | includes 5 ms-poll quantization |

## Files to create / touch

- **Create:** `tests/latency_test.rs`; optionally `benches/latency_benchmark.rs`.
- **Touch:** `src/audio/mixer.rs` (`ActiveSample`/`StreamedSource` fields + first-mix
  publication), `src/rt_engine.rs` (only if the drain path must thread anything — prefer not),
  `src/main.rs` (stage capture, per-play Arc creation, control-side aggregate, log line),
  `src/http/handlers.rs` (+`AppState` plumbing in `src/http/mod.rs`) for `/metrics`,
  `tests/alloc_harness.rs`, `tests/http_api_test.rs`, `benches/loading_benchmark.rs`,
  `docs/http-api.md` (document the new `/metrics` fields).

## Verification

### Lane A (Docker / Linux)
- `scripts/validate.sh` green: build/clippy `-D warnings`, fmt, full tests including the new
  alloc-harness case, `latency_test.rs`, and the `/metrics` test.
- Criterion benches run; the baseline table above is filled in and committed.

### Lane B (native macOS real device)
- `scripts/validate.sh --native` green; on the real device, play a cached file and confirm
  `/metrics` reports a sub-50 ms `play_to_first_mix_ns.last` (sanity, not a hard gate).

### Lane C (manual Windows)
- None.

## Acceptance criteria (verbatim from SPRINT-TRACKER.md, §Sprint 11)

- [ ] Six-stage play-latency model (D50) captured per play and logged; first-mix latency published from the audio thread via pre-allocated atomics with the alloc harness proving the publication is 0 alloc / 0 free `[A]`
- [ ] `/metrics` exposes `latency.play_to_first_mix_ns{last,max}` + `plays_measured` with real values asserted by an HTTP test `[A]`
- [ ] `tests/latency_test.rs` drives cache-hit, cold-local, and windowed plays offline and asserts populated, monotone stages `[A]`
- [ ] Criterion baselines recorded in sprint-11 doc for warm hit, cold local, cold disk-cached HTTP, probe, and prebuffer-ready `[A]`
- [ ] Real-device sanity: cached play shows sub-50 ms first-mix latency on `/metrics` `[B]`

## Behavior-change / changelog notes

- New `/metrics` fields and a new per-play `latency` info log line — **additive only**;
  document in `docs/http-api.md`. No play-path behavior may change in this sprint.

## Definition of Done

Lane A green (build `-D warnings` · clippy · fmt · full suite · new alloc-harness case ·
latency + metrics tests · benches run) · Lane B green with the real-device sanity check ·
baseline table filled in and committed in this doc · `docs/http-api.md` updated ·
out-of-scope discoveries logged in `docs/bugs.md` · committed atomically with a clear message ·
`cargo build --release` warning-free.
