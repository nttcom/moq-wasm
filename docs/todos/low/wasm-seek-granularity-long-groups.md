# [M] Decide how review and seek work with 30 s video groups

Next: seek in `examples/browser/examples/media` against a browser publisher and measure the time from the click to the picture.

## Goal

A seek lands on the requested time within about 1 s. A seek replays from the start of the keyframe group that holds the target (`livePlayer.ts`, "replayed from the closed keyframe group"), and the media timeline gets one record per group (`crates/broadcast/src/track_publisher.rs:206`), so 30 s groups mean up to 30 s of frames to decode and a 30 s seek grid. Not measured.

## Plan

Open: decode faster than real time, or have the publisher keep shorter groups while someone reviews.
