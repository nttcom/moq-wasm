# [S] Let the camera-detection detector ask for a keyframe

Next: subscribe the detector's `video` with `subscribe_group_start` (`crates/python/src/session.rs:177`) instead of `subscribe` in `crates/python/python/moq_wasm/media/_remote_broadcast.py:42`, then drop `keyframeIntervalSeconds: 1` from `examples/browser/examples/moq-camera-detection/main.ts:24`.

## Goal

`moq-camera-detection` publishes with the SDK's default 30 s keyframe interval. Today it keeps a 1 s interval because the detector starts a decoder per group and waits for the next keyframe.
