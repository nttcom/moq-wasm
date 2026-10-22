# clap for the auth-token CLI

## Status

Accepted

## Date

2026-10-22

## What

Add `clap` as an optional dependency of the `auth-token` crate, behind the
`cli` feature, to parse the arguments of the `auth-token` binary.

## Context

Operators, tests and services sign client tokens from the command line. The
signing lived in `vts` as `vts-mint`, a second binary of a service that only
verifies, and read the whole registry to find the secret. Signing belongs to
`auth-token`, so the CLI moves there and takes only the secret.

## Alternatives

- A separate CLI crate. A crate for a few dozen lines that only call
  `auth-token`.
- Parse the arguments by hand. Less code to pull in, more code to keep
  correct for help text and errors.
- A non-optional dependency. Every crate that uses the library, the relay
  among them, would build `clap`.

## Decision

Use `clap` with `derive`, as an optional dependency the `cli` feature turns
on. It is already in the workspace lock through other binaries, and the
library's users never build it.
