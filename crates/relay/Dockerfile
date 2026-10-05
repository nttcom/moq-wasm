FROM lukemathwalker/cargo-chef:0.1.77-rust-1.96.0-slim-bookworm@sha256:4a51277f4e3e8e4643dd6384f6f6b2b3c8de9f074299cd0c19a80f3c29e8dd15 AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG RELAY_BUILD_PROFILE=debug

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

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:b0ae8e989418b458e0f25489bc3be523718938a2b70864cc0f6a00af1ddbd985
ARG RELAY_BUILD_PROFILE=debug

WORKDIR /app
COPY --from=builder /workspace/target/${RELAY_BUILD_PROFILE}/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/crates/relay/keys /workspace/crates/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
