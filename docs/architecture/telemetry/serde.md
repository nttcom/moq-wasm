# Serde and JSON for relay snapshots

## Status

Accepted

## What

`serde` with `derive` defines the relay snapshot's serialization, and
`serde_json` reads and writes its JSON payload.

## Context

The relay sends snapshots as the JSON attribute of an OTLP log record, and
the observability server decodes the ones it stored for `?at=` reads. The
`relay_stats` module of `telemetry` holds their shared format.

## Alternatives

- Encode and decode fields manually. This duplicates Serde's field mapping
  and defaults, increasing the risk of breaking older snapshots.

## Decision

Keep the existing Serde derives and JSON conversion unchanged. Both
dependencies already exist in the workspace; the move adds no external
package to `Cargo.lock`.
