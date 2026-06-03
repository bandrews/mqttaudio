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

echo "==> generating throwaway TLS certificates"
/usr/local/bin/gen-test-certs.sh /etc/mosquitto/tls

echo "==> starting mosquitto broker (plain 1883 + TLS 8883)"
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
    echo "ERROR: mosquitto did not become ready on 1883" >&2
    exit 1
fi
echo "    plain listener is up"

tls_up=0
for _ in $(seq 1 40); do
    if mosquitto_pub --cafile /etc/mosquitto/tls/ca.crt -h localhost -p 8883 \
        -t mqttaudio/health -m ok >/dev/null 2>&1; then
        tls_up=1
        break
    fi
    sleep 0.25
done
if [ "$tls_up" -ne 1 ]; then
    echo "ERROR: mosquitto TLS listener did not become ready on 8883" >&2
    exit 1
fi
echo "    TLS listener is up"

# Broker tests enabled; device-opening tests stay skipped (not --ignored, no device).
# MQTTAUDIO_TLS_CA enables the TLS connect test against the 8883 listener.
echo "==> cargo test (MQTTAUDIO_BROKER_TESTS=1, MQTTAUDIO_TLS_CA set)"
MQTTAUDIO_BROKER_TESTS=1 MQTTAUDIO_TLS_CA=/etc/mosquitto/tls/ca.crt cargo test

echo "VALIDATION OK"
