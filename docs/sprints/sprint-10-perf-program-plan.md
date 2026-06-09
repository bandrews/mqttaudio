# Sprint 10 — Performance & RT-Hardening Program Plan

| Field | Value |
|-------|-------|
| Status | Done |
| Depends on | 0–9 |
| Effort | M (doc-only) |
| Lanes | none (no code changes) |
| Subagents | No — principal-engineer evaluation and planning |

## Goal

Evaluate the core audio loop and the daemon's performance characteristics — **cold first-start
latency** above all — and turn that evaluation into four executable sprint plans
(sprints 11–14), each in the established sprint-doc format, with every finding's file:line
citation verified against the live tree, the tracker and DECISIONS.md updated, and the stale
entries in `docs/bugs.md` reconciled. This sprint produces documents only; the code changes are
sprints 11–14.

## Why

The owner reports warm-play latency is good but cold first-play latency is not: *"it felt like
the whole sample was having to load before the first frames were pumped."* That observation is
correct, and the root cause is confirmed in code (see the evaluation below): two of the three
cold full-load paths block on a complete one-shot decode before any sound, even though the
progressive machinery that fixes this already exists and is used by the third path. Beyond the
cold-start fix, the evaluation found a small set of real RT-path residuals (all already
documented in `docs/bugs.md`), several measurable secondary latency costs, and a quality backlog
the owner has scoped. None of it is measurable today because no first-start latency
instrumentation exists — so instrumentation comes first.

## The evaluation (what was found, verified)

### Cold first-start latency — the centerpiece finding

`CacheManager::get_or_load_streaming_with_freshness` (`src/cache/mod.rs:215-309`) has three cold
paths, only one of which is progressive:

| Cold path (full-load mode) | Behavior today | First sound |
|---|---|---|
| Uncached HTTP | `start_streaming_load` (`mod.rs:312-366`) spawns `decode_streaming`, returns `SampleBuffer::Streaming` immediately | ~network + one callback |
| **Local file** | `mod.rs:296-301` awaits a one-shot `decoder::decode_file` of the entire file | after full decode+resample (seconds for long files on Pi-class hardware) |
| **Disk-cached HTTP** | `mod.rs:252-274` — the same blocking full decode | after full decode |

