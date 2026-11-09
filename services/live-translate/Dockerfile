FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-slim-trixie@sha256:f122e3c240d1c05668b07179cf6fa07647db2a79a110f4bb8a91d75f90d5b8e4 AS chef
WORKDIR /workspace
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
 && rm -rf /var/lib/apt/lists/*

# The browser SDK's wasm, which the pages bundle.
FROM chef AS wasm
ARG WASM_PACK_VERSION=v0.15.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && curl -fsSL "https://github.com/wasm-bindgen/wasm-pack/releases/download/${WASM_PACK_VERSION}/wasm-pack-${WASM_PACK_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz --strip-components=1 -C /usr/local/bin "wasm-pack-${WASM_PACK_VERSION}-x86_64-unknown-linux-musl/wasm-pack"
COPY . .
RUN cd crates/wasm/js && wasm-pack build ../ --target web --out-name moqt --out-dir=js/pkg

# The Streamer and Viewer pages, built at the root the service serves them from.
FROM node:22.23.4-bookworm-slim@sha256:5991875dfb80938dd8a6ba3fe3e96fd04929e70792762b3319d8caadbbe229bc AS web
WORKDIR /workspace
COPY package.json package-lock.json ./
COPY crates/wasm/js/package.json crates/wasm/js/
COPY examples/browser/package.json examples/browser/
RUN npm ci
COPY crates/wasm/js crates/wasm/js
COPY --from=wasm /workspace/crates/wasm/js/pkg crates/wasm/js/pkg
COPY examples/browser examples/browser
RUN cd examples/browser && npx vite build --base=/

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
COPY --from=web /workspace/examples/browser/dist/examples/live-translate /srv/live-translate/examples/live-translate
COPY --from=web /workspace/examples/browser/dist/assets /srv/live-translate/assets
ENV LIVE_TRANSLATE_STATIC_DIR=/srv/live-translate
EXPOSE 8080
USER nonroot
CMD ["/usr/local/bin/live-translate"]
