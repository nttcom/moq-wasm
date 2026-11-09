# [M] Rename the `Publication` types that name MoQT state

Next: rename `Publication` and `PublicationTarget` in `crates/moqt/src/modules/moqt/domains/session_context.rs:56` after the subscription a PUBLISH starts (draft-14 §5.1).

## Goal

No MoQT state is called a publication, as the Naming rule in `.agents/rules/coding-style.md` requires: `relay` `MatchingPublication` (`services/relay/src/modules/directory/track.rs:323`), `python` `TrackPublication` (`crates/python/src/publication.rs`), `live-translate` `CatalogPublication` and the meeting example's `TrackPublication` name a track instead.
`recorder` `Publication` (`services/recorder/src/recordings.rs:66`) is a replay of a recording, not MoQT state; name it after the replay.

## Plan

The wasm `Publication` and `SourcePublication` stay: the rule allows the word in application-facing public APIs.
`python` `TrackPublication` is public API, but it is a MoQT track, so it is renamed too; one PR per crate keeps each rename small.
