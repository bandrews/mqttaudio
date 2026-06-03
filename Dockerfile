FROM rust:1.95-bookworm AS builder

RUN apt-get update && apt-get install -y \
    libasound2-dev libssl-dev pkg-config clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

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
    libasound2 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/mqttaudio /usr/local/bin/mqttaudio

CMD ["mqttaudio", "--config", "/config/mqttaudio.json"]