The doc comment at `mod.rs:210-213` ("A streaming buffer is returned immediately and playback
begins right away") is only true for uncached HTTP. The mixer plays `Streaming` buffers
generically (silence for unloaded frames), the play handler's streaming branch is logging-only
(`src/main.rs:1664-1704`), and `StreamingDecoder` already decodes local files progressively for
the windowed path (`src/audio/streamed_source.rs:55-64`). **Sprint 12** unifies the three paths.

### Supporting latency findings

- **No instrumentation:** nothing measures command-receipt → first-mixed-sample anywhere in
  `src/`. The xruns counter (`src/audio/engine.rs:426-444`, `src/main.rs:582`) is the proven
  pattern for RT-safe counters. **Sprint 11** builds the metric and baselines before any
  optimization ships.
- **Prebuffer gate polls:** the windowed-play gate sleeps 5 ms per check
  (`src/main.rs:1245-1264`), adding ~2.5 ms average quantization. **Sprint 12.**
- **Local probe re-runs every play:** `probe_local_file` (`src/cache/strategy.rs:56-87`) opens
  and header-parses the file on every uncached play — and windowed plays never enter the memory
  cache, so every replay of a large auto-windowed file pays it again (~5–20 ms). **Sprint 12.**
- **HTTP header open is serialized:** `open_http_stream` (`src/cache/http_stream.rs:362`)
  completes before any local setup work begins. **Sprint 12.**

### Stale-while-revalidate race — traced, partially refuted

The suspicion was that background revalidation could drop a cache entry out from under an
in-flight streaming load. The trace (read-only, this sprint):

- `revalidate_stale_http` (`src/cache/mod.rs:619-641`) only touches URLs that are **both**
  disk-cached and memory-resident (the `:624` filter). An `active_loads` streaming entry exists
  only for a URL that was *neither* when the load started (path order at `mod.rs:229-282`), and
  the HTTP full-load streaming path does not write the disk cache (documented in
  `docs/bugs.md`). The sets cannot overlap, so **today's revalidation tick is safe — refuted.**
- **A real race exists in `invalidate`** (`mod.rs:605-611`, the `cache_reload` path): it clears
  the memory and disk caches but **not `active_loads`**. An in-flight streaming load of the same
  URL survives the invalidation, and `cleanup_completed_loads` (`mod.rs:462-492`) later promotes
  its **stale** content into the memory cache; a concurrent play also joins the stale stream at
  `mod.rs:244-246`. **Sprint 12 fixes this.**
- **Post-unification hazard:** once disk-cached HTTP files decode progressively (Sprint 12), a
  revalidation that atomically replaces the disk file mid-decode leaves the decoder reading the
  old inode and the promotion publishing stale content as fresh. Promotion must be
  generation-guarded. **Owned by Sprint 12's unification task.**
- Side observation: promotion copies the whole buffer (`guard.data().to_vec()`,
  `mod.rs:483`) — a transient 2× spike bounded by `full_load_max_bytes`. Recorded in
  `docs/bugs.md`; not scheduled.

### Core audio loop / RT-path residuals (all pre-documented in `docs/bugs.md`, now owned)

- Ducking `apply_target` allocates on first duck of a voice (`src/audio/ducking.rs:493-503`,
  `entry(change.voice.clone()).or_insert_with(..)`). → **Sprint 13.**
- The voice pool is a soft reserve: >`MAX_VOICES` (256, `src/audio/mixer.rs:919`) reallocs on
  the RT thread; the locked D18 steal/reject policy is unimplemented
  (`src/rt_engine.rs:270`). → **Sprint 13** (owner re-approved 2026-06-09).
- A Speed command that toggles pitch correction constructs/drops the signalsmith stretcher on
  the RT thread (`src/rt_engine.rs:230-237` → `set_speed_with_mode` `src/audio/mixer.rs:470-478`
  → `enable_pitch_correction` `:406-411`). → **Sprint 13.**
- `tracing` calls reachable from RT threads: `src/audio/mixer.rs:374` (negative-speed-with-pitch
  warn inside `set_speed`, applied via `apply_mutation` on the callback) and
  `src/audio/input.rs:424` / `:441` / `:479` (capture-callback resample paths).
  `input.rs:565` is the cpal *error* callback (accepted, mirrors xruns). → **Sprint 13.**
- Scratch buffers grow on the RT thread if the device block size grows mid-run
  (`src/audio/mixer.rs:1430-1432` pitch scratch; `src/audio/input.rs:228`/`:554` input
  scratch). Steady-state-safe; unbounded only for pathological devices. → **Sprint 13.**
- Pitch correction silently no-ops on streaming buffers (`src/audio/mixer.rs:1243-1246`,
  `:1408-1416`) with no operator feedback. → control-side warning in **Sprint 13**; the play
  path keeps full decode for pitch plays per **Sprint 12**.

### Quality backlog dispositions (owner-scoped 2026-06-09)

- Resampler sinc `Linear` → `Cubic` (R1): **approved** → Sprint 14 (D59).
- Dead config field `audio.channel_names`: remove → Sprint 14 (D60).
- `/command` 400 body shape → `CommandResponse` JSON: **approved as daemon plumbing** →
  Sprint 14 (D61).
- Wire `WebSocketLogLayer` into the subscriber: **approved as daemon plumbing** →
  Sprint 14 (D62).
- **Already done — `docs/bugs.md` was stale (reconciled this sprint):** `DiskCache` stable
  hashing (SHA-256 at `src/cache/disk.rs:153-158`, Sprint 3); the two file-level
  `#![allow(dead_code)]` banners (gone from the tree; per-item dispositions shipped with the
  Sprint 9 final gate); the per-sample `windowed` flag (`src/http/handlers.rs:749`).
- **Out of this program** (owner scope decision): reverse-loop crossfade, per-input capture
  meters, webui front-end changes, speed>1 anti-aliasing (D27), PI drift control (D33), LFE-bus
  low-pass (D32), ALSA name-matcher hardening, binary-redeclares-modules refactor.

## Scope

**In scope:** the four sprint docs (11–14), the tracker section, DECISIONS.md D50–D62, the
`docs/bugs.md` reconciliation, and this evaluation record.

**Out of scope:** all code changes (sprints 11–14 own them).

## Deliverables

1. `sprint-11-latency-instrumentation.md` — measurement before optimization.
2. `sprint-12-first-start-latency.md` — the cold-start unification + secondary latency wins.
3. `sprint-13-rt-path-hardening.md` — the remaining RT-thread alloc/log residuals.
4. `sprint-14-quality-and-correctness.md` — resampler, config, HTTP plumbing backlog.
5. `SPRINT-TRACKER.md` — "Performance & RT-hardening program" section: status board rows 10–14 +
   lane-tagged acceptance checklists. The Charter applies verbatim.
6. `DECISIONS.md` — new section, **D50–D62**.
7. `docs/bugs.md` — stale entries marked RESOLVED with citations; open residuals cross-linked to
   their owning sprint.

## Sequencing and dependencies

```
10 (docs) → 11 (measure) → 12 (cold-start latency)   ← the user-priority path
                        ↘ 13 (RT hardening)          ← needs 11's /metrics counters surface
14 (quality)            ← independent of 12/13 except soft ordering after 13 for the
                          RT-logging counters it documents
```

Sprint 12 before 13: latency is the owner's stated priority, and 12's before/after evidence
depends only on 11. Sprint 14 last: it is the lowest-risk batch and its resampler change
re-pins decode-output tests that 12 also touches — running it last avoids double re-pinning.

## Verification

Doc-only sprint. Verification = the four sprint docs follow the sprint-09 structural conventions
(header table; Goal/Why/Scope/Findings/Caveats/Tasks/Files/Verification/Acceptance/
Behavior-changes/DoD); every `Verified at:` citation was re-read against the tree during this
sprint; tracker and DECISIONS are consistent with the docs; `cargo` is untouched.

For the executing agents (recorded in each sprint doc): the canonical gate is Lane A
`scripts/validate.sh` (Docker, `docker/validate.Dockerfile`, rust:1.95-bookworm). If Docker is
unavailable, approximate on the host (`apt-get install libasound2-dev pkg-config libssl-dev
clang libclang-dev mosquitto`, then the `docker/run-validation.sh` steps) — but a sprint is not
`Done` until real Lane A passes. Lane B boxes are checked only after the partner's native macOS
run.

## Definition of Done

All four sprint docs committed in the established format with verified citations · tracker
section + status board + acceptance checklists added · DECISIONS.md D50–D62 recorded ·
`docs/bugs.md` reconciled (stale entries resolved with citations, open residuals cross-linked) ·
this doc's Status set to `Done` · no source changes in the commit.
