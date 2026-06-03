# Sprint 3 — Security & File Safety

| Field | Value |
|-------|-------|
| Status | Not started |
| Depends on | 0 |
| Effort | M |
| Lanes | A (Docker) |
| Subagents | Optional (TLS plumbing / allowlist enforcement / disk-cache integrity can fan out) |

## Goal

Make it **possible** to lock the daemon down — file allowlist, MQTT TLS, HTTP auth, crash-safe cache — without
**forcing** it. The audit found the security controls are either dead config or absent. We add real, working
controls, but every one of them is **opt-in / opt-out with backward-compatible defaults**, because this app is
often deliberately run wide open on a trusted private network where the network layer is the security boundary,
and for legacy reasons that setup may be hard to change. **Nothing that works today may stop working.** Secure
*options* by default-available; not secure-by-coercion. None of this touches the real-time audio path.

> **Backward-compatibility is explicitly authorized for this sprint** (partner directive). Per CLAUDE.md we
> normally need sign-off before adding backward-compat behavior — that sign-off is granted here: preserve the
> anonymous/open/easy mode. The guiding rule: a user who configures nothing keeps today's behavior (plus
> non-fatal warnings); a user who opts in gets enforcement.

## Why

The daemon accepts file paths and URLs from untrusted MQTT/HTTP callers and feeds them straight into the
filesystem and HTTP stack with no gatekeeping. Verified against current source:

- **`security.allowed_directories` is declared, defaulted, validated, and then never consulted by any
  production code path.** A repo-wide `rg allowed_directories` (re-run while writing this) returns hits only in
  `src/config.rs`: the field (`config.rs:192`), its default (`config.rs:198`), a `#[cfg(test)]`-only accessor
  (`config.rs:524`), and tests. `main.rs`, `src/cache/`, and `src/audio/decoder.rs` contain zero references. A
  Play command's file path flows through `get_or_load_streaming` (`cache/mod.rs:88`) → `decode_file`
  (`cache/mod.rs:131`) → `File::open(path)` (`decoder.rs:72`) with no allowlist check anywhere. Any caller can
  read arbitrary local files **if they choose to configure an allowlist and expect it to work** — today it
  silently does nothing.
- **MQTT runs cleartext only.** `connect_mqtt` builds `MqttOptions` with the default TCP transport
  (`client.rs:38`) and never calls `set_transport`; `client.rs:4` doesn't even import a TLS type. Credentials
  (`set_credentials`, `client.rs:45`) and all command payloads cross the wire in the clear — with no *option* to
  encrypt even if the user wants to.
- **HTTP auth is optional and partial.** `auth_middleware` waves every request through when no token is set
  (`routes.rs:24-27`), only command routes get the middleware (`routes.rs:100-101`) while `/status*`, `/health`,
  and `/ws` are unauthenticated (`routes.rs:84-97, 107`), and the token is also accepted as a `?token=` query
  parameter (`routes.rs:45-55`). Bind address defaults to `127.0.0.1` (`config.rs:403`). This open mode is a
  **feature** for trusted-network installs — the gap is that there is no *opt-in* way to tighten it and no
  warning when exposed.
- **The disk cache can hand decode a truncated file as if it were valid.** `download_and_cache` overwrites the
  deterministic hash path in place with a single `fs::write(&cache_path, &bytes)` (`disk.rs:289`); `is_cached`
  only checks `path.exists()` (`disk.rs:180-187`) and ignores the `file_size` field it already stores
  (`disk.rs:26`). A crash mid-overwrite leaves a short file that `is_cached` calls good. (Pure integrity bug —
  no backward-compat tension.)
- **The cache key is `DefaultHasher` despite a comment claiming SHA-256.** `cache_filename_for_url`
  (`disk.rs:143-167`) uses `std::collections::hash_map::DefaultHasher`, whose output is explicitly unstable
  across Rust versions, under a doc comment that says "Uses first 12 chars of SHA256 hash" (`disk.rs:142`).
