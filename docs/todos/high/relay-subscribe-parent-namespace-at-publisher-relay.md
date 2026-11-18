# [M] Announce a namespace whose sub-namespace this relay already subscribed at the publisher relay

Next: in `services/relay/src/modules/cascading/relay_namespace_subscriptions.rs:18`, handle a new namespace that is a prefix of one the session already holds. Today only the reverse case counts as covered.

## Goal

A client on relay B sees both `a/b` and `a/b/c` when another relay A publishes them. Today B subscribes `a/b/c` at A first, then `a/b` on the same session (`sequences/subscribe_namespace.rs:478`). A refuses the second as overlapping, so B never announces `a/b`.
Seen in the cloud: `anon/moq-ai-voice-agent/{room}/gemini` and `.../gemini/transcript` came from relay-1. relay-2 showed both, relay-3 only the transcript. The order is the route registry's `KEYS` order.

## Plan

Open: whether to replace the narrower subscription with the wider one (UNSUBSCRIBE_NAMESPACE, then SUBSCRIBE_NAMESPACE), or to look the namespaces up shortest first and keep the existing covering rule.
