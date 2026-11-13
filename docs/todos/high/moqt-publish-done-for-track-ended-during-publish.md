# [S] Send PUBLISH_DONE for a track that ends while its PUBLISH awaits PUBLISH_OK

Next: in `Origin::end_track` (`crates/moqt/src/modules/moqt/domains/origin.rs:664`), wait for the `publish` in flight as `release` does (`:634`), or make `publish` see the ended track before it registers its outlet.

## Goal

A subscriber that accepts a PUBLISH receives PUBLISH_DONE when the track has ended.
Reproduced on both drafts: `end_track` called after the peer received PUBLISH and before `publish` registered the outlet returns `Ok` and sends nothing; `publish` then registers the outlet.
The subscriber waits for objects that never come and the origin keeps serving the track to it.
