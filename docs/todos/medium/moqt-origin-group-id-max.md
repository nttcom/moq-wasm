# [S] Let the origin write group u64::MAX without overflowing

Next: `crates/moqt/src/modules/moqt/domains/origin/object_numbering.rs:81` sets the next group to `group_id + 1`.

## Goal

A draft-18 origin that places `GroupBoundary::At(u64::MAX)` writes that group instead of panicking (debug) or wrapping the next Group ID to 0 (release).

## Plan

`next_group_id` cannot be held once group u64::MAX is open: decide whether a later `GroupBoundary::Next` fails or the numbering keeps an `Option`.
