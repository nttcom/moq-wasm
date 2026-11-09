# [S] Release a room lock whose recording never wrote its state

Next: read `Recordings::holds_active_recording` in `services/recorder/src/recordings.rs:320`; it returns `false` for a holder with no state but never unlocks it.

## Goal

A room whose lock was taken by a recorder API that died before writing `state/<id>.json` can be recorded again without deleting `locks/<prefix>` by hand. Today `Recordings::lock_room` (`recordings.rs:295`) sees the holder as inactive and retries `Store::lock_room`, whose create fails, so the room answers 409 forever.

## Plan

Release the lock once `minted_within_stale_window` (`recordings.rs:425`) returns false, and delete the limitation at `docs/architecture/recorder/architecture.md:544`.
