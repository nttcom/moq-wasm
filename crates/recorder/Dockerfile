FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.0-slim-trixie@sha256:bcda2cef02bedabfaf433379d82fecd02c0a428fb66fe28ab2d65c25c3db3b5e AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Dependencies are cooked from the recipe alone so this layer is reused
# until a manifest or Cargo.lock changes; only the workspace crates are
# compiled after the source is copied in.
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release -p recorder --recipe-path recipe.json
COPY . .
RUN cargo build --release -p recorder

# Compositing runs GStreamer, which distroless does not carry.
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
    ca-certificates libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly gstreamer1.0-libav \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 65532 --no-create-home nonroot \
    && mkdir /recordings && chown nonroot:nonroot /recordings

# The user has no home for GStreamer to cache its plugin registry in.
ENV GST_REGISTRY=/tmp/gstreamer-registry.bin
WORKDIR /app
COPY --from=builder /workspace/target/release/recorder /usr/local/bin/

EXPOSE 8096
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/recorder"]
CMD ["serve", "--listen", "0.0.0.0:8096", "--output-dir", "/recordings"]
