# SHA-256 for archive chunks

## Status

Accepted.

## What and context

Use `aws-lc-rs` to identify corruption in saved archive payload and index
chunks (`archive_chunks.rs`). The archive reader must detect a damaged object
before replaying it.

## Alternatives

Storage ETags are provider-dependent and do not consistently identify the
contents. A custom implementation would add cryptographic code to maintain.
`sha2` would add a second SHA-256 next to the one the workspace already links.

## Decision

Use SHA-256 through `aws_lc_rs::digest`, the crypto library the workspace
already uses. Store the digest beside each chunk in the archive manifest.
