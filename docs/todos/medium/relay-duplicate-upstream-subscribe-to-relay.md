# [S] Do not subscribe a publisher relay twice to one track

Next: in `tracks_wanting_publisher` (`services/relay/src/modules/directory.rs:709`), leave out a track that already has a producer or a pending upstream SUBSCRIBE on the relay, also when `publisher_session_id` is `None` (the relay path, `publish_namespace.rs:126`).

## Goal

`compose_e2e (cascading-relay)` "dual relay namespace -> catalog -> track relay-a to relay-b" passes every run. It failed once (2026-11-11) with `timed out waiting for subscriber object`: relay-b sent relay-a two SUBSCRIBEs for `catalog`, one for the client's SUBSCRIBE (`subscribe.rs:363`) and one from the PUBLISH_NAMESPACE that arrived at the same time (`join_tracks_wanting_publisher_relays`).
relay-b then UNSUBSCRIBEd the first one (request 8). Read from the logs, not reproduced: that one carried the only object, and the one it kept (request 10) starts after it, so the client never got it.
