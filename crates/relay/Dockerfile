ARG RELAY_BUILD_PROFILE=debug
# source:   compile the relay inside the image (docker compose --profile build).
# prebuilt: take target/<profile>/relay from a build context holding only that
#           binary and crates/relay/keys (CI builds it on the runner with cargo).
ARG RELAY_BINARY=source

FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-slim-trixie@sha256:f122e3c240d1c05668b07179cf6fa07647db2a79a110f4bb8a91d75f90d5b8e4 AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS source
ARG RELAY_BUILD_PROFILE

# Dependencies are cooked from the recipe alone so this layer is reused
# until a manifest or Cargo.lock changes; only the workspace crates are
# compiled after the source is copied in.
COPY --from=planner /workspace/recipe.json recipe.json
RUN case "$RELAY_BUILD_PROFILE" in \
      debug) cargo chef cook -p relay --recipe-path recipe.json ;; \
      release) cargo chef cook --release -p relay --recipe-path recipe.json ;; \
      *) echo "unsupported RELAY_BUILD_PROFILE: $RELAY_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

COPY . .
RUN case "$RELAY_BUILD_PROFILE" in \
      debug) cargo build -p relay ;; \
      release) cargo build --release -p relay ;; \
      *) echo "unsupported RELAY_BUILD_PROFILE: $RELAY_BUILD_PROFILE" >&2; exit 1 ;; \
    esac && mkdir -p /workspace/crates/relay/keys

FROM scratch AS prebuilt
ARG RELAY_BUILD_PROFILE
COPY relay /workspace/target/${RELAY_BUILD_PROFILE}/relay
COPY keys /workspace/crates/relay/keys

FROM ${RELAY_BINARY} AS builder

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf
ARG RELAY_BUILD_PROFILE

WORKDIR /app
COPY --from=builder /workspace/target/${RELAY_BUILD_PROFILE}/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/crates/relay/keys /workspace/crates/relay/keys
ENV RELAY_CERT_DIR=/workspace/crates/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
