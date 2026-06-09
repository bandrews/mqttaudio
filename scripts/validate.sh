#!/usr/bin/env bash
# ABOUTME: One-command validation gate for mqttaudio used to gate every sprint.
# ABOUTME: Default runs Lane A (Docker/Linux); --native runs Lane B on this host.
#
# Usage:
#   ./scripts/validate.sh            Lane A: build + lint + tests inside Docker, broker tests
#                                    against a containerized mosquitto (no real audio device).
#   ./scripts/validate.sh --native   Lane B: the same cargo steps on this host PLUS the
#                                    real-device smoke test (cargo test --include-ignored).
#
# Exits non-zero on the first failing step.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

run_native() {
    echo "== Lane B (native host) =="
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    RUSTFLAGS="-D warnings" cargo build --release
    # Property/fuzz suite for the untrusted JSON surfaces, surfaced on its own.
    cargo test --test fuzz_command_config
    # --include-ignored also runs the gated real-device smoke test.
    MQTTAUDIO_DEVICE_TESTS=1 cargo test -- --include-ignored
    echo "== Lane B OK =="
}

run_docker() {
    echo "== Lane A (Docker, Linux) =="
    if ! command -v docker >/dev/null 2>&1; then
        echo "ERROR: docker is not installed or not on PATH." >&2
        echo "       Start Docker and re-run, or use --native for the host lane." >&2
        exit 1
    fi
    docker build -t mqttaudio-validate -f docker/validate.Dockerfile docker
    # The repo is mounted at /build; artifacts go to a named volume so host
    # (macOS) target/ is never mixed with Linux build output.
    docker run --rm \
        -v "$REPO_ROOT":/build \
        -v mqttaudio_validate_target:/target \
        -v mqttaudio_validate_cargo:/usr/local/cargo/registry \
        -e CARGO_TARGET_DIR=/target \
        mqttaudio-validate
    echo "== Lane A OK =="
}

case "${1:-}" in
    --native) run_native ;;
    "")       run_docker ;;
    *)
        echo "usage: $0 [--native]" >&2
        exit 2
        ;;
esac
