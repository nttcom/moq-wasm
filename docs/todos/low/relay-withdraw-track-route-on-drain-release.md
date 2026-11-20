# [S] Withdraw a PUBLISHed track's route when a draining relay releases it

Next: withdraw the route where a drain release hands back a PUBLISH-initiated producer, starting at `Unsubscribe::handle` (`services/relay/src/modules/control_plane/sequences/unsubscribe.rs:47`).

## Goal

A track this relay no longer receives keeps no route here: the release forgets the PUBLISH handler, so the publisher's PUBLISH_DONE finds no producer (`upstream_publish_done.rs:25`) and its session cleanup no longer lists the track.
`ReleaseUnwatchedPublishes` (`drain_release.rs`) leaves the route the same way. The heartbeat keeps refreshing it in Redis until the relay exits.

## Plan

Low because other relays ignore the routes of a draining relay (`find_active_routes` keeps `active` relays only).
