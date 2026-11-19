# [S] Start a subscription after a Largest Object with Object ID u64::MAX without overflowing

Next: `crates/moqt/src/modules/moqt/control_plane/control_messages/messages/parameters/filter_type.rs:38` adds 1 to the Largest Object's Object ID.

## Goal

A draft-18 Largest Object `{g, u64::MAX}` makes the next start `{g + 1, 0}` instead of panicking (debug) or wrapping to `{g, 0}` and resending group g (release).
The same `+ 1` sits in `first_whole_group` (`crates/moqt/src/modules/moqt/domains/session/migration_task.rs:776`) and the relay's `resolve_start_location` (`services/relay/src/modules/data_plane/egress/scheduler.rs:21`).

## Plan

One `Location` method for "the first location after this object", next to `Location::end_after`. Group ID u64::MAX has no next group: decide whether that start is empty.
