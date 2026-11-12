# [S] Stop logging a stream of a cancelled FETCH as an ERROR

Next: in `crates/moqt/src/modules/moqt/runtime/dispatch/fetch_notifier.rs:36`, tell a FETCH this side cancelled from one it never sent before choosing the log level.

## Goal

A FETCH this side cancels (FETCH_CANCEL) can cross the peer's FETCH stream, which then logs `no fetch stream sender for the request` at ERROR. A reader dropped while its Joining FETCH is unanswered does that, as when a viewer leaves right after `subscribe_current_group`. That race is expected, so it logs at DEBUG; a stream for a request this side never sent stays an ERROR.

## Plan

Open: whether the stream that arrives is closed (STOP_SENDING) or only dropped; read what dropping `FetchDataReceiver` does first.
