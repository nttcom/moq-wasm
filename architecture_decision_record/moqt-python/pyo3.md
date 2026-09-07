# ADR: pyo3 for the Python bindings

## Status
Accepted

## Date
2026-09-07

## Context
`bindings/python` exposes the `moqt` crate to Python so that applications such
as media pipelines can publish and subscribe MoQT tracks from asyncio code.
The binding must call into a tokio-based Rust library, hand `bytes` payloads
across the boundary in both directions, and be installable as a wheel.

## Decision
Use **pyo3** (`abi3-py310`, built with **maturin**) and write the binding as a
native extension module. `pyo3-build-config` is used from `build.rs` so that
`cargo build` also links the module on macOS; maturin sets the same flags for
wheel builds.

## Consequences

### Positive
- `#[pyclass]` wrappers hold `Arc<moqt::Session<DUAL>>` and friends directly;
  no C ABI layer to maintain.
- `abi3` produces one wheel per platform for all supported Python versions.
- Errors map to Python exceptions through the `anyhow` feature.

### Negative
- `#[pyclass]` types cannot be generic, so the binding is pinned to one
  transport marker (`DUAL`), which is why `DUAL` gained client mode.
- Building the workspace now requires a Python interpreter for
  `pyo3-build-config`; the crate disables its cargo test harness
  (`test = false`) because extension modules cannot link as test binaries.

### Neutral
- Unit tests for the binding are Python-side (`pytest`) against an in-process
  `DUAL` server.

## Alternatives Considered

### C ABI + `ctypes` / `cffi`
Would require hand-written `extern "C"` wrappers for every async operation and
a callback scheme for completion; more code than the binding itself.

### `uniffi`
Generates bindings for several languages from one interface definition, but its
async support wraps futures in a blocking worker model and its Python output is
not asyncio-native.

### Driving `moq-cli` as a subprocess
Zero Rust work, but only payload bytes cross the pipe; group/object ids and
extension headers are lost, and server mode is impossible.

## References
- https://pyo3.rs
- https://www.maturin.rs
