# ADR: serde_json in the python crate

## Status
Accepted

## Date
2026-11-04

## Context
`client_telemetry::ReportBuffer` holds the records as `serde_json::Value`, and `moqt.take_telemetry()` hands
them to Python.

## Decision
Add **`serde_json`** (1, already in the workspace) to `python`. `take_telemetry` serializes the records once and
returns `json.loads` of the text, so Python gets plain dicts and lists.

## Consequences

### Positive
- No new package in the lock file.

### Negative
- Each drain serializes and parses the records once more; at most 200 records every 10 seconds.

## Alternatives Considered

### `pythonize`
Converts a `Value` to Python objects without the text in between, but is a new crate for a path that runs once
per report.
