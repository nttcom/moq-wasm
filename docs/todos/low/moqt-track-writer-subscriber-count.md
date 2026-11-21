# [S] Bound the subscriber notifications a TrackWriter keeps for `next_subscriber`

Next: replace the unbounded channel `crates/moqt/src/modules/client/track_writer.rs:23` gets from `routes.rs:191` with a pending count and a `Notify`, and drop the watcher when the writer is dropped.

## Goal

A data publisher that never calls `next_subscriber` holds constant memory however many SUBSCRIBEs it answers (Python SDK review, R3-22).
