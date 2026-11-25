# [S] Keep a prefix's route for a client that subscribed while the first one was registering it

Next: in `services/relay/src/modules/control_plane/sequences/subscribe_namespace.rs:96`, when the first client's `register_route` fails, hand the route to a client that joined meanwhile instead of only unregistering the first one.

## Goal

A prefix a client is subscribed to on this relay always has its route in the registry.
Today client B subscribes while client A, the prefix's first client, is in `register_route`: B is answered OK at once, A's registration fails (Redis error, or the conflict of `high/relay-accept-namespace-subscriber-during-cleanup.md`), A leaves the directory, and B stays subscribed with no route.
Found by `docs/architecture/relay/models/namespace.qnt` (invariant `subscribedPrefixHasRoute`, 4 steps).
