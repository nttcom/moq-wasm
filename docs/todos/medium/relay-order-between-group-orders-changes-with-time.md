# [M] Find what should decide the order between an Ascending and a Descending subscription of equal priority

Next: write down what the group part of `StreamPriority::transport_priority` (`crates/moqt/src/modules/moqt/data_plane/stream_priority.rs:32`) compares when one subscription is Ascending and the other Descending, and reproduce both halves of the relay's cycle on a 2 Mbps downlink.

## Goal

Two subscriptions of equal subscriber and publisher priority behave the same whenever they run.
Today the group part is the low 12 bits `t` of the relay's 256 ms tick for Ascending and `4095 - t` for Descending, so which of the two goes first swaps every 8.7 minutes, and the other sends nothing while the first cannot drain.

## Plan

Measured: with the relay's tick in the first half of its cycle a small Ascending track is sent ahead of Descending video; in the second half it is not sent at all.
Not decided: what the order should be. Draft-14 §7.2 leaves it to the implementation and expects every subscription to send some data.
