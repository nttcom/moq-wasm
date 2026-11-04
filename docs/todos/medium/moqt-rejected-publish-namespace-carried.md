# [S] Stop carrying a PUBLISH_NAMESPACE the peer rejected

Next: in `Publisher::publish_namespace_answered` (`crates/moqt/src/modules/moqt/domains/publisher.rs:56`), remove the namespace from `carried.published_namespaces` when `accepted()` fails, as `subscribe_namespace_answered` does for SUBSCRIBE_NAMESPACE.

## Goal

A PUBLISH_NAMESPACE answered with PUBLISH_NAMESPACE_ERROR or timed out stays in `published_namespaces` (pushed in `announce`, `publisher.rs:92`), so the next GOAWAY migration sends it again and fails when the new relay rejects it too. A test with a rejected PUBLISH_NAMESPACE shows `published_namespaces` empty.

## Plan

`begin_publish_namespace` returns the pending answer to the caller, so that path cannot remove it inside `publish_namespace_answered`; decide whether `PendingPublishNamespace::accepted` removes it.
