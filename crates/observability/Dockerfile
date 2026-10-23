FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-slim-trixie@sha256:f122e3c240d1c05668b07179cf6fa07647db2a79a110f4bb8a91d75f90d5b8e4 AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Dependencies are cooked from the recipe alone so this layer is reused
# until a manifest or Cargo.lock changes; only the workspace crates are
# compiled after the source is copied in.
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release -p observability --recipe-path recipe.json
COPY . .
RUN cargo build --release -p observability

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf

WORKDIR /app
COPY --from=builder /workspace/target/release/observability /usr/local/bin/

EXPOSE 8095
USER nonroot:nonroot
CMD ["/usr/local/bin/observability"]
