# [M] Keep a namespace's route when it is published again while its last publisher withdraws

Next: order the route's withdrawal against a concurrent registration, starting at `PublishNamespaceDone` (`services/relay/src/modules/control_plane/sequences/publish_namespace_done.rs:35`), which removes the session from the directory and awaits before it withdraws the route.

## Goal

A namespace a client publishes on this relay always has its route in the registry.
Reproduced with a route registry double that holds calls: client B's `register_route` (`publish_namespace.rs:47`) is in flight, client A's PUBLISH_NAMESPACE_DONE finds no client left and withdraws, B registers in the directory, A's withdrawal lands last.
The route also leaves `owned_routes`, so the heartbeat does not restore it: clients of other relays can neither subscribe to nor discover the namespace until it is published again.

## Plan

The same crossing exists when A disconnects (`session_workers/session_cleanup.rs:21`, withdrawal at `:84`); the window is one Redis round trip.
Open: derive the route from the directory by reconciliation, as `cascading/watched_namespace_routes.rs` does, or serialize the two per namespace.
