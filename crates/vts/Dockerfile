FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.0-slim-trixie@sha256:bcda2cef02bedabfaf433379d82fecd02c0a428fb66fe28ab2d65c25c3db3b5e AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Dependencies are cooked from the recipe alone so this layer is reused
# until a manifest or Cargo.lock changes; only the workspace crates are
# compiled after the source is copied in.
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release -p vts --recipe-path recipe.json
COPY . .
RUN cargo build --release -p vts

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf

WORKDIR /app
COPY --from=builder /workspace/target/release/vts /workspace/target/release/vts-mint /usr/local/bin/

EXPOSE 8081
USER nonroot:nonroot
CMD ["/usr/local/bin/vts"]
