# [M] Move browser sessions to another relay on GOAWAY

Next: open a browser session on a local relay, stop the relay with SIGTERM, and check whether the page logs `GOAWAY received; migrating the session` (`crates/moqt/src/modules/moqt/domains/session/migration_task.rs:158`).

## Goal

A browser on a draining relay moves to the relay in the GOAWAY instead of waiting for the connection to drop.
Evidence: in the week up to 2026-11-10, browser telemetry never logged `GOAWAY received`, while moq-cli did and browsers logged `connection lost; reconnecting` (`migration_task.rs:182`) when relays restarted.
Unknown: whether GOAWAY reaches the wasm session at all; `crates/wasm/src/session.rs:908` takes `ClientConfig::default()`, which keeps `follow_go_away` on.
