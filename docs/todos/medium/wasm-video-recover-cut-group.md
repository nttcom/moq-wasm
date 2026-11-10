# [M] Ask for a new group when a live video group is cut short

Next: in `crates/wasm/js/src/media/internal/decoders/videoDecoder.ts:290`, post a message when a delta is dropped while `waitingForKeyFrame`, and have `LivePlayer` call `requestNewGroup` on it.

## Goal

A viewer whose current video group stops being decodable shows a picture again within about 1 s instead of at the next keyframe, up to 30 s later with the default keyframe interval.
Causes seen in the code, none reproduced: a decoder error (`videoDecoder.ts:301`), and a subgroup stream the relay resets for a slow subscriber, after which no object of the group arrives at all.

## Plan

The decoder hook covers the first cause only. The second needs the reset surfaced from `TrackReader` (it ends the stream silently) or a stall rule that a still screen share does not trip.
