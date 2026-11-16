# ADR: serde_json in the python crate

## Status
Accepted

## Date
2026-11-16

## What
`crates/python` depends on `serde_json` (1, already in the workspace).

## Context
`client_telemetry::log_record` and `SpanRecordLayer` build the records as `serde_json::Value`, and
`wait_log_records` hands them to Python.

## Alternatives
- **`pythonize`**: converts a `Value` to Python objects without the text in between, but is a new
  crate for records the sink thread parses a few at a time.

## Decision
`wait_log_records` returns each record as its JSON text, which the sink thread parses with
`json.loads`. The lock file gains no package; each record is serialized and parsed once more.
