FROM node:22.12.0-bookworm-slim@sha256:35531c52ce27b6575d69755c73e65d4468dba93a25644eed56dc12879cae9213 AS web
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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

COPY --from=builder /workspace/target/release/console /usr/local/bin/
COPY --from=web /web/dist /srv/console
ENV CONSOLE_STATIC_DIR=/srv/console

EXPOSE 8080
USER nonroot:nonroot
CMD ["/usr/local/bin/console"]
