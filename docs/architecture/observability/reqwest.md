# reqwest for the ClickHouse HTTP interface

## Status

Accepted

## Date

2026-10-06

## What

Add `reqwest` (with the `rustls` feature, no default features) as a direct
dependency of the `observability` crate. `src/clickhouse.rs` uses it to run the
schema statements, insert snapshot rows (`INSERT … FORMAT JSONEachRow` with
`async_insert`) and read query results (`FORMAT JSONEachRow`) through
ClickHouse's HTTP interface.

## Context

The observability server stores one snapshot per relay per second in
ClickHouse and reads time ranges back for the chart drawer. ClickHouse speaks
a native TCP protocol and an HTTP interface; the HTTP interface takes the same
SQL and returns rows as JSON lines.

## Alternatives

- The `clickhouse` crate (official Rust client, native RowBinary over HTTP).
  Typed rows and less JSON overhead, but a new crate with its own derive macro;
  at a few hundred rows per second JSON is not a bottleneck.
- `hyper` as an HTTP client. Already needed for the API server, but the client
  side (connection pool, URL encoding, basic auth) would be hand-written.

## Decision

Use `reqwest` over the HTTP interface. It is already in the workspace lock at
0.13.4 (`relay`, `onvif-ingest`), so no new crate enters the dependency tree,
and the rows serialize with the `serde` derives `relay-stats` already has.
