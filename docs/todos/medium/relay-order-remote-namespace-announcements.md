# [S] Keep a remote relay's PUBLISH_NAMESPACE_DONE ahead of a later PUBLISH_NAMESPACE

Next: in `services/relay/src/modules/control_plane/sequences/publish_namespace_done.rs:193`, the DONE to remote subscriber relays is looked up and sent after `reconcile_publisher` (`:176`) released the namespace's lock; send it before a later publisher's PUBLISH_NAMESPACE (`publish_namespace.rs:92`) can reach the same relay.

## Goal

A remote relay subscribed to a namespace sees its last DONE and PUBLISH_NAMESPACE from this relay in the order the routes changed.
Found by reading, not reproduced: client A's withdrawal drops the route and awaits `find_namespace_subscribers` and `connect_relay`; client B publishes the namespace again, registers the route and announces it to the same relay, but `ControlMessageForwarder::publish_namespace_to_relay` / `announce_namespace` (`control_message_forwarder.rs:83`, `:120`) drops that PUBLISH_NAMESPACE because the relay session still has the namespace recorded as announced; A's DONE then goes out, so that relay withdraws the namespace while B publishes it.
The race predates `NamespaceRoutes`: the old code withdrew and announced with the same awaits in between.

## Plan

Open: send the DONE under the reconcile's lock, or queue both announcements per namespace in one task.
