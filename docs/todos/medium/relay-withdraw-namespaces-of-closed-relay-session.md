# [S] Tell clients when the relay session a namespace came from closes

Next: in a two-relay run (`tests/cascading-relay-e2e`), kill relay-a after a client of relay-b learned its namespace, and check whether relay-b sends that client PUBLISH_NAMESPACE_DONE; the namespaces of a closed session are announced as withdrawn only `if was_client` (`services/relay/src/modules/control_plane/session_workers/session_cleanup.rs:64`).

## Goal

A client of relay B drops a broadcast of relay A once relay A crashes or restarts, instead of keeping the departed publisher and re-subscribing its catalog every 2 s.
Unverified: whether moqt's draft-18 layer raises PublishNamespaceDone for the namespaces of a SUBSCRIBE_NAMESPACE stream that ends with its connection, before Disconnected.
