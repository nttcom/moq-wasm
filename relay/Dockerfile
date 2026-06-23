FROM rust:1.96.0-slim-bookworm@sha256:c8a94a78f67ec8c4d474ec7f71e0720f21eb7e584e158daec0874cafa7c30e4d AS builder
ARG RELAY_BUILD_PROFILE=debug

WORKDIR /workspace
COPY . .
RUN case "$RELAY_BUILD_PROFILE" in \
      debug) cargo build -p relay ;; \
      release) cargo build --release -p relay ;; \
      *) echo "unsupported RELAY_BUILD_PROFILE: $RELAY_BUILD_PROFILE" >&2; exit 1 ;; \
    esac && mkdir -p /workspace/relay/keys

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:b0ae8e989418b458e0f25489bc3be523718938a2b70864cc0f6a00af1ddbd985
ARG RELAY_BUILD_PROFILE=debug

WORKDIR /app
COPY --from=builder /workspace/target/${RELAY_BUILD_PROFILE}/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/relay/keys /workspace/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
