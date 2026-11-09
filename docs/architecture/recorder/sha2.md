# SHA-256 for archive chunks

## Status

Accepted.

## What and context

Use `sha2` to identify corruption in saved archive payload and index chunks.
The archive reader must detect a damaged object before replaying it.

## Alternatives

Storage ETags are provider-dependent and do not consistently identify the
contents. A custom implementation would add cryptographic code to maintain.

## Decision

Use SHA-256 through `sha2`, already present in the workspace lockfile. Store
the digest beside each chunk in the archive manifest.
