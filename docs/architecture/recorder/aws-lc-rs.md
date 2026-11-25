# `aws-lc-rs` SHA-256 for Archive Chunk Integrity (`recorder`)

**Status:** Accepted

## What

Use `aws_lc_rs::digest` (SHA-256) in `archive_chunks.rs` to hash archive payload and index chunks and store each digest in the archive manifest.

## Context

The archive reader must detect corrupted payload or index chunks before replaying them.

## Alternatives

- **Storage ETags:** Provider-dependent and not guaranteed to match raw content hashes.
- **Custom SHA-256 implementation:** Adds hand-maintained cryptographic code.
- **`sha2`:** Adds a second SHA-256 implementation alongside the crypto library the workspace already links.

## Decision

Compute SHA-256 digests with `aws_lc_rs::digest`—already linked across the workspace—and record each digest beside its chunk in the archive manifest.
