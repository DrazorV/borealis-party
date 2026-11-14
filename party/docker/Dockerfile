# syntax=docker/dockerfile:1.7
# Borealis Party server. Build from the repository root:
#   docker build -f party/docker/Dockerfile -t borealis-party .

# The PostgreSQL client major version for the automatic backup (see the last stages).
ARG PG_CLIENT_MAJOR=18

FROM node:22-bookworm-slim AS web
WORKDIR /src
COPY ui/package.json ui/package-lock.json ui/
RUN npm --prefix ui ci --no-audit --no-fund
COPY ui ui
# The app shows the party version (ui/party/version.ts).
COPY party/Cargo.toml party/VERSION party/
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

# The automatic backup before an upgrade runs pg_dump and pg_restore (backup.rs). Their major version must be at least
# the PostgreSQL server's, so they come from the PostgreSQL project's own repository. The final image takes only those
# two programs and libpq: the full client package pulls in Perl for Debian's wrapper scripts, which aren't used.

FROM debian:bookworm-slim AS pgdg
ADD https://www.postgresql.org/media/keys/ACCC4CF8.asc /usr/share/keyrings/pgdg.asc
RUN chmod 0644 /usr/share/keyrings/pgdg.asc \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && echo "deb [signed-by=/usr/share/keyrings/pgdg.asc] https://apt.postgresql.org/pub/repos/apt bookworm-pgdg main" \
        > /etc/apt/sources.list.d/pgdg.list \
    && rm -rf /var/lib/apt/lists/*

FROM pgdg AS pgclient
ARG PG_CLIENT_MAJOR
RUN apt-get update \
    && apt-get install -y --no-install-recommends "postgresql-client-${PG_CLIENT_MAJOR}" \
    && rm -rf /var/lib/apt/lists/*

FROM pgdg
ARG PG_CLIENT_MAJOR
RUN apt-get update \
    && apt-get install -y --no-install-recommends tzdata libpq5 liblz4-1 libzstd1 zlib1g \
    && rm -rf /var/lib/apt/lists/*
COPY --from=pgclient /usr/lib/postgresql/${PG_CLIENT_MAJOR}/bin/pg_dump /usr/lib/postgresql/${PG_CLIENT_MAJOR}/bin/pg_restore \
    /usr/lib/postgresql/${PG_CLIENT_MAJOR}/bin/
COPY --from=server /borealis-server /app/borealis-server
COPY --from=web /src/ui/party/dist /app/web
COPY content /app/content
COPY LICENSE-MIT LICENSE-APACHE NOTICE /app/
COPY --chmod=0755 party/docker/entrypoint.sh /entrypoint.sh
ENV BOREALIS_CONFIG_DIR=/config \
    BOREALIS__BACKUP__POSTGRES_BIN=/usr/lib/postgresql/${PG_CLIENT_MAJOR}/bin \
    PUID=1000 \
    PGID=1000 \
    TZ=Etc/UTC
VOLUME /config
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD ["/app/borealis-server", "health"]
ENTRYPOINT ["/entrypoint.sh"]
