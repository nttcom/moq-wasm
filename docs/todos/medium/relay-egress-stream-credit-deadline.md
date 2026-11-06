# [M] Give up on a subscriber that never grants stream credit

Next: read the `factory.next(priority)` call in `send_stream` (`services/relay/src/modules/data_plane/egress/group_sender.rs:248`), which waits in `open_uni` for the subscriber's MAX_STREAMS credit.

## Goal

A subscriber that never grants stream credit loses its send tasks after a deadline instead of keeping them until the subscription or session ends.

## Plan

These tasks are capped by `MAX_CONCURRENT_SEND_TASKS` (`group_sender.rs:32`) and hold no cached object while they wait. Open: whether the deadline drops the group (and logs it) or ends the subscription, and whether `open_uni` waiting on one shared factory lock delays the other groups.
