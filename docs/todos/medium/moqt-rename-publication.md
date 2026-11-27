# [M] Rename the `Publication` types that name MoQT state

Next: rename `Publication` and `PublicationTarget` in `crates/moqt/src/modules/moqt/domains/session_context.rs:81` after the subscription a PUBLISH starts (draft-14 §5.1).

## Goal

No MoQT state is called a publication, as the Naming rule in `.agents/rules/coding-style.md` requires: `moqt` `Publication` (`crates/moqt/src/modules/moqt/domains/origin.rs:137`, the state of a served track), `relay` `MatchingPublication` (`services/relay/src/modules/directory/track.rs:481`), `live-translate` `CatalogTrack`, the meeting example's `TrackPublication` and `cross-version-e2e`'s `Publication` (`tests/cross-version-e2e/src/main.rs:99`) name a track or a subscription instead.
`recorder` `Publication` (`services/recorder/src/recordings.rs:50`) is a replay of a recording, not MoQT state; name it after the replay.

## Plan

The wasm `Publication` and `SourcePublication` stay: the rule allows the word in application-facing public APIs.
One PR per crate keeps each rename small.
