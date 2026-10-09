FROM lukemathwalker/cargo-chef:0.1.77-rust-1.96.0-slim-bookworm@sha256:4a51277f4e3e8e4643dd6384f6f6b2b3c8de9f074299cd0c19a80f3c29e8dd15 AS chef
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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

WORKDIR /app
COPY --from=builder /workspace/target/release/vts /workspace/target/release/vts-mint /usr/local/bin/

EXPOSE 8081
USER nonroot:nonroot
CMD ["/usr/local/bin/vts"]
