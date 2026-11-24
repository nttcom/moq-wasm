# [M] Remember a PUBLISH_BLOCKED on the SUBSCRIBE_TRACKS it was sent on

Next: in `open_publish_stream` (`crates/moqt/src/modules/moqt/version/draft18.rs:641`), record each track PUBLISH_BLOCKED is sent for on that SUBSCRIBE_TRACKS stream, and answer a later PUBLISH of it with `RequestFailure::PublishBlocked` without writing.

## Goal

No PUBLISH follows a PUBLISH_BLOCKED on the same SUBSCRIBE_TRACKS (draft-18 §6.1 MUST NOT), also after its prefix moved (§10.9.2).
Today moqt sends what it is asked to, and only the relay remembers the block, per session and track (`blocked_publishes` in `services/relay/src/modules/directory.rs`). `unregister_subscribe_namespace` drops it when a REQUEST_UPDATE moves the subscription, so the next PUBLISH of a blocked track the new prefix still covers goes to the subscriber again.
Reproduced in a unit test of `SubscribeNameSpace::handle_update`: block a track on a SUBSCRIBE_TRACKS of `ns`, move it to the empty prefix, and `is_publish_blocked` turns false.

## Plan

The request stream stays the same across a prefix update and ends with the subscription, so a block kept on it needs no hand-over. Once moqt refuses the PUBLISH, the relay's `blocked_publishes`, `block_publish` and `is_publish_blocked` can go.
`medium/moqt-publish-blocked-before-subscribe-tracks-answer.md` changes the same function.
