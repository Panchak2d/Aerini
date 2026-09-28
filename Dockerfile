# ── Build stage ──────────────────────────────────────────────────────────────
# Images are pinned to SHA-256 digests to prevent supply-chain tag overwrites.
# To update: docker pull <image>, then docker inspect --format='{{index .RepoDigests 0}}' <image>
# Dependabot (.github/dependabot.yml) will keep digests current automatically.
FROM rust:1-slim@sha256:31ee7fc65186be7e0e0ccb3f2ca305f14e4739e7642a1ae65753aa5d7b874523 AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y musl-tools curl ca-certificates xz-utils binutils && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

# Node.js for the Code node's server-side sandbox. Version is read from the
# repo-root NODE_VERSION file below, shared with scripts/fetch-node-binaries.sh
# (desktop) so both fetch from one source instead of two separate pins.
# Official nodejs.org Linux builds are glibc/libstdc++ dynamically linked, not
# static — this is why the runtime stage below is cc-debian12, not static-debian12.
# The official build embeds a full native debug symbol table; strip removes it
# (native debug metadata only, never touches V8/JS execution) — binutils is
# installed above for this. The post-strip --version check confirms the binary
# still runs before it's allowed into the image, the same check
# scripts/fetch-node-binaries.sh runs for the desktop targets.
COPY NODE_VERSION ./NODE_VERSION
RUN NODE_VERSION="$(head -n1 NODE_VERSION | tr -d '\r')" && \
    if ! echo "$NODE_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then \
      echo "ERROR: NODE_VERSION file must have a bare semver as its first line, got: '$NODE_VERSION'" >&2; \
      exit 1; \
    fi && \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" -o /tmp/node-shasums.txt && \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" -o /tmp/node.tar.xz && \
    NODE_EXPECTED_SHA256="$(awk -v f="node-v${NODE_VERSION}-linux-x64.tar.xz" '$2==f {print $1; exit}' /tmp/node-shasums.txt)" && \
    [ -n "$NODE_EXPECTED_SHA256" ] && \
    echo "${NODE_EXPECTED_SHA256}  /tmp/node.tar.xz" | sha256sum -c - && \
    tar -xJf /tmp/node.tar.xz -C /tmp && \
    cp "/tmp/node-v${NODE_VERSION}-linux-x64/bin/node" /app/node-bundled && \
    chmod +x /app/node-bundled && \
    strip /app/node-bundled && \
    [ "$(/app/node-bundled --version)" = "v${NODE_VERSION}" ] && \
    rm -rf /tmp/node.tar.xz /tmp/node-shasums.txt "/tmp/node-v${NODE_VERSION}-linux-x64"

COPY Cargo.toml Cargo.lock ./
COPY aerini-engine ./aerini-engine
COPY aerini-server ./aerini-server

# Drop src-tauri from the workspace members — it has Tauri/desktop dependencies
# that cannot build in a headless container, and aerini-server does not depend
# on it. Editing the real root manifest (instead of writing a replacement)
# keeps [workspace.package] fields such as version and license inherited by
# the member crates. Dropping the member prunes Cargo.lock, so the build below
# must not pass --locked.
RUN sed -i '/^[[:space:]]*"src-tauri",[[:space:]]*$/d' Cargo.toml && \
    if grep -q 'src-tauri' Cargo.toml; then \
      echo "ERROR: src-tauri is still referenced in Cargo.toml after the sed edit; update the pattern in the Dockerfile" >&2; \
      exit 1; \
    fi

RUN cargo build --release --target x86_64-unknown-linux-musl -p aerini-server

# Pre-create the data directory with nonroot ownership (uid/gid 65532).
# distroless/static has no shell or useradd, so ownership must be set here in
# the builder and carried over via --chown in the COPY instruction below.
RUN mkdir -p /data && chown 65532:65532 /data

# ── Runtime stage ─────────────────────────────────────────────────────────────
# aerini-server itself is a statically linked MUSL binary and would be happy on
# distroless/static, but the bundled Node.js binary below is not — official
# nodejs.org Linux builds are dynamically linked against glibc and libstdc++.
# cc-debian12 is distroless/static plus exactly those two libraries (it exists
# specifically for C++-linked runtimes), still no shell, no package manager.
# The nonroot variant runs as uid 65532 automatically — no useradd needed.
#
# HEALTHCHECK needs a wget binary. This base has none, so we copy the
# statically compiled wget from busybox:musl. It is ~1 MB and adds no runtime
# attack surface because it is only invoked by the Docker daemon's health prober,
# not by the container process itself.
FROM busybox:1.38-musl@sha256:8635836765b0c4c43970660219739baa58b0883c2e429e4b8918f7dd1519455c AS busybox

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

COPY --from=busybox  /bin/wget                                                    /usr/local/bin/wget
COPY --from=builder  /app/target/x86_64-unknown-linux-musl/release/aerini-server  /usr/local/bin/aerini-server
COPY --from=builder  /app/node-bundled                                            /usr/local/bin/node-bundled
COPY --from=builder  --chown=65532:65532 /data                                   /data

VOLUME ["/data"]

ENV AERINI_DATA_DIR=/data
ENV AERINI_PORT=7700
# 0.0.0.0 here is the container's *internal* listen address, not a host-facing
# exposure setting — Docker only forwards traffic that an explicit `-p`/`ports:`
# mapping publishes, and a process bound to 127.0.0.1 inside the container never
# sees packets arriving over the container's virtual network interface at all.
# The actual access boundary is the host-side mapping (docker-compose.yml pins
# it to 127.0.0.1:7700, i.e. host-loopback-only); don't "fix" this back to
# 127.0.0.1 to restrict exposure — that belongs in the `-p`/`ports:` mapping.
ENV AERINI_BIND=0.0.0.0

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
