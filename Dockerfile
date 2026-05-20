# Build stage
FROM rust:1-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

# Copy workspace manifests and lock file
COPY Cargo.toml Cargo.lock ./
COPY flowo-engine ./flowo-engine
COPY flowo-server ./flowo-server

# Rewrite the workspace Cargo.toml to exclude src-tauri.
# The workspace member src-tauri has Tauri/desktop dependencies that cannot
# build in a headless Linux container. Since flowo-server does not depend on
# src-tauri, excluding it from the workspace keeps the dependency graph clean
# and the build fast.
RUN printf '[workspace]\nmembers = ["flowo-engine", "flowo-server"]\nresolver = "2"\n' > Cargo.toml

RUN cargo build --release --target x86_64-unknown-linux-musl -p flowo-server

# Runtime stage — bookworm-slim works because the binary is fully static (musl)
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/flowo-server /usr/local/bin/flowo-server

RUN useradd -r -s /bin/false flowo && \
    mkdir -p /data && \
    chown flowo:flowo /data

USER flowo

VOLUME ["/data"]

ENV FLOWO_DATA_DIR=/data
ENV FLOWO_PORT=7700

EXPOSE 7700

ENTRYPOINT ["flowo-server"]
CMD ["api"]
