# [M] Rename the `moqt` `Publication` types that name MoQT state

Next: rename `Publication` and `PublicationTarget` in `crates/moqt/src/modules/moqt/domains/session_context.rs:88` after the subscription a PUBLISH starts (draft-14 §5.1).

## Goal

No MoQT state in `moqt` is called a publication, as the Naming rule in `.agents/rules/coding-style.md` requires: `Publication` and `PublicationTarget` (`session_context.rs:88`), `Publication` (`origin.rs:134`, the state of a served track), `SendTargets::Publication` (`send_targets.rs`) and `NamespaceAnnouncement::Publication` (`publisher.rs:42`) name a track or a subscription instead.

## Plan

Application-facing JS SDK types such as `SourcePublicationStats` stay: the rule allows the word in application-facing public APIs.
