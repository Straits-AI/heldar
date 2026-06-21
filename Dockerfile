# Heldar server — multi-stage container build (Cargo workspace).
# Build context is the repo root. Runtime bundles FFmpeg (recorder/clip/snapshot need it).
# Migrations are embedded into the binary at compile time (sqlx::migrate!), so the runtime image
# does not need the migrations directory.

FROM rust:1-bookworm AS builder
# Optional cargo features to compile in (space-separated), e.g. FEATURES="smtp". Empty = default build.
ARG FEATURES=""
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --bin heldar-core ${FEATURES:+--features "$FEATURES"}

FROM debian:bookworm-slim
ARG FEATURES=""
# ffmpeg: recorder/clip/snapshot/sampler. curl: container HEALTHCHECK. ca-certificates: outbound TLS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
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
