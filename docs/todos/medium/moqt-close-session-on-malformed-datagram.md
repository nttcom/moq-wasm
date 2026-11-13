# [S] Close the session on a malformed object datagram

Next: in `on_datagram_received` (`crates/moqt/src/modules/moqt/runtime/tasks/datagram_receive_task.rs:43`), close the session with PROTOCOL_VIOLATION on a decode error instead of logging it and dropping the datagram.

## Goal

A datagram of an undefined type, or with Extensions Present and an empty extension block, ends the session, as draft-14 §10.3.1 requires (MUST).
Today `DatagramField::decode` rejects both, but the receive task only logs an ERROR and reads the next datagram.
