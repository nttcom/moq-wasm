# [S] Resume a subgroup after a Largest Object with Object ID u64::MAX

Next: `services/relay/src/modules/domain/delivery_control.rs:151` saturates `largest.object_id + 1`.

## Goal

When the Largest Object at an update is `{g, u64::MAX}`, `resume_object_id(g, _)` returns `None` (the whole group lies at or before it) instead of `Some(u64::MAX)`, which sends object `{g, u64::MAX}` again.

## Plan

Use `Location::next()`, as `resolve_start_location` in `services/relay/src/modules/data_plane/egress/scheduler.rs` does.
