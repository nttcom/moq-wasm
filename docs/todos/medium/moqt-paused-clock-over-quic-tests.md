# [M] Stop pausing the clock over a real QUIC connection in the update-timeout tests

Next: try connecting the sessions of `an_unanswered_namespace_prefix_update_gets_request_error_timeout` (`crates/moqt/src/modules/moqt/version/draft18.rs:4716`) over `qmux::in_memory` (`crates/moqt/src/modules/transport/qmux/in_memory.rs:72`) instead of UDP.

## Goal

The test failed once in `cargo test -p moqt`; reproduced 1/600 (30 parallel loops), panic `Closed(ConnectionLost)` at `draft18.rs:4726`. Sibling `an_unanswered_request_update_gets_request_error_timeout` (`:3804`) fails the same way at `:3807` (2/600).
Cause, seen in a traced failure: quinn reads tokio's paused clock (`TokioRuntime::now`), so whenever the runtime idles before a loopback datagram is readable, auto-advance jumps to quinn's next timer. 7 PTOs and 4 keep-alives went by in 3 ms of real time, then `timer=Idle` (30 s) closed both connections. Test bug, not a product bug.

## Plan

Fix the shape, not the odds: no paused clock while real sockets carry the exchange. If an in-memory session is not reachable, the fallback is driving the 10 s `CONTROL_MESSAGE_RESPONSE_TIMEOUT` (`version.rs:91`) another way; decide which.
Also check the other `time::pause()` tests on real QUIC (`draft14.rs:1183`, `subscriber.rs:3433`): they are safe only if nothing crosses the network while paused.
