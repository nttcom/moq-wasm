# [S] Reset a native QUIC send stream dropped before close or reset

Next: give `QUICSendStream` (`crates/moqt/src/modules/transport/quic/quic_send_stream.rs:16`) a `Drop` that resets with code 0 unless `close` or `reset` ran, as `QmuxSendStream` does (`crates/moqt/src/modules/transport/qmux/qmux_send_stream.rs:221`).

## Goal

A subgroup stream whose sender is dropped mid-subgroup is reset, not FIN'd. Read from the code, not reproduced: quinn's `SendStream::drop` calls `finish()`, so when the relay's egress runner ends on MALFORMED_TRACK or an unencodable object and drops its other send tasks, their partial subgroups look complete to the subscriber (draft-14 §10.4.3). The QMux comment "As in quinn" is wrong about quinn.

## Plan

Check the WebTransport send stream (`wt_send_stream.rs`) for the same drop behaviour.
