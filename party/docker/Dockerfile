# syntax=docker/dockerfile:1.7
# Borealis Party server. Build from the repository root:
#   docker build -f party/docker/Dockerfile -t borealis-party .

FROM node:22-bookworm-slim AS web
WORKDIR /src
COPY ui/package.json ui/package-lock.json ui/
RUN npm --prefix ui ci --no-audit --no-fund
COPY ui ui
# The app shows the party version (ui/party/version.ts).
COPY party/Cargo.toml party/Cargo.toml
RUN node ui/party/build.mjs

FROM rust:1-bookworm AS server
WORKDIR /src
# rust-toolchain.toml asks for components a release build doesn't need.
ENV RUSTUP_TOOLCHAIN=stable
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY src-tauri src-tauri
COPY content content
COPY party party
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/party/target \
    cd party && cargo build --release --locked -p borealis-server \
    && cp target/release/borealis-server /borealis-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=server /borealis-server /app/borealis-server
COPY --from=web /src/ui/party/dist /app/web
COPY content /app/content
COPY LICENSE-MIT LICENSE-APACHE NOTICE /app/
COPY --chmod=0755 party/docker/entrypoint.sh /entrypoint.sh
ENV BOREALIS_CONFIG_DIR=/config \
    PUID=1000 \
    PGID=1000 \
    TZ=Etc/UTC
VOLUME /config
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD ["/app/borealis-server", "health"]
ENTRYPOINT ["/entrypoint.sh"]
