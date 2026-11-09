# [S] Find why the SUBSCRIBE of a browser camera's media timeline is rejected right after it is published

Next: run `node tests/browser-e2e/run-media-e2e.mjs` and match the `publisher_session_id` of the relay's WARN `upstream SUBSCRIBE failed ... camera_timeline is not published here` to the session that sent it, the media publisher's or the test page's bot session.

## Goal

A `RemoteBroadcast` that starts as its publisher starts (`tests/remote-broadcast-e2e.spec.ts`, the `watchBroadcasts` tests) gets `camera_timeline` on its first SUBSCRIBE; today the relay logs the WARN once per test.
The rejection comes from the fallback arm of `crates/broadcast/src/relay_connection.rs:328`, which answers a SUBSCRIBE that `serve_subscribe` did not take.

## Plan

Open: whether the catalog lists `<source>_timeline` before `serve_subscribe` accepts it (`createBroadcast` declares only `<name>` and `<name>_audio` with `declareTracks`), or the relay sends the SUBSCRIBE to another session.
