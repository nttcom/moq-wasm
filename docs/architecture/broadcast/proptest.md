# `proptest` for Group Alignment Tests in `broadcast`

## Status
Accepted (2026-11-19)

## What
Dev-dependency of `broadcast` (`default-features = false`, `std` only). Tests `GroupAlignment` across arbitrary interleavings of source video, rendition video, and audio (including repeated timestamps):
- Every track's group IDs strictly increase (always accepted by `Origin`).
- All tracks use only source-assigned group IDs, and rendition keyframes matching a remembered source keyframe reuse its ID.
- Each CMAF fragment starts the exact group its LOC sibling started (draft-ietf-moq-cmsf-01 §3.2).

## Context
`GroupAlignment` receives source video, renditions, and audio in unpredictable orders, while the fMP4 muxer buffers one frame and shifts duplicate decode times. Example tests missed a bug where a repeated-DTS CMAF keyframe started its own group; randomized interleavings find and shrink such bugs automatically.

## Alternatives
- **More example tests**: Only cover manually anticipated orderings.
- **Custom random loop**: Lacks minimal-case shrinking.

## Decision
Use `proptest` (matching `moqt` and `mediapack`): runs in `cargo test` on stable (256 cases default via `PROPTEST_CASES`) and stays out of dependent builds. Commit regression files written to `crates/broadcast/proptest-regressions/` alongside fixes.
