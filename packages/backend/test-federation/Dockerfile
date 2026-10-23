ARG BUN_VERSION=1.4.4
ARG RUST_VERSION=1.99.0

FROM rust:${RUST_VERSION}-bookworm AS rust
FROM oven/bun:${BUN_VERSION}

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=rust /usr/local/rustup /usr/local/rustup
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
