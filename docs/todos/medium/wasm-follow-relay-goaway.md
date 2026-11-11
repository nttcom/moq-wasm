# [S] Confirm browser sessions move to another relay on GOAWAY in production

Next: after the next production relay restart, search browser telemetry for `GOAWAY received; migrating the session` (`crates/moqt/src/modules/moqt/domains/session/migration_task.rs:158`) and the relay's `sending GOAWAY to every session` (`services/relay/src/relay_server/drain.rs:53`).

## Goal

A browser on a draining relay logs `GOAWAY received` and `moved to the new connection`, not `connection lost; reconnecting` (`migration_task.rs:182`).
The wasm path already follows GOAWAY: `run-goaway-e2e.mjs` passes on master, with the live viewer moving from relay A to relay B over WebTransport without a frame gap.

## Plan

Until 2026-11-10 the deployment named `localhost:443` in GOAWAY and killed the relay 10 s into its 30 s drain, losing its last logs; both are fixed on the deployment side, and no relay has drained since.
Open: browsers reach the relays through a UDP passthrough load balancer, while moq-cli dials a relay directly. On 2026-11-10 a browser lost its connection 6 s before the next relay began draining, so the balancer may move or drop the flow before the GOAWAY arrives; if browsers still log `connection lost`, look there first.
