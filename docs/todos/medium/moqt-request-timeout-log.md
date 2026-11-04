# [S] Log a request that times out and record it on its span

Next: add an INFO log to the timeout arm of `PendingResponse::response` in `crates/moqt/src/modules/moqt/version/draft14.rs:501`; it needs the request kind, `request_id` and track, which `SentRequest` in `crates/moqt/src/modules/moqt/version.rs` already builds.

## Goal

A request without an answer for 10 s logs one line (`SUBSCRIBE timed out`, ...) with `request_id`, the track and `duration_ms`. Today the only sign is a `* sent` log with no matching `*_OK`/`*_ERROR received`.
The request span (`moqt.subscriber.subscribe`, `moqt.publisher.publish`, ...) gets `request_id` and `outcome = "timeout"` when it times out; `record_request_id` and `record_outcome` in `crates/moqt/src/modules/moqt/domains/request_failure.rs:30` record neither.

## Plan

Open: `outcome` is in the AGENTS.md vocabulary only for calls the relay makes to another service; widen its meaning there to request outcomes too. `track_status` has a debug span with no `outcome` at all.
