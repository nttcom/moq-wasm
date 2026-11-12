# [XS] Pass `onKeyFrameNeeded` to the examples' `LocLive`

Next: add `onKeyFrameNeeded` to the callbacks at `examples/browser/examples/live-translate/viewer/translationPlayer.ts:108` and `examples/browser/examples/media/subscriber/main.ts:31`.

## Goal

The live-translate Viewer and the media subscriber do not throw when the video decoder loses a group.
`LocLive` calls `callbacks.onKeyFrameNeeded` (`crates/wasm/js/src/media/player/locLive.ts:118`), which neither page passes, so that message ends in a `TypeError`.
`npm --prefix examples/browser run lint` reports both as TS2741.

## Plan

Each page asks for a new group as `LivePlayer` does (`livePlayer.ts:228`), or passes a no-op if it should keep waiting for the next keyframe.
Open: CI does not type-check `examples/browser`, which is why this was merged; decide whether `lint.yml` should run it.
