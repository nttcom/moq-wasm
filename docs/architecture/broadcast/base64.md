# `base64` for Catalog Initialization Data

## Status
Accepted (2026-10-14)

## What
Direct dependency of `broadcast`: encodes LOC audio `AudioSpecificConfig` and CMAF init segments into the catalog (`broadcast.rs`), and decodes `AudioSpecificConfig` on read (`selection.rs`).

## Context
MSF catalogs embed `AudioSpecificConfig` as Base64-encoded inline `initDataList` entries referenced by `initRef` (draft-ietf-moq-msf-02 §5.1.7, §5.2.14). `loc::Demuxer::audio` needs the raw bytes, which LOC objects do not carry.

## Alternatives
- **Hand-written Base64**: Duplicates a crate already used by `mediapack` and others.
- **Decode inside `msf`**: Mixes media decoding concerns into the catalog schema crate.

## Decision
Use `base64` at the workspace's existing locked version (no new crate in the tree).
