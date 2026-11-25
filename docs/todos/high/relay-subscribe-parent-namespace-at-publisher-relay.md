# [M] Announce a namespace whose sub-namespace an earlier SUBSCRIBE_NAMESPACE already subscribed at the publisher relay

Next: in `services/relay/src/modules/cascading/relay_namespace_subscriptions.rs:18`, handle a new namespace that is a prefix of one the session already holds. Today only the reverse case counts as covered.

## Goal

A client on relay B sees `a/b` when another relay A publishes it after an earlier client's SUBSCRIBE_NAMESPACE made B subscribe `a/b/c` at A. B then subscribes `a/b` on the same session (`subscribe_at_publisher_relays`, `sequences/subscribe_namespace.rs`), A refuses it as overlapping, and B never announces `a/b`.
Within one SUBSCRIBE_NAMESPACE the namespaces are subscribed shortest first, so the case seen in the cloud (`.../gemini` and `.../gemini/transcript` listed in the route registry's `KEYS` order) is fixed; this one is read from the code, not reproduced.

## Plan

Open: replace the narrower subscription with the wider one (UNSUBSCRIBE_NAMESPACE, then SUBSCRIBE_NAMESPACE) without B withdrawing `a/b/c` from its clients in between.
