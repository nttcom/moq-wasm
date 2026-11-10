# [S] Stop logging a stream of a cancelled FETCH as an ERROR

Next: in `crates/moqt/src/modules/moqt/runtime/dispatch/fetch_notifier.rs:36`, tell a FETCH this side cancelled from one it never sent before choosing the log level.

## Goal

A browser viewer subscribing to `camera` logs `no fetch stream sender for the request` at ERROR (`request_id` 14) 2 ms after SUBSCRIBE_OK. `subscribe_group_start` dropped its Joining FETCH (FETCH_CANCEL) to send NEW_GROUP_REQUEST, and the relay's FETCH stream crossed it. That race is expected, so it logs at DEBUG; a stream for a request this side never sent stays an ERROR.

## Plan

Open: whether the stream that arrives is closed (STOP_SENDING) or only dropped; read what dropping `FetchDataReceiver` does first.
