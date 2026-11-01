# Serde and JSON for relay snapshots

## Status

Accepted

## What

`serde` with `derive` defines the relay snapshot's serialization, and
`serde_json` reads and writes its JSON payload.

## Context

The relay publishes snapshots over MoQT, and the observability server reads
and stores them. Both already depend on `telemetry`, so its `relay_stats`
module holds their shared format without a separate package.

## Alternatives

- Keep a separate snapshot crate. It preserves a smaller dependency set for
  standalone consumers, but neither current consumer needs that separation.
- Encode and decode fields manually. This duplicates Serde's field mapping
  and defaults, increasing the risk of breaking older snapshots.

## Decision

Keep the existing Serde derives and JSON conversion unchanged. Both
dependencies already exist in the workspace; the move adds no external
package to `Cargo.lock`.
