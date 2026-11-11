# [S] Keep an upstream SUBSCRIBE from counting down a track created after it was sent

Next: in `PendingProducerTask::run` (`services/relay/src/modules/control_plane/sequences/subscribe/pending_producer_task.rs:90`), end the pending producer and the `Track::subscribing` claim only on the track entry the SUBSCRIBE was sent for, e.g. by a generation stored with the entry.

## Goal

A SUBSCRIBE sent for a track whose entry is removed and created again while it is in flight leaves the new entry alone. Read from the code, not reproduced: today its `pending_producer_ended` counts down the new entry's `pending_producers`, so its failure can fail a creation whose own SUBSCRIBE is still in flight, and its `end_upstream_subscribe` drops the new entry's claim on the same publisher.
