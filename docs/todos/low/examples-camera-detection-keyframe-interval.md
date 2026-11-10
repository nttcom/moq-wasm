# [S] Let the camera-detection detector ask for a keyframe

Next: read how the detector bot subscribes to `video`, starting from `examples/browser/examples/moq-camera-detection/main.ts:25`.

## Goal

`moq-camera-detection` publishes with the SDK's default 30 s keyframe interval. It keeps `KEYFRAME_INTERVAL_FRAMES = 30` (1 s) because the detector starts a decoder per group and waits for the next keyframe.

## Plan

`crates/python` has no `subscribe_group_start`; add it there first (`crates/python/src/track.rs:44`).
