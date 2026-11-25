# `reqwest` for the ClickHouse HTTP Interface (`observability`)

**Status:** Accepted (2026-10-06)

## What

Add `reqwest` (`default-features = false`, feature: `rustls`) as a direct dependency of `observability`. `src/clickhouse.rs` executes DDL/queries and reads `FORMAT JSONEachRow` results over ClickHouse's HTTP interface.

## Context

`observability` writes one snapshot per relay per second to ClickHouse and reads time ranges for charts. ClickHouse's HTTP interface accepts the same SQL as its native TCP protocol and returns newline-delimited JSON.

## Alternatives

- **`clickhouse` crate (RowBinary over HTTP):** Typed rows and lower JSON overhead, but adds a new crate and derive macro when a few hundred rows/sec is nowhere near a JSON bottleneck.
- **`hyper` as an HTTP client:** Already present for the API server, but requires hand-writing connection pooling, URL encoding, and basic auth.

## Decision

Use `reqwest` over ClickHouse's HTTP interface. It is already in the workspace lock (`ingest-gateway pull`), adding no new crate to the dependency tree.
