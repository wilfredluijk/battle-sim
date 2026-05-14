# Multi-stage build for the naval-server binary. The runtime image is a slim
# Debian with just the compiled binary — no Rust toolchain, no source.

# ---- builder ----------------------------------------------------------------
FROM rust:1.83-bookworm AS builder
WORKDIR /build

# Cache the dependency layer separately from the source layer.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

COPY src ./src
COPY tests ./tests
# Re-touch main.rs so cargo re-builds with the real source.
RUN touch src/main.rs && cargo build --release --bin naval-server

# ---- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/target/release/naval-server /usr/local/bin/naval-server

# Replay output directory; mount a volume here from the integration harness.
RUN mkdir -p /app/replays
VOLUME ["/app/replays"]

EXPOSE 7878
ENTRYPOINT ["/usr/local/bin/naval-server"]
