# [S] Accept SUBSCRIBE_NAMESPACE for a prefix whose last subscriber is still being cleaned up

Next: in session cleanup (`services/relay/src/modules/control_plane/session_workers/session_cleanup.rs:21`), the session leaves the directory before the prefix's route is withdrawn (`sequences/unsubscribe_namespace.rs:73`); make the two one step for a subscriber that arrives between them.

## Goal

A client that subscribes to a prefix right after its last subscriber disconnected is answered SUBSCRIBE_NAMESPACE_OK.
Reproduced with a route registry double that holds calls: the new client is the prefix's first client, `register_route` (`sequences/subscribe_namespace.rs:96`) finds this relay's own entry still in the registry, and the client is answered SUBSCRIBE_NAMESPACE_ERROR with NamespacePrefixOverlap.

## Plan

The window is the Redis round trips for the tracks the leaving session published.
Same shape as `high/relay-keep-namespace-route-on-republish.md`; one ordering rule may fix both.
