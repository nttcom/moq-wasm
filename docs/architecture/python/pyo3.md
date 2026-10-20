# PyO3 and maturin for the Python bindings

## Status

Accepted

## Date

2026-10-20

## What

`crates/python` depends on `pyo3` (with `abi3-py311` and `anyhow`) to define
the Python module `moqt`, and on `pyo3-build-config` in `build.rs` for the
macOS link arguments of an extension module. maturin builds the module as a
wheel; it is a build tool named in `pyproject.toml`, not a Cargo dependency.

## Context

The Python bots need joining FETCH, object extension headers and a publisher
that answers late joiners, which moq-rs does not give them and `moqt` does
(see [architecture.md](architecture.md)). Python reaches Rust code either
through the C API of an extension module or through a C ABI loaded with
`ctypes`/`cffi`.

## Alternatives

- **cffi or ctypes over a C ABI.** The `moqt` types, async methods and
  iterators would each need a hand-written C shim and a Python wrapper, with
  ownership and threading crossing the boundary by hand.
- **UniFFI**, which moq-ffi uses. It generates the Python side from an
  interface definition and supports async, but adds its own runtime glue and
  a code generator to the build, for one target language.
- **A separate Rust process speaking JSON to Python.** No binding at all, but
  a second process per bot and a protocol of its own to maintain.

## Decision

Use PyO3, the common way to write a CPython extension in Rust, and maturin,
its build tool, which uv runs as the build backend. `abi3-py311` builds one
module for every CPython from 3.11, the version the bots require, and lets
`pyo3-build-config` configure the build without an interpreter
(`PYO3_NO_PYTHON` in `.cargo/config.toml`), so building the workspace needs no
Python. The extension cannot link as a Cargo test binary, so the crate turns
its test harness off and is tested with pytest.
