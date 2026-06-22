# ── Build stage ──────────────────────────────────────────────────────────────
# Images are pinned to SHA-256 digests to prevent supply-chain tag overwrites.
# To update: docker pull <image>, then docker inspect --format='{{index .RepoDigests 0}}' <image>
# Dependabot (.github/dependabot.yml) will keep digests current automatically.
FROM rust:1-slim@sha256:a818c23087b65be495e78fa329d577481e7700748bb2d1f28658b6bce3c7b931 AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

COPY Cargo.toml Cargo.lock ./
COPY aerini-engine ./aerini-engine
COPY aerini-server ./aerini-server

# Exclude src-tauri from the workspace — it has Tauri/desktop dependencies that
# cannot build in a headless container. aerini-server does not depend on it.
RUN printf '[workspace]\nmembers = ["aerini-engine", "aerini-server"]\nresolver = "2"\n' > Cargo.toml

RUN cargo build --release --target x86_64-unknown-linux-musl -p aerini-server

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
FROM busybox:1.36-musl@sha256:3c6ae8008e2c2eedd141725c30b20d9c36b026eb796688f88205845ef17aa213 AS busybox

FROM gcr.io/distroless/static-debian12:nonroot@sha256:d093aa3e30dbadd3efe1310db061a14da60299baff8450a17fe0ccc514a16639

COPY --from=busybox  /bin/wget                                                    /usr/local/bin/wget
COPY --from=builder  /app/target/x86_64-unknown-linux-musl/release/aerini-server  /usr/local/bin/aerini-server
COPY --from=builder  --chown=65532:65532 /data                                   /data

VOLUME ["/data"]

ENV AERINI_DATA_DIR=/data
ENV AERINI_PORT=7700

EXPOSE 7700

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/wget", "-qO-", "http://localhost:7700/api/health"]

ENTRYPOINT ["/usr/local/bin/aerini-server"]

# SECURITY: if AERINI_TOKEN is not set, a random token is generated on first
# run and printed to stdout. In Docker without an attached TTY, this token
# can be lost in log rotation before you retrieve it, leaving the server
# inaccessible. Always set AERINI_TOKEN explicitly:
#
#   docker run -e AERINI_TOKEN=your-secret-token aerini-server api
#   # or in docker-compose.yml: environment: - AERINI_TOKEN=${AERINI_TOKEN:?must be set}
#
# The docker-compose.yml in this repo enforces this via ${AERINI_TOKEN:?...}.
# If you use the image directly without compose, set the env var.
CMD ["api"]
