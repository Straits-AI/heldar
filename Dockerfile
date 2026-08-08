# Heldar server — multi-stage container build (Cargo workspace).
# Build context is the repo root. Runtime bundles FFmpeg (recorder/clip/snapshot need it).
# Migrations are embedded into the binary at compile time (sqlx::migrate!), so the runtime image
# does not need the migrations directory.

# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for a reproducible build.
# MSRV is 1.85 (see crates/*/Cargo.toml rust-version). To bump: pick the new tag, then resolve its
# index digest with `docker buildx imagetools inspect rust:<tag>` (see docs/SUPPLY-CHAIN.md).
FROM rust:1.97.1-bookworm@sha256:14bc9c5966e7b3a385794b3d5389a8765668342025fbcc7b2e3d2866ac4bd8c3 AS builder
# Optional cargo features to compile in (space-separated), e.g. FEATURES="smtp". Empty = default build.
ARG FEATURES=""
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --bin heldar-core ${FEATURES:+--features "$FEATURES"}

# Pinned runtime base: bookworm (Debian 12) codename + multi-arch index @sha256 digest.
# To bump: `docker buildx imagetools inspect debian:bookworm-slim` and update the digest.
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
ARG FEATURES=""
# ffmpeg: recorder/clip/snapshot/sampler. curl: container HEALTHCHECK. ca-certificates: outbound TLS.
# tzdata is required for `chrono::Local` (recording-schedule windows are evaluated in the server's
# local timezone). Without it, and with TZ unset, Local silently resolves to UTC, so "HH:MM local"
# schedules record at the wrong wall-clock time. Set `-e TZ=Area/City` at run time to your timezone.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl tzdata \
    && rm -rf /var/lib/apt/lists/*
# Run as a non-root user (container hardening / Pod Security). Fixed UID so a bind-mounted /data can
# be chowned to it by the operator; a named volume is initialized with these perms automatically.
RUN groupadd -r -g 10001 heldar && useradd -r -u 10001 -g heldar heldar
WORKDIR /app
COPY --from=builder /app/target/release/heldar-core /usr/local/bin/heldar-core
ENV HELDAR_DATA_DIR=/data
RUN mkdir -p /data && chown -R heldar:heldar /data /app
USER heldar
EXPOSE 8000
# Readiness probe: /readyz returns 503 until the database is reachable (vs /healthz = liveness only),
# so orchestrators don't route traffic before the service can serve it.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8000/readyz || exit 1
ENTRYPOINT ["heldar-core"]
