FROM rust:1.88-bookworm AS builder

WORKDIR /workspace
COPY . .
RUN cargo build -p relay

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /workspace/target/debug/relay /usr/local/bin/relay

EXPOSE 443/udp 444/udp
CMD ["relay"]
