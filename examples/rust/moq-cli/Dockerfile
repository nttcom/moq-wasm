FROM rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6 AS builder
WORKDIR /workspace
COPY . .
RUN cargo build --locked --release -p moq-cli

# ffmpeg sits next to the CLI so a container can pipe a stream into `moq-cli publish`.
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/moq-cli /usr/local/bin/