- **HTTP content is never revalidated.** `etag`/`last_modified`/`last_validated` are stored
  (`disk.rs:263-302`) but never read; `config.cache.revalidate_after_seconds` (`config.rs:161`, default 300 at
  `config.rs:180`) is referenced nowhere outside `config.rs`.

## Scope

**In scope**
- Enforce `allowed_directories` (canonicalize + reject) before `File::open` — **only when an allowlist is
  configured**; empty list = allow-all (today's behavior) + a one-time startup warning.
- Add **opt-in** MQTT TLS (`mqtt.tls` config + rumqttc TLS feature). Default transport unchanged (plain TCP).
  Non-fatal warning when credentials would cross a non-loopback link in cleartext.
- Add **opt-in** HTTP auth enforcement: keep "no token = open" working; add a loud startup **warning** when
  bound to a non-loopback address without a token; add an opt-in `http.require_auth` (default `false`) that
  enforces on all routes; constant-time token compare; keep `?token=` as a documented convenience (it exists for
  simple clients). Do **not** refuse to start and do **not** remove the open path.
- Crash-safe disk-cache writes (temp file + atomic rename) with size verification on load — always on (pure
  integrity, no opt-out needed).
- Replace `DefaultHasher` with a stable content hash; bump `CacheMetadata.version`.
- HTTP revalidation **or** honest removal of the dead `revalidate_after_seconds` knob — decision point (Caveats).

**Out of scope** (log to `docs/bugs.md` if touched)
- The `std::sync::Mutex`-held-across-`.await` cache-manager issue and `spawn_blocking` for `decode_file`
  (Sprint 2).
- Streaming buffer / memory-cache correctness (looping a growing buffer, eviction protection) — Sprint 4.
- Any change to the audio callback or mixer.

## Findings addressed

### F3.1 — `allowed_directories` is never enforced (HIGH, confirmed) — fix is opt-in
- **Statement:** The configured filesystem allowlist is dead config; a user who sets it expecting protection
  gets none. (When unset, allow-all is the intended, preserved default.)
- **Evidence:** `rg allowed_directories` across `src/` hits only `config.rs` — field at `config.rs:192`,
  default at `config.rs:198`, a `#[cfg(test)]` accessor at `config.rs:524-529`, and tests. The production load
  paths feed the raw path through `get_or_load_streaming` (local branch `cache/mod.rs:129-135`), and through
  `get_or_load` (`cache/mod.rs:326`, local branch `PathBuf::from(file_path)` at `cache/mod.rs:354`), both
  reaching `decode_file` (`decoder.rs:64`) → `File::open(path)` (`decoder.rs:72`).
- **Severity:** HIGH (when an allowlist is configured-but-ignored).
- **Fix:** When `allowed_directories` is **non-empty**, canonicalize the requested local path and reject
  anything not contained in a canonicalized allowlist entry **before** `File::open`, returning a clear error.
  When it is **empty** (default), allow any path (today's behavior) and emit a one-time startup warning that no
  allowlist is configured. Applies only to local file paths, not `http(s)://` URLs.

### F3.2 — No MQTT TLS option (HIGH, confirmed) — fix is opt-in
- **Statement:** MQTT is plain TCP only with no option to encrypt; credentials and commands are cleartext.
- **Evidence:** `client.rs:4` imports no TLS/`Transport` type. `MqttOptions::new` at `client.rs:38` +
  `set_keep_alive`/`set_clean_session`/`set_credentials` (`client.rs:39-45`); `set_transport` is never called.
  `Cargo.toml:20` has `rumqttc = "0.24"` with no TLS feature compiled.
- **Severity:** HIGH (for anyone who *wants* a secure broker link and can't get one).
- **Fix:** Enable a rumqttc TLS feature and add an **opt-in** `mqtt.tls` config block (CA path, optional client
  cert/key, optional insecure-skip-verify for self-signed test brokers). Call `set_transport(Transport::Tls…)`
  **only when `mqtt.tls` is configured.** **Default transport stays plain TCP regardless of port** (do not
  auto-enable TLS on 8883 — that would silently break a legacy plain-on-8883 setup). Emit a non-fatal warning
  when `set_credentials` is used over a plain transport to a non-loopback server. Plumb through the
  `connect_mqtt` callsite (`main.rs:311-317`) and `MqttConfig` (`config.rs:12-24`).

### F3.3 — HTTP auth is optional/partial; no opt-in enforcement; no exposure warning (MEDIUM) — preserve open mode
- **Statement:** Open mode (no token) is intended and must keep working, but there is no opt-in way to enforce
  auth and no warning when the control API is exposed to a network unauthenticated.
- **Evidence:** `auth_middleware` returns `Ok(next.run(...))` immediately when `state.auth_token` is `None`
  (`routes.rs:24-27`). Only `command_routes` receive the middleware (`routes.rs:100-101`); `status_routes` and
  `/health` (`routes.rs:84-97`) and `/ws` (`routes.rs:107`) are unauthenticated. The token rides in `?token=`
  (`routes.rs:45-55`) and is compared with `==` (`routes.rs:38, 50`), not constant-time. CORS is `Any` when
  `cors_permissive` (`routes.rs:111-116`). Bind defaults to `127.0.0.1` (`config.rs:403`) but accepts `0.0.0.0`.
- **Severity:** MEDIUM.
- **Fix (backward-compatible):**
  - **Keep** the "no token = open" behavior and the `?token=` convenience param (document the latter's
    log-leak caveat). Do **not** refuse to start.
  - Add an **opt-in** `http.require_auth: bool` (default `false`). When `true`, all routes — including
    `/status*` and `/ws` — require a valid `Bearer` token (or the `?token=` param).
  - Emit a **loud one-time startup warning** when `!addr.ip().is_loopback()` and `auth_token` is `None`
    ("HTTP control API exposed on <addr> without authentication") — informational, not fatal.
  - Replace the `==` token check with a constant-time comparison (`routes.rs:38, 50`). Keep CORS configurable;
    leave the default unchanged.

### F3.4 — Non-atomic disk-cache write + existence-only validity (LOW/MEDIUM) — always on
- **Statement:** A crash mid-write leaves a truncated cache file that `is_cached` reports as valid; decode fails.
- **Evidence:** `download_and_cache` writes in place with `fs::write(&cache_path, &bytes)` (`disk.rs:289`), then
  `save_metadata()` (`disk.rs:306`). `cache_filename_for_url` is deterministic so re-downloads overwrite the
  same file. `is_cached` only checks `path.exists()` (`disk.rs:180-187`) and ignores stored `file_size`
  (`disk.rs:26`).
- **Severity:** LOW/MEDIUM.
- **Fix:** Write to a temp file in `files/` and atomically rename into place; update metadata only after a
  successful rename. Verify on-disk length against `entry.file_size` in `is_cached`; on mismatch return false so
  the caller re-downloads. (No backward-compat tension — this only makes a corrupt state recover.)

### F3.5 — Cache key uses `DefaultHasher`, not the SHA-256 the comment claims (LOW, confirmed)
- **Statement:** The cache filename derives from `DefaultHasher`, unstable across Rust versions, contradicting
  the doc comment.
- **Evidence:** `cache_filename_for_url` (`disk.rs:143-167`) builds a `DefaultHasher`, hashes the URL, formats
  `{:016x}`, under the comment "Uses first 12 chars of SHA256 hash" (`disk.rs:142`). No `sha2`/`blake3`/`xxhash`
  dep in `Cargo.toml`.
- **Severity:** LOW.
- **Fix:** Use a stable hash — truncated SHA-256 (`sha2`) or `blake3`/`xxhash`. Bump `CacheMetadata.version`
  from `1` (`disk.rs:44`) so old-hash entries are invalidated rather than orphaned.

### F3.6 — Disk cache never revalidates HTTP content (MEDIUM)
- **Statement:** Stored `etag`/`last_modified`/`last_validated` and `revalidate_after_seconds` are dead; a
  changed server-side file is served stale indefinitely.
- **Evidence:** `is_cached` (`disk.rs:180-187`) returns true on existence with no conditional GET.
  `download_and_cache` stores the validators (`disk.rs:263-302`) but nothing issues
  `If-None-Match`/`If-Modified-Since`. `config.cache.revalidate_after_seconds` (`config.rs:161`, default `300`)
  is read nowhere outside `config.rs`.
- **Severity:** MEDIUM.
- **Fix (decision):** Either implement conditional revalidation (when `last_validated` is older than
  `revalidate_after_seconds`, conditional GET; `304` → refresh, `200` → replace via the atomic-write path) **or**
  delete the dead knob + unused validators. Don't leave validated-but-unused config. Locked in DECISIONS.md (D10).

## Caveats (decisions — locked in DECISIONS.md)

- **Open mode is a supported configuration, not a bug (overarching).** The default posture stays open and
  anonymous; every control added here is opt-in. The non-negotiable changes are: make the allowlist *work when
  set*, make TLS *available*, warn when exposed without auth, and fix cache integrity. Do **not** introduce any
  default that breaks an existing trusted-network install.
- **Empty-allowlist semantics (F3.1).** Empty list = allow-all (preserve today's behavior) + a one-time startup
  warning. This is the intended default; document it loudly.
- **Default MQTT transport (F3.2).** Stays plain TCP even on 8883; TLS is purely opt-in. (Auto-enabling TLS on
  8883 would break legacy plain-8883 brokers — explicitly avoided.)
- **Revalidation vs. removal (F3.6).** Implementing conditional GET adds network round-trips on cache hits;
  removing the knob drops a documented feature. Follow DECISIONS.md (D10). Either way, no validated-but-ignored field
  may remain.
- **Nothing here is refuted.** All six findings were re-verified against current source. The reframing is about
  *how* (opt-in vs forced), not *whether* the gaps exist.

## Tasks (ordered, TDD)

> TDD per the Charter: failing test first, watch it fail, minimum code to pass, verify, refactor green. These are
> control-plane/cache changes (no audio output), so the Sprint 0 render harness is **not** used — assertions are
> on returned errors, file bytes on disk, router responses, and a live TLS broker.

1. **Allowlist enforcement (opt-in).** Promote the `#[cfg(test)]` accessor at `config.rs:524` to a normal
   method and add `fn is_path_allowed(&self, path: &Path) -> bool` (or a free
   `check_allowed(path, &allowlist) -> Result<PathBuf, _>`).
   - **Failing test first:** with allowlist `["/opt/sounds"]`, `/etc/passwd` is rejected, `/opt/sounds/a.wav`
     accepted, `../` traversal out of an allowed dir rejected after canonicalization; with an **empty** allowlist
     **any** path is allowed (the preserved default).
   - Wire the check into the local-file branches of both cache load paths (`cache/mod.rs:129-135` and
     `cache/mod.rs:354`) **before** `decode_file`/`File::open`; pass the allowlist into the cache manager at
     construction. Emit a one-time startup warning when the allowlist is empty.
   - **Integration test:** drive a Play for `/etc/passwd` through `handle_command` (Sprint 0) with an allowlist
     set and assert it errors and opens nothing; with no allowlist, it proceeds (open by default).

2. **MQTT TLS (opt-in).** Add the `mqtt.tls` config to `MqttConfig` (`config.rs:12-24`) and the rumqttc TLS
   feature to `Cargo.toml` (`Cargo.toml:20`).
   - **Failing test first:** a pure `fn select_transport(cfg: &MqttConfig) -> Transport` returns TLS **only**
     when `mqtt.tls` is configured, and plain TCP otherwise (including on port 8883 with no tls config).
   - Call `set_transport` in `connect_mqtt` (`client.rs:38`) based on that selection; plumb args through
     `main.rs:311-317`. Add the cleartext-credentials-to-non-loopback warning.
   - **Lane A integration test:** mosquitto with TLS in the Docker pipeline; assert `connect_mqtt` succeeds on
     `8883` with `mqtt.tls` set and a published command is received (gated behind `MQTTAUDIO_BROKER_TESTS=1`).
     Also assert a plain connection to a plain broker still works (open mode preserved).

3. **HTTP auth (opt-in enforcement + exposure warning).**
   - **Failing test first (warning):** a pure `fn exposure_warning(addr, &auth, require_auth) -> Option<String>`
     returns a warning when `!is_loopback` and `auth_token` is `None` and `require_auth` is false; `None` for a
     loopback bind. (Startup logs it; does **not** fail.)
   - **Failing test first (enforcement):** an `axum`/`tower` test with `require_auth = true` asserting a
     **status** route returns `401` without a token and `200` with the correct `Bearer` token; and with
     `require_auth = false` (default) the same status route returns `200` with no token (open mode preserved).
   - **Failing test first (command auth unchanged):** with a token set, a command route returns `401` without /
     `200` with the Bearer token, and `200` with the correct `?token=` query (the convenience param stays). Add a
     constant-time-compare unit test.
   - Implement: `http.require_auth` config (default `false`) applied to all routes when true; the exposure
     warning in `start_server` after `addr` parse (`http/mod.rs:59`); constant-time compare replacing `==`
     (`routes.rs:38, 50`); keep `?token=` and the open path. CORS stays configurable (no default change).

4. **Atomic disk-cache write + size verify.** In `disk.rs`:
   - **Failing test first:** leave a short file (< `file_size`) at the entry path; assert `is_cached` returns
     `false`. Then assert `download_and_cache` writes via `<name>.tmp` + `fs::rename` so the final path is never
     partial.
   - Implement temp-file + rename (`disk.rs:289`), then `save_metadata`; add the size check to `is_cached`
     (`disk.rs:180-187`).

5. **Stable content hash.** Replace `DefaultHasher` in `cache_filename_for_url` (`disk.rs:143-167`).
   - **Failing test first:** a fixed URL maps to a fixed expected filename (extend
     `test_cache_filename_generation`, `disk.rs:333`). Fix the misleading comment (`disk.rs:142`).
   - Implement with `sha2` truncated to 12 hex chars (matching the comment's intent) or `blake3`. Bump
     `CacheMetadata::default().version` (`disk.rs:44`) and discard older-version entries on load.

6. **Revalidation decision (F3.6).** Per DECISIONS.md (D10) — implement conditional GET (test `304`/`200` against
   a local Lane-A test server) **or** remove the dead `revalidate_after_seconds` + unused validators (test that
   config no longer needs the field) and note in README.

## Files to create / touch

- `src/config.rs` — un-`cfg(test)` the allowlist accessor (`524`), add `is_path_allowed`; add **opt-in**
  `mqtt.tls` (`12-24`) and `http.require_auth`; resolve `revalidate_after_seconds` (`161, 180`) per F3.6.
- `src/cache/mod.rs` — allowlist check before `decode_file` in `get_or_load_streaming` (`129-135`) and
  `get_or_load` (`354`); plumb allowlist into the cache manager; revalidation hook if implementing.
- `src/cache/disk.rs` — atomic temp-file+rename (`289`); size verify in `is_cached` (`180-187`); stable hash
  (`143-167`); bump `version` (`44`); fix the comment (`142`); conditional-GET if implementing F3.6.
- `src/mqtt/client.rs` — `select_transport` + opt-in `set_transport` in `connect_mqtt` (`38`); cleartext-creds
  warning.
- `src/http/mod.rs` — exposure warning + `require_auth` plumb in `start_server` after `addr` parse (`59`).
- `src/http/routes.rs` — `require_auth` gating of all routes when set; constant-time compare (`38, 50`); keep
  `?token=` and open mode; CORS stays configurable.
- `src/main.rs` — pass TLS config to `connect_mqtt` (`311-317`); pass allowlist to the cache manager.
- `Cargo.toml` — rumqttc TLS feature (`20`); `sha2` (or `blake3`); any dev-dep for the revalidation test server.
- `docker/` (Lane A) — TLS-enabled mosquitto config + test certs so the TLS connect test runs in the pipeline.
- Tests — new modules under `tests/` or `#[cfg(test)]` for each task.

## Verification

**Lane A (Docker / Linux) — this sprint is Lane A only:**
- Allowlist: with an allowlist set, Play `/etc/passwd` is refused and an allowed path plays; **with no
  allowlist, any path plays (open default preserved)**; `../` traversal rejected.
- MQTT TLS: with `mqtt.tls` set, `connect_mqtt` succeeds against mosquitto+TLS on `8883`; **without it, plain
  TCP still connects** (including on 8883).
- HTTP auth: default (no token) status + command routes still return `200` (open mode); with `require_auth`,
  status routes require a token; command routes honor `Bearer` and the `?token=` convenience; constant-time
  compare unit test passes; the exposure warning fires (and only warns) on a non-loopback bind without auth.
- Cache integrity: a short/partial cache file is rejected by `is_cached`; the atomic write never exposes a
  truncated file; a pinned URL maps to a pinned filename across runs.
- `scripts/validate.sh` exits non-zero on any failure; build `-D warnings`, clippy `-D warnings`,
  `fmt --check` all clean.

**Lane B (native macOS):** No device-dependent behavior; the full host suite must still pass green.

**Lane C (manual Windows):** Nothing Windows-specific unless TLS cert handling needs a Windows note (add only if
discovered).

## Acceptance criteria (mirror the tracker)

- [ ] `allowed_directories` enforced **when configured** (canonicalize + reject); empty list = allow-all preserved with a startup warning; playing `/etc/passwd` is refused under a configured allowlist; tests cover both `[A]`
- [ ] **Opt-in** MQTT TLS supported (`mqtt.tls`); default transport stays plain TCP (incl. 8883); TLS connect verified against mosquitto+TLS; plain connect still works `[A]`
- [ ] Open HTTP mode preserved by default; opt-in `http.require_auth` enforces (incl. status/ws); loud non-fatal warning on non-loopback bind without auth; constant-time token compare; `?token=` convenience kept `[A]`
- [ ] Disk-cache writes are temp-file+rename with size verify on load; kill-mid-write leaves no "valid" truncated file; stable content hash replaces `DefaultHasher` `[A]`
- [ ] HTTP revalidation implemented (If-None-Match/If-Modified-Since) **or** dead `revalidate_after_seconds` knob removed `[A]`

## Behavior-change / changelog notes

Record in `CHANGELOG.md` / README. **The headline is that defaults do not change** — open/anonymous installs
keep working untouched. New, opt-in, or non-breaking:
- **Allowlist now works when set** (previously inert). Empty allowlist = unchanged allow-all + a startup warning.
- **MQTT TLS** is a new opt-in `mqtt.tls` option; default transport unchanged.
- **HTTP `require_auth`** is a new opt-in flag; open mode and `?token=` unchanged; a new non-fatal warning logs
  when the API is exposed without auth.
- **Cache hash change** bumps `CacheMetadata.version`, invalidating existing cached entries on first run after
  upgrade (they re-download). Internal, not user-facing config.
- **Revalidation** either adds conditional GETs on cache hits or removes the `revalidate_after_seconds` field —
  **decided upfront in DECISIONS.md** (Caveats).

## Definition of Done

Lane A green · Lane B green (full host suite) · allowlist/TLS/auth/atomic-write/stable-hash tests landed and
passing · open-mode-preserved tests passing · revalidation decision implemented and documented · behavior notes
in `CHANGELOG.md`/README · no test disabled or `#[ignore]`d to pass · out-of-scope discoveries logged to
`docs/bugs.md` · work committed atomically to the branch as units complete (your own clear messages) ·
`cargo build --release` warning-free.
