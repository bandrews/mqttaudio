#!/usr/bin/env bash
# ABOUTME: Generates a throwaway CA + localhost server cert for the Lane A TLS broker.
# ABOUTME: Self-contained test PKI, regenerated each run; never used outside the container.
set -euo pipefail
OUT="${1:-/etc/mosquitto/tls}"
mkdir -p "$OUT"

# Certificate authority (the client trusts this; the Rust test points its ca_path here).
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$OUT/ca.key" -out "$OUT/ca.crt" \
    -subj "/CN=mqttaudio-test-ca"

# Server key + signing request.
openssl req -newkey rsa:2048 -nodes \
    -keyout "$OUT/server.key" -out "$OUT/server.csr" \
    -subj "/CN=localhost"

# Sign the server cert with a localhost SAN (rustls verifies the SAN, not the CN).
openssl x509 -req -in "$OUT/server.csr" \
    -CA "$OUT/ca.crt" -CAkey "$OUT/ca.key" -CAcreateserial \
    -out "$OUT/server.crt" -days 3650 \
    -extfile <(printf '[v3]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\n') \
    -extensions v3

# World-readable so mosquitto can read them regardless of which user it drops to.
# Acceptable only because this is an ephemeral, throwaway test PKI.
chmod 644 "$OUT"/ca.crt "$OUT"/ca.key "$OUT"/server.crt "$OUT"/server.key
rm -f "$OUT/server.csr"
