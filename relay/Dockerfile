FROM rust:1.96.0-slim-bookworm@sha256:c8a94a78f67ec8c4d474ec7f71e0720f21eb7e584e158daec0874cafa7c30e4d AS builder

WORKDIR /workspace
COPY . .
RUN cargo build -p relay && mkdir -p /workspace/relay/keys

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:b0ae8e989418b458e0f25489bc3be523718938a2b70864cc0f6a00af1ddbd985

WORKDIR /app
COPY --from=builder /workspace/target/debug/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/relay/keys /workspace/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
