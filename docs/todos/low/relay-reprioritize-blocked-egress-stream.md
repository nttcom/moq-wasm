# [M] Reprioritize an egress stream while its write is blocked

Next: reproduce it: starve a low-priority subscription on a congested downlink, raise its Subscriber Priority, and watch for the `reprioritizing egress stream` DEBUG line of `services/relay/src/modules/data_plane/egress/group_sender.rs:442`.

## Goal

An update of the Subscriber Priority reaches an open stream at once, also while its task waits inside a write.
Today the task sees the update only between objects, and `moqt::StreamSender` holds the transport stream locked for the whole `send` (`crates/moqt/src/modules/moqt/data_plane/stream/stream_sender.rs:25`), so a starved stream keeps its old priority until some of its data gets through.

## Plan

Open question: whether a stream starves long enough to matter, since quinn buffers megabytes before a write blocks; measure before changing anything.
If it does: set the priority without the send lock (quinn's `SendStream::set_priority` takes `&self`), and have the task watch `DeliveryControl` while a write is pending.
