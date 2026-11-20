# [M] Stop a subgroup cursor after the object with Object ID u64::MAX

Next: `services/relay/src/modules/data_plane/egress/group_sender.rs:714` sets `sent.unsent = object_id.saturating_add(1)`.

## Goal

After sending object `{g, u64::MAX}`, egress waits for the subgroup's end instead of reading `{g, u64::MAX}` again: `group_objects` reads `from_object_id..=u64::MAX`, so a cursor left at `u64::MAX` returns the same object and sends it in a loop.

## Plan

The same `saturating_add(1)` cursor is in `group_sender.rs` (`held`, the datagram loops), `fetch_cursor.rs:97`, and the cache's `after()`, `knowledge_frontier` and `start_run_if_reopening_aborted` (`track_cache.rs`, `ledger.rs`). Give the cursor a "past the group" state (`Option<u64>` or `Location::next()`) in one place. `fetch_plan.rs` adds 1 to Group IDs unchecked.
