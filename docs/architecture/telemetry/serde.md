# `serde` and `serde_json` for Relay Snapshots (`telemetry`)

**Status:** Accepted

## What

Depend on `serde` (`derive`) and `serde_json` in `telemetry` to serialize and deserialize relay snapshot JSON payloads in `relay_stats`.

## Context

`relay` attaches snapshots as JSON attributes on OTLP log records, and `observability` decodes stored snapshots for `?at=` historical queries. `telemetry::relay_stats` defines their shared schema.

## Alternatives

- **Manual field encoding/decoding:** Duplicates `serde` field mappings and defaults, increasing the risk of breaking compatibility with older snapshots.

## Decision

Keep the `serde` derives and `serde_json` serialization in `telemetry`. Both crates already exist in `Cargo.lock`, adding no new external dependency.
