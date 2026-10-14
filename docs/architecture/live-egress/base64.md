# base64 for the audio configuration in the catalog

## Status

Accepted

## Date

2026-10-14

## What

Add `base64` as a dependency of `live-egress`. The broadcast reader decodes
the audio track's AudioSpecificConfig from its catalog `initDataList` entry.

## Context

A `broadcast` catalog carries a LOC audio track's AudioSpecificConfig as a
Base64-encoded inline `initDataList` entry named by the track's `initRef`
(draft-ietf-moq-msf-01 §5.1.7, §5.2.13). `loc::Demuxer::audio` needs the
decoded bytes, and the LOC objects themselves do not repeat them.

## Alternatives

- Decode Base64 by hand. A second implementation of a routine `broadcast`,
  `mediapack` and three other crates already take from `base64`.
- Have `msf` decode `initDataList` entries. Moves a decoding concern into the
  catalog model crate for one caller; `broadcast`, which encodes the entries,
  does so itself as well.

## Decision

Depend on `base64` at the version the workspace already locks, so no new
crate enters the dependency tree.
