# MoQT

MoQT draft-14 の Rust workspace です。

## Directories

- `moqt/`: MoQT core implementation
- `bindings/`: bindings for non-Rust runtimes
- `relay/`: QUIC + WebTransport relay
- `bridges/`: ONVIF and live ingest bridges
- `shared/`: shared data structures
- `examples/`: browser and interop examples
- `spec/`: protocol specifications

## Setup

```shell
cargo check --workspace
npm --prefix examples/browser install
npm --prefix examples/browser run wasm
```

## Run

```shell
make relay
make browser
make chrome
make live-ingest
make onvif
```

## Check

```shell
cargo check --workspace
cargo fmt --check
npm --prefix examples/browser run lint
npx --prefix examples/browser prettier --check "examples/browser/**/*.{js,jsx,ts,tsx,json,css,md}" --ignore-path examples/browser/.prettierignore
```
