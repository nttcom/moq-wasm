# [S] Reset a browser send stream dropped before close or reset

Next: check what the peer sees when `BrowserSendStream` (`crates/moqt/src/modules/transport/browser/browser_send_stream.rs:22`) is dropped mid-subgroup.

## Goal

A browser subgroup stream dropped before `close` or `reset` is reset with code 0, as the QUIC, WebTransport and QMux send streams are. Suspected from the code, not reproduced: dropping only releases the JS handles, so the stream likely stays open, neither FIN'd nor reset, until the session ends, and the relay waits on it.

## Plan

If confirmed, add a `Drop` that calls `writer.abort_with_reason` with code 0 unless `close` or `reset` ran; the abort's promise is awaited with `spawn_local`, as `close` does.
