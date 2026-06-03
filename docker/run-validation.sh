#!/usr/bin/env bash
# ABOUTME: In-container validation steps for Lane A (run by validate.Dockerfile's CMD).
# ABOUTME: Runs fmt/clippy/release-build, starts mosquitto, then broker-backed tests.
set -euo pipefail
cd /build

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

echo "==> cargo build --release (RUSTFLAGS=-D warnings)"
RUSTFLAGS="-D warnings" cargo build --release

echo "==> starting mosquitto broker"
mosquitto -c /etc/mosquitto/validate.conf -d
broker_up=0
for _ in $(seq 1 40); do
    if mosquitto_pub -h localhost -p 1883 -t mqttaudio/health -m ok >/dev/null 2>&1; then
        broker_up=1
        break
    fi
    sleep 0.25
done
if [ "$broker_up" -ne 1 ]; then
    echo "ERROR: mosquitto did not become ready" >&2
    exit 1
fi
echo "    broker is up"

# Broker tests enabled; device-opening tests stay skipped (not --ignored, no device).
echo "==> cargo test (MQTTAUDIO_BROKER_TESTS=1)"
MQTTAUDIO_BROKER_TESTS=1 cargo test

echo "VALIDATION OK"
