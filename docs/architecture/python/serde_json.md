# `serde_json` for Passing Telemetry Records to Python (`python`)

**Status:** Accepted (2026-11-16)

## What

Add `serde_json` 1.x (already in the workspace) as a dependency of `sdk/python`.

## Context

`telemetry_report::log_record` and `SpanRecordLayer` construct JSON telemetry records that `wait_log_records` hands to Python as JSON strings.

## Alternatives

- **`pythonize`:** Converts `serde_json::Value` directly into Python objects without intermediate JSON text, but adds a new crate for low-volume records parsed a few at a time on a background sink thread.

## Decision

Return each record from `wait_log_records` as JSON text and parse it on Python's sink thread with `json.loads`. No new crate enters the lock file, at the minor cost of one serialize/parse round trip per record.
