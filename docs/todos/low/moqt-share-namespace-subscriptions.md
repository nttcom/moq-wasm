# [L] Share one SUBSCRIBE_NAMESPACE among a session's watches and followers

Next: list the namespace subscriptions `client::Session` sends (`watch_namespaces`, `watch_tracks`, `follow_track` in `crates/moqt/src/modules/client/session.rs`) in one table keyed by prefix.

## Goal

One session can follow two tracks of one namespace, or follow a track under a prefix it watches, without the peer's NAMESPACE_PREFIX_OVERLAP (draft-14 §9.28; draft-18 judges SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS apart).
Today each call sends its own SUBSCRIBE_NAMESPACE, so the second one is rejected; no caller needs this yet.

## Plan

Open: a late consumer needs the namespaces already announced (cache them per subscription), and a PUBLISH has one answer, so pick which consumer takes it (the most specific?).
Open: a broader prefix requested after a narrower one replaces it (UNSUBSCRIBE_NAMESPACE the narrower) or fails locally; names-only and names-and-tracks interests merge how.
