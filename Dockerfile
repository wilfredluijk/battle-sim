# Pinned official build images; static musl binary avoids runtime OS packages.
FROM node:24-bookworm-slim@sha256:2fe369e969550cde8e867afc3fe370b260140cab4a23d467074295b42163d553 AS spectator-build
WORKDIR /spectator
COPY spectator/package.json spectator/package-lock.json ./
RUN npm ci
COPY spectator/ ./
RUN npm run check && npm test -- --run && npm run build

FROM rust:1.95.0-slim-trixie@sha256:e14e87345b4d5964ddcc3491d27ee046a0f23820f340c3c1e24da6880141f7c0 AS server-build
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools \
    && rustup target add x86_64-unknown-linux-musl
WORKDIR /build
COPY server/ ./server/
COPY --from=spectator-build /spectator/dist /build/spectator/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry cd server && cargo build --release --locked --target x86_64-unknown-linux-musl && mkdir -p /runtime/replays

# No shell, package manager, curl, shared libraries, or writable root filesystem.
FROM scratch AS runtime
ARG RELEASE=development
LABEL org.opencontainers.image.title="battle-sim" org.opencontainers.image.version=$RELEASE
ENV BATTLE_RELEASE=$RELEASE
COPY --from=server-build --chown=10001:10001 /build/server/target/x86_64-unknown-linux-musl/release/naval-server /usr/local/bin/naval-server
COPY --from=server-build --chown=10001:10001 /runtime/ /app/
WORKDIR /app
USER 10001:10001
EXPOSE 7878
HEALTHCHECK --interval=15s --timeout=4s --start-period=10s --retries=3 CMD ["/usr/local/bin/naval-server", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/naval-server"]
CMD ["--port", "7878", "--replay-dir", "/app/replays"]
