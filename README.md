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

### Local cleanup commands

```shell
make lint
make format
```

CI runs the full validation suite, including the Linux-only browser media E2E flow.
