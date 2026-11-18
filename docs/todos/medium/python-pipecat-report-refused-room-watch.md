# [S] Report a refused room watch in `MoqWasmTransport` through `on_error`

Next: in `crates/python/python/moq_wasm/pipecat/_room.py:96`, wrap the `watch_broadcasts` task so a `RuntimeError` reaches `self._sink.event("on_error", ...)` and the log.

## Goal

When the relay refuses the room's SUBSCRIBE_NAMESPACE, the transport fires `on_error` and logs it. Today the error stays in the task, and no participant's audio arrives, with nothing logged.
Reproduce: watch the room on the transport's session before it starts. The relay refuses the overlapping prefix (`services/relay/src/modules/directory.rs:452`).

## Plan

Open: whether a refused watch should also fail `start()`, as a refused broadcast does.
