# Heldar server — multi-stage container build (Cargo workspace).
# Build context is the repo root. Runtime bundles FFmpeg (recorder/clip/snapshot need it).
# Migrations are embedded into the binary at compile time (sqlx::migrate!), so the runtime image
# does not need the migrations directory.

FROM rust:1-bookworm AS builder
# Optional cargo features to compile in (space-separated), e.g. FEATURES="wireguard". Empty = default build.
ARG FEATURES=""
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --bin heldar-core ${FEATURES:+--features "$FEATURES"}

FROM debian:bookworm-slim
ARG FEATURES=""
# ffmpeg: recorder/clip/snapshot/sampler. curl: container HEALTHCHECK. ca-certificates: outbound TLS.
# With the `wireguard` feature the kernel-managed remote access shells out to `ip` (iproute2) and `wg`
# (wireguard-tools), and the binary needs CAP_NET_ADMIN baked as a file capability (libcap2-bin → setcap)
# so the non-root container user can manage its OWN interface (runtime also needs cap_add: NET_ADMIN).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
    && if echo "$FEATURES" | grep -qw wireguard; then \
         apt-get install -y --no-install-recommends iproute2 wireguard-tools libcap2-bin; \
       fi \
    && rm -rf /var/lib/apt/lists/*
# Run as a non-root user (container hardening / Pod Security). Fixed UID so a bind-mounted /data can
# be chowned to it by the operator; a named volume is initialized with these perms automatically.
RUN groupadd -r -g 10001 heldar && useradd -r -u 10001 -g heldar heldar
WORKDIR /app
COPY --from=builder /app/target/release/heldar-core /usr/local/bin/heldar-core
# Bake the network capability (build-time, no host sudo) so the non-root user can bring up its own
# WireGuard interface at runtime. Inert unless the `wireguard` feature + HELDAR_WG_MANAGED.
# Capability the binary itself, AND the `ip`/`wg` helpers it execs: with a non-root container user,
# caps from `cap_add` sit in the bounding set but NOT the inheritable set (runc dropped inheritable
# caps in 2022), so they don't pass through exec via ambient. File caps on ip/wg make them elevate
# directly within the bounding set — the robust, stay-non-root fix. (Runtime still needs cap_add: NET_ADMIN.)
# readlink -f resolves symlinks (e.g. /usr/sbin/ip -> /usr/bin/ip) — setcap on a symlink silently no-ops.
RUN if echo "$FEATURES" | grep -qw wireguard; then \
      setcap cap_net_admin,cap_net_raw+eip /usr/local/bin/heldar-core; \
      setcap cap_net_admin,cap_net_raw+ep "$(readlink -f "$(command -v ip)")"; \
      setcap cap_net_admin,cap_net_raw+ep "$(readlink -f "$(command -v wg)")"; \
    fi
ENV HELDAR_DATA_DIR=/data
RUN mkdir -p /data && chown -R heldar:heldar /data /app
USER heldar
EXPOSE 8000
# Readiness probe: /readyz returns 503 until the database is reachable (vs /healthz = liveness only),
# so orchestrators don't route traffic before the service can serve it.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8000/readyz || exit 1
ENTRYPOINT ["heldar-core"]
