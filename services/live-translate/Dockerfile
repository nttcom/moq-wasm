FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-slim-trixie@sha256:f122e3c240d1c05668b07179cf6fa07647db2a79a110f4bb8a91d75f90d5b8e4 AS chef
WORKDIR /workspace
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
 && rm -rf /var/lib/apt/lists/*

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release -p live-translate --recipe-path recipe.json
COPY . .
RUN cargo build --release -p live-translate

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libgstreamer1.0-0 gstreamer1.0-plugins-base \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 65532 nonroot
COPY --from=builder /workspace/target/release/live-translate /usr/local/bin/
USER nonroot
CMD ["/usr/local/bin/live-translate"]
