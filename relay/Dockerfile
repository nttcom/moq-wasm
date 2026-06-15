FROM rust:1.96.0-slim-bookworm AS builder

WORKDIR /workspace
COPY . .
RUN cargo build -p relay && mkdir -p /workspace/relay/keys

FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app
COPY --from=builder /workspace/target/debug/relay /usr/local/bin/relay
COPY --from=builder --chown=nonroot:nonroot /workspace/relay/keys /workspace/relay/keys

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
