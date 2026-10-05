# hyper-util for the tokio I/O adapter

## Status

Accepted

## Date

2026-10-05

## What

Add `hyper-util` (feature `tokio`) as a direct dependency of the `vts` crate
for `hyper_util::rt::TokioIo`, which wraps a `tokio::net::TcpStream` in the
`hyper::rt::Read` / `Write` traits that `hyper` 1.x requires.

## Context

`hyper` 1.x is runtime-agnostic and no longer accepts tokio streams directly;
the adapter lives in `hyper-util`. See [hyper.md](hyper.md).

## Alternatives

- Implement `hyper::rt::Read` and `hyper::rt::Write` for a local newtype over
  `TcpStream`. Around 60 lines that duplicate `TokioIo`.
- `hyper-util`'s `auto` server builder. Adds HTTP/2 support the relay's
  `reqwest` client does not use against the VTS.

## Decision

Use `hyper-util` for `TokioIo` only. It is already in the workspace lock as a
dependency of `reqwest`, so nothing new is compiled.
