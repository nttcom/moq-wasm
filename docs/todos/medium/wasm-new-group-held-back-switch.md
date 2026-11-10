# [M] Finish a rendition switch and `goLive` when the relay holds back the new group request

Next: in `crates/wasm/js/src/media/player/livePlayer.ts` `spliceVideo`, time out a splice whose start group has not come 1 s after `requestNewGroup` and fall back to `resubscribe('video')`.

## Goal

A rendition switch or `goLive` shows the new picture within about 1 s even when another viewer's NEW_GROUP_REQUEST for the same track went upstream less than 2 s earlier.
Today the relay holds the second request back (`NEW_GROUP_REQUEST_INTERVAL`), nothing tells the viewer, and both wait for the next keyframe, up to 30 s. Not reproduced; follows from `new_group_request_gate.rs` and `TrackSubscriptions.setForward`.

## Plan

Only `Subscriber::subscribe_group_start` falls back to a Joining FETCH. `resubscribe` uses it, at the cost of a visible catch-up; a mid-group splice would avoid that but needs the decoder to catch up unseen.
