# [M] Release the `carried` lock before awaiting a request's answer

Next: in `crates/moqt/src/modules/moqt/domains/subscriber.rs:130`, find what the migration needs the lock for once SUBSCRIBE is written, then narrow it in `subscribe_joining` and `subscribe_namespace_answered` (`subscriber.rs:69`).

## Goal

A session that follows GOAWAY (`ClientConfig::default()`, so the browser and `broadcast`) holds the `carried` write lock until SUBSCRIBE_OK or SUBSCRIBE_NAMESPACE_OK arrives. Every other SUBSCRIBE, PUBLISH_NAMESPACE and PUBLISH on the session waits behind it, up to 10 s when the peer does not answer.
A test with an unanswered SUBSCRIBE shows a second SUBSCRIBE on the same session answered before the timeout.

## Plan

Open: `session_context.rs:39` says a request holds the lock "while it is sent"; check whether a migration that starts between the write and the answer would skip the request once the lock is dropped earlier.
