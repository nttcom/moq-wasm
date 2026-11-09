# [M] Let live-ingest and onvif-ingest publish into a temporary app

Next: read how live-ingest maps a stream key to a namespace in `services/live-ingest/src/namespace.rs`.

## Goal

In the examples' temporary mode (`examples/browser/utils/access.ts`), an RTMP/SRT stream and an ONVIF camera show up under the page's `temp-…` root.
Today both bridges publish under `anon` only, so the live-viewer's RTMP/SRT path and the onvif example show nothing in that mode.

## Plan

Take the page's temporary token with the stream (e.g. in the stream key or the ingest config) and renew it at the token mint, as `services/recorder/src/token_refresh_task.rs` (`TokenRenewal::Issuer`) does.
