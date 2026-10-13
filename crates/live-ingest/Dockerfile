FROM rust:1.99.0-slim-trixie@sha256:2752b332db73fdbb7dc576f06c82ed1f312005784ef913d7e04a28f5f55dc581 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .
RUN cargo build --locked --release -p live-ingest

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
    ca-certificates libssl3t64 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /workspace/target/release/live-ingest /usr/local/bin/
EXPOSE 1935/tcp 9000/udp
ENTRYPOINT ["/usr/local/bin/live-ingest"]
