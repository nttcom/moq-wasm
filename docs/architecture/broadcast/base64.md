# base64 for the audio configuration in the catalog

## Status

Accepted

## Date

2026-10-14

## What

`broadcast` depends on `base64`. The writer encodes a LOC audio track's
AudioSpecificConfig and the CMAF init segments into the catalog
(`broadcast.rs`), and the reader decodes the AudioSpecificConfig
(`selection.rs`).

## Context

A `broadcast` catalog carries a LOC audio track's AudioSpecificConfig as a
Base64-encoded inline `initDataList` entry named by the track's `initRef`
(draft-ietf-moq-msf-02 §5.1.7, §5.2.14). `loc::Demuxer::audio` needs the
decoded bytes, and the LOC objects themselves do not repeat them.

## Alternatives

- Decode Base64 by hand. A second implementation of a routine `mediapack`
  and other crates already take from `base64`.
- Have `msf` decode `initDataList` entries. Moves a decoding concern into the
  catalog model crate.

## Decision

Depend on `base64` at the version the workspace already locks, so no new
crate enters the dependency tree.
