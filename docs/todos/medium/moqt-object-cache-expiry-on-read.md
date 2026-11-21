# [M] Expire a track's cached objects when they are read, not only when one is written

Next: make the lookups in `crates/moqt/src/modules/moqt/domains/origin/object_cache.rs:51` skip objects past their retention, and reclaim an idle track's cache after its retention window.

## Goal

A track published with `retention_seconds=0.05` and not written for 400 ms answers a FETCH for its last object with no objects; today the relay still gets it (found by the Python SDK review, R2-10).
A long-lived session that served many rooms holds no room's cache past its retention.

## Plan

Keep the track's numbering when its cache is reclaimed, so registering the track again continues it (`Origin::end_track` relies on that).
