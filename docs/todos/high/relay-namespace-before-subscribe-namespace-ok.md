# [M] Do not send a namespace to a SUBSCRIBE_NAMESPACE that has no response yet

Next: reproduce in a relay sequence test: a PUBLISH_NAMESPACE under the prefix arrives while `SubscribeNamespace::handle` awaits `register_route` (`services/relay/src/modules/control_plane/sequences/subscribe_namespace.rs:95`).

## Goal

A draft-18 client whose SUBSCRIBE_NAMESPACE races a PUBLISH_NAMESPACE keeps its session. Today it can close it with `the first response does not match the request` (`crates/moqt/src/modules/moqt/version/draft18/request_streams.rs:992`): in a `compose_e2e (live-translate)` run on 2026-11-11 the relay logged `PUBLISH_NAMESPACE sent` to the subscriber 1 ms before `SUBSCRIBE_NAMESPACE_OK sent`, and live-translate ended with `subscribe to the broadcasts' namespace: the session closed before the response arrived`.

## Plan

The subscription is registered in the directory before `handler.ok()`, with the await of `register_route` between them, so another session's PUBLISH_NAMESPACE finds it early. Open question: which message the client read first on the request stream; confirm it before choosing between answering before registering and holding namespaces until the response is written.
