FROM rust:1.95-bookworm AS builder

RUN apt-get update && apt-get install -y \
    libasound2-dev libssl-dev pkg-config clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Embed the source revision in `/version` when the caller pins one. A missing
# arg remains visibly unknown rather than pretending the image is released.
ARG MQTTAUDIO_GIT_SHA=unknown
ENV MQTTAUDIO_GIT_SHA=$MQTTAUDIO_GIT_SHA

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy sources to pre-compile dependencies
RUN mkdir -p src benches \
    && echo 'fn main() {}' > src/main.rs \
    && echo 'fn main() {}' > benches/mixer_benchmark.rs \
    && echo 'fn main() {}' > benches/loading_benchmark.rs \
    && cargo build --release \
    && rm -rf src benches

# Copy real source and rebuild (only the crate itself recompiles)
COPY src ./src
COPY benches ./benches
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libasound2 ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/mqttaudio /usr/local/bin/mqttaudio

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
  CMD if [ "${MQTTAUDIO_HTTP_REQUIRE_AUTH:-false}" = "true" ]; then \
        test -n "${MQTTAUDIO_HTTP_AUTH_TOKEN:-}" && curl -fsS -H "Authorization: Bearer ${MQTTAUDIO_HTTP_AUTH_TOKEN}" http://127.0.0.1:8080/ready; \
      else curl -fsS http://127.0.0.1:8080/ready; fi

CMD ["mqttaudio", "--config", "/config/mqttaudio.json"]
