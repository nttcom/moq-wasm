# [S] Keep `an_unencodable_subgroup_id_resets_only_that_subgroup` from failing on a reprioritized stream

Next: make `next_steps` (`services/relay/src/modules/data_plane/tests/unencodable_object.rs:33`) skip `Sent::Reprioritized`, or wait for group 0 to leave before opening group 1, after checking the cause below.

## Goal

The test passes on every run. In CI on 2026-11-24 it panicked with `unexpected Reprioritized(StreamPriority { subscriber_priority: 128, publisher_priority: 128, group_position: 0, subgroup_id: 0 })` on a branch that does not touch the relay.

## Plan

Cause read from the code, not reproduced: group 1's stream may open while the reset group 0 is still counted, then move to `group_position` 0 once it is gone (`position_moved`, `services/relay/src/modules/data_plane/egress/group_sender.rs:238`), which is correct relay behaviour.
Reproduce it first with the test in a loop (`cargo test -p relay --lib an_unencodable_subgroup_id -- --test-threads 1` repeated), to confirm the race before changing the test.
