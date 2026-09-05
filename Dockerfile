# Heldar server — multi-stage container build (Cargo workspace).
# Build context is the repo root. Runtime bundles FFmpeg (recorder/clip/snapshot need it).
# Migrations are embedded into the binary at compile time (sqlx::migrate!), so the runtime image
# does not need the migrations directory.

# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for a reproducible build.
# To bump: pick the new tag, then resolve its index digest with
# `docker buildx imagetools inspect rust:<tag>` (see docs/SUPPLY-CHAIN.md).
#
# This tracks the toolchain the build actually needs, NOT the workspace MSRV. It was pinned to 1.85.1
# to match MSRV, and the v0.4.0 image build then failed: transitive deps (icu_* 2.2) require rustc
# 1.86. CI builds with `stable`, so the two disagreed and nothing noticed until a tag ran the image
# build — the one job that uses this base. Keep it at or above what the locked dependency tree
# demands; MSRV is a promise to LIBRARY consumers (declared per-crate via `rust-version`) and is
# checked by `cargo` at build time, not by this pin.
# Scanned by Trivy (HIGH/CRITICAL, fixed only) BEFORE anything is published — see
# docs/SUPPLY-CHAIN.md. A base bump that drags in a fixable HIGH fails the build rather than
# shipping. Bump the pinned digest to fix; where there is no fix yet, record it in
# security/dependency-exceptions.json with an owner and an expiry.
FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS builder
# Optional cargo features to compile in (space-separated), e.g. FEATURES="smtp". Empty = default build.
ARG FEATURES=""
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --bin heldar-core ${FEATURES:+--features "$FEATURES"}

# Pinned runtime base: bookworm (Debian 12) codename + multi-arch index @sha256 digest.
# To bump: `docker buildx imagetools inspect debian:bookworm-slim` and update the digest.
FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
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
