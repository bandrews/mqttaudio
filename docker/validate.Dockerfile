# Linux validation image for mqttaudio (Lane A).
# Bundles the Rust toolchain, the audio/TLS build dependencies, and a mosquitto
# broker. The repo is mounted at /build at run time; CMD runs the validation
# script (fmt + clippy + build + broker-backed tests).
# Pinned to match the project's host toolchain (1.87) so Lane A and Lane B
# run the same clippy lint set.
FROM rust:1.87-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        libasound2-dev libssl-dev pkg-config build-essential \
        clang libclang-dev mosquitto mosquitto-clients \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt

COPY validate-mosquitto.conf /etc/mosquitto/validate.conf
COPY run-validation.sh /usr/local/bin/run-validation.sh
RUN chmod +x /usr/local/bin/run-validation.sh

WORKDIR /build
CMD ["/usr/local/bin/run-validation.sh"]
