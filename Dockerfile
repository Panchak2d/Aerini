# ── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

COPY Cargo.toml Cargo.lock ./
COPY flowo-engine ./flowo-engine
COPY flowo-server ./flowo-server

# Exclude src-tauri from the workspace — it has Tauri/desktop dependencies that
# cannot build in a headless container. flowo-server does not depend on it.
RUN printf '[workspace]\nmembers = ["flowo-engine", "flowo-server"]\nresolver = "2"\n' > Cargo.toml

RUN cargo build --release --target x86_64-unknown-linux-musl -p flowo-server

# Pre-create the data directory with nonroot ownership (uid/gid 65532).
# distroless/static has no shell or useradd, so ownership must be set here in
# the builder and carried over via --chown in the COPY instruction below.
RUN mkdir -p /data && chown 65532:65532 /data

# ── Runtime stage ─────────────────────────────────────────────────────────────
# distroless/static is the correct pairing for a statically linked MUSL binary:
# no glibc, no shell, no package manager. Minimal attack surface.
# The nonroot variant runs as uid 65532 automatically — no useradd needed.
#
# HEALTHCHECK needs a wget binary. distroless/static has none, so we copy the
# statically compiled wget from busybox:musl. It is ~1 MB and adds no runtime
# attack surface because it is only invoked by the Docker daemon's health prober,
# not by the container process itself.
FROM busybox:1.36-musl AS busybox

FROM gcr.io/distroless/static-debian12:nonroot

COPY --from=busybox  /bin/wget                                                    /usr/local/bin/wget
COPY --from=builder  /app/target/x86_64-unknown-linux-musl/release/flowo-server  /usr/local/bin/flowo-server
COPY --from=builder  --chown=65532:65532 /data                                   /data

VOLUME ["/data"]

ENV FLOWO_DATA_DIR=/data
ENV FLOWO_PORT=7700

EXPOSE 7700

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/wget", "-qO-", "http://localhost:7700/api/health"]

ENTRYPOINT ["/usr/local/bin/flowo-server"]

# SECURITY: if FLOWO_TOKEN is not set, a random token is generated on first
# run and printed to stdout. In Docker without an attached TTY, this token
# can be lost in log rotation before you retrieve it, leaving the server
# inaccessible. Always set FLOWO_TOKEN explicitly:
#
#   docker run -e FLOWO_TOKEN=your-secret-token flowo-server api
#   # or in docker-compose.yml: environment: - FLOWO_TOKEN=${FLOWO_TOKEN:?must be set}
#
# The docker-compose.yml in this repo enforces this via ${FLOWO_TOKEN:?...}.
# If you use the image directly without compose, set the env var.
CMD ["api"]
