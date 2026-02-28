# ---- Builder ----
FROM rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Dependency cache: copy manifests, create dummy src, build deps only.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/animus.rs && \
    echo "" > src/lib.rs && \
    cargo build --release --bin animus || true && \
    rm -rf src

# Copy real source + migrations, build for real.
COPY src/ src/
COPY migrations/ migrations/
RUN cargo build --release --bin animus

# ---- Runtime ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -r animus && useradd -r -g animus animus

COPY --from=builder /app/target/release/animus /usr/local/bin/animus

USER animus
ENTRYPOINT ["animus"]
