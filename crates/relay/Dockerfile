ARG RELAY_BUILD_PROFILE=debug
# source:   compile the relay inside the image (docker compose --profile build).
# prebuilt: take target/<profile>/relay from a build context holding only that
#           binary and crates/relay/keys (CI builds it on the runner with cargo).
ARG RELAY_BINARY=source

FROM lukemathwalker/cargo-chef:0.1.77-rust-1.96.0-slim-trixie@sha256:43ee9436c9b0c5c6631931a0f891233bb819261b02dab12646709d8bd8d74bdd AS chef
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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
ARG RELAY_BUILD_PROFILE

WORKDIR /app
COPY --from=builder /workspace/target/${RELAY_BUILD_PROFILE}/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/crates/relay/keys /workspace/crates/relay/keys
ENV RELAY_CERT_DIR=/workspace/crates/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
