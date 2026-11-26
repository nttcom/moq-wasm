# [XS] Run the Python workflow when the relay's or VTS's crates change

Next: add `crates/auth-token`, `crates/telemetry-otlp`, `crates/app-registry`, `crates/google-cloud` and `crates/http-server` to both `paths` lists in `.github/workflows/python.yml:18`.

## Goal

A PR that changes only one of those crates runs `pytest`. The tests start the relay and VTS built from this workspace, and `cargo tree -p relay` / `-p vts` reach those crates, but today such a PR skips the workflow.
