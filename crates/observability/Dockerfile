FROM lukemathwalker/cargo-chef:0.1.77-rust-1.96.0-slim-trixie@sha256:43ee9436c9b0c5c6631931a0f891233bb819261b02dab12646709d8bd8d74bdd AS chef
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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

WORKDIR /app
COPY --from=builder /workspace/target/release/observability /usr/local/bin/

EXPOSE 8095
USER nonroot:nonroot
CMD ["/usr/local/bin/observability"]
