# The browser examples built as deploy.yml builds them for GitHub Pages, under /moq-wasm/.
FROM rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6 AS wasm
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl; sha256=c09f971ecaed9a2efc80fdcea7a00ef6b53c7fadc8c57d1f61b53a6aa66b668a ;; \
      arm64) target=aarch64-unknown-linux-musl; sha256=e17ef0806381c3a0acb9c9ddad643a49facaa5a2ecf657a421d4d8f3357a24b7 ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSL -o /tmp/wasm-pack.tar.gz \
      "https://github.com/wasm-bindgen/wasm-pack/releases/download/v0.15.0/wasm-pack-v0.15.0-${target}.tar.gz" \
    && echo "${sha256}  /tmp/wasm-pack.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/wasm-pack.tar.gz --strip-components=1 -C /usr/local/bin "wasm-pack-v0.15.0-${target}/wasm-pack" \
    && rm /tmp/wasm-pack.tar.gz \
    && rustup target add wasm32-unknown-unknown
WORKDIR /workspace
COPY . .
RUN wasm-pack build crates/wasm --target web --out-name moqt --out-dir=js/pkg

FROM node:22.23.4-trixie-slim@sha256:be1e2a3dc2c4fd68cdfadad2a696a280e0d2749098bee509230174d72fb768c6 AS web
WORKDIR /workspace
COPY . .
RUN npm ci
COPY --from=wasm /workspace/crates/wasm/js/pkg crates/wasm/js/pkg
RUN cd examples/browser && npx vite build

FROM nginx:1.29-alpine@sha256:5616878291a2eed594aee8db4dade5878cf7edcb475e59193904b198d9b830de
COPY --from=web /workspace/examples/browser/dist /usr/share/nginx/html/moq-wasm
