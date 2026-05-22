# MoQT

Rust workspace for MoQT draft-14.

## Directories

- `moqt/`: MoQT core implementation
- `bindings/`: bindings for non-Rust runtimes
- `relay/`: QUIC + WebTransport relay
- `bridges/`: ONVIF and live ingest bridges
- `shared/`: shared data structures
- `examples/`: browser and interop examples
- `spec/`: protocol specifications

## Setup

Enter the Nix development shell:

```shell
nix develop
```

Install browser dependencies:

```shell
npm --prefix examples/browser ci
```

## Run

Run these commands inside the Nix development shell:

```shell
make relay
make browser
make chrome
make live-ingest

# For Linux users
make chrome:linux
```

`make browser` builds the browser WASM bindings before starting Vite.

## Test

Run Rust tests:

```shell
make test
make browser-e2e-media
```

`make browser-e2e-media` installs the browser E2E prerequisites, starts the local
relay and Vite server, waits until both are ready, runs the Playwright media E2E
test, and then cleans up the child processes. The automated media E2E flow is
supported on Linux and macOS.

## Validation

Run linters and formatters:

```shell
make lint
make format
```
