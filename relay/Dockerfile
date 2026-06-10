FROM rust:1.96.0-slim-bookworm AS builder

WORKDIR /workspace
COPY . .
RUN cargo build -p relay --release

FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app
COPY --from=builder /workspace/target/release/relay /usr/local/bin/relay

EXPOSE 443/udp 444/udp
USER nonroot:nonroot
CMD ["/usr/local/bin/relay"]
