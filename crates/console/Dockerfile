FROM node:22.23.4-bookworm-slim@sha256:5991875dfb80938dd8a6ba3fe3e96fd04929e70792762b3319d8caadbbe229bc AS web
WORKDIR /web
COPY crates/console/web/package.json crates/console/web/package-lock.json ./
RUN npm ci
COPY crates/console/web/ ./
RUN npm run build

FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.0-slim-trixie@sha256:bcda2cef02bedabfaf433379d82fecd02c0a428fb66fe28ab2d65c25c3db3b5e AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release -p console --recipe-path recipe.json
COPY . .
RUN cargo build --release -p console

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf

COPY --from=builder /workspace/target/release/console /usr/local/bin/
COPY --from=web /web/dist /srv/console
ENV CONSOLE_STATIC_DIR=/srv/console

EXPOSE 8080
USER nonroot:nonroot
CMD ["/usr/local/bin/console"]
