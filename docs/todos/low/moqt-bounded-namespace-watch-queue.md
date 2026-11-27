# [S] Bound the events a slow SUBSCRIBE_NAMESPACE reader leaves queued

Next: replace the unbounded channel of `crates/moqt/src/modules/moqt/domains/namespace_watchers.rs:28` with a bounded one and decide what a full queue does, and unregister a watcher on `NamespaceWatch::close` (`crates/moqt/src/modules/client/namespace_watch.rs:45`).

## Goal

A client that stops reading a namespace watch holds a bounded number of events (found by the Python SDK review, R2-14).

## Plan

Open: coalesce per namespace (the latest active/withdrawn state) instead of dropping, so a reader that catches up sees the current set.
