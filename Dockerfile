# Multi-stage build: the Vite-built spectator dist must exist before the Rust crate is
# compiled, because `server/src/net.rs` baked the spectator bundle into the binary at
# compile time via `include_str!`. Order: node (build dist) → rust (build server, embedding
# the freshly-produced dist) → debian-slim (runtime).

# ---- stage 1: spectator build ----
FROM node:20-alpine AS spectator-build
WORKDIR /spectator

# Copy lockfile + manifest first so `npm ci` can be cached when only source changes.
COPY spectator/package.json spectator/package-lock.json ./
RUN npm ci

COPY spectator/ ./
RUN npm run build
# Produces /spectator/dist/{index.html,index.js,index.css}

# ---- stage 2: server build ----
FROM rust:1.86-slim AS server-build
WORKDIR /build

# Dep-cache trick: copy just the manifests, build a stub binary so cargo fetches & compiles
# all dependencies, then bring in the real sources. A code-only change re-uses this layer.
COPY server/Cargo.toml server/Cargo.lock ./server/
RUN mkdir -p server/src \
    && echo 'fn main() {}' > server/src/main.rs \
    && cd server && cargo build --release || true \
    && rm -f server/src/main.rs

# Real sources + the spectator dist that the include_str! macros need.
COPY server/ ./server/
COPY --from=spectator-build /spectator/dist /build/spectator/dist
RUN cd server && cargo build --release
# Produces /build/server/target/release/naval-server

# ---- stage 3: runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
RUN mkdir -p /app/replays

COPY --from=server-build /build/server/target/release/naval-server /usr/local/bin/naval-server

EXPOSE 7878
ENTRYPOINT ["/usr/local/bin/naval-server"]
CMD ["--port", "7878", "--replay-dir", "/app/replays"]
