# PyO3 and `maturin` for Python Bindings (`python`)

**Status:** Accepted (2026-10-20)

## What

- Add `pyo3` (`abi3-py311`, `anyhow`) to `sdk/python` to expose the `moq_wasm._native` extension module.
- Add `pyo3-build-config` in `build.rs` for macOS extension link arguments.
- Build wheels with `maturin` (declared as the build backend in `pyproject.toml`, not a Cargo dependency).

## Context

Python bots require joining FETCH, object extension headers, and late-joiner publishing—capabilities provided by `moqt` but absent from `moq-rs` (see [architecture.md](architecture.md)). Exposing Rust to Python requires either a CPython C-API extension or a C ABI loaded via `ctypes`/`cffi`.

## Alternatives

- **`cffi` / `ctypes` over a C ABI:** Requires hand-written C shims, Python wrappers, and manual ownership/threading across every `moqt` type, async method, and iterator.
- **UniFFI (as used by `moq-ffi`):** Generates Python bindings with async support, but adds its own runtime glue and code generator for a single target language.
- **Separate Rust subprocess over JSON:** Avoids FFI, but adds a second process per bot and a custom IPC protocol.

## Decision

Use PyO3 and `maturin` (invoked by `uv`):

- `abi3-py311` produces one stable-ABI wheel for CPython 3.11+ (matching the bots' minimum version).
- `pyo3-build-config` with `PYO3_NO_PYTHON` in `.cargo/config.toml` lets Cargo build the workspace without a local Python interpreter.
- Because a CPython extension cannot link as a Cargo test binary, Cargo's test harness is disabled and tests run via `pytest`.
