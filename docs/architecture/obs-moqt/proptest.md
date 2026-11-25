# `proptest` for `MediaInput` Properties (`obs-moqt`)

**Status:** Accepted (2026-11-25)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `obs-moqt` to verify the timestamp handling of `media_input`:

- `ticks_to_micros` never maps a later tick to an earlier microsecond, across timebases and across zero.
- Whatever the first DTS and the B-frame reorder delay, video comes out with its first DTS at zero and its frame spacing and PTS lead unchanged.

## Context

OBS encoder packets carry timestamps in the encoder's timebase (for example 1001/30000 s), and the first video DTS is negative when the encoder reorders B-frames. `media_input` converts them to the unsigned microsecond `mediapack::Timestamp` with a division that rounds and a shift taken from the first packet.

## Alternatives

- **Example tests only:** Cover the frame rates listed in them; miss the sign boundary and uneven frame durations.
- **`cargo-fuzz`:** Requires nightly Rust and runs outside `cargo test`.

## Decision

Use `proptest`, matching `mediapack` and `broadcast`. It runs in `cargo test` on stable and stays out of the plugin binary. Generators lean toward real timebases (1/30, 1001/30000, 1001/60000, 1/48000) and toward ticks around zero.
