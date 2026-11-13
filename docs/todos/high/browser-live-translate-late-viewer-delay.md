# [S] Play a late live-translate viewer at the 5 s delay again

Next: subscribe the Viewer's video with `session.subscribeGroupStart` instead of `subscribeCurrentGroup` in `examples/browser/examples/live-translate/viewer/translationPlayer.ts` (`subscribe`), as `LivePlayer` does through `TrackSubscriptions.open`.

## Goal

`a viewer who joins after the translation started hears the original audio` (`examples/browser/tests/live-translate-e2e.spec.ts:102`) passes: the late viewer's `viewerDelayMs` is under 6 000.
On 2026-11-12 it was about 10 000 on master: the viewer reads the current video group from its start, and since keyframes come every 30 s it plays behind by the group's age.

## Plan

`subscribeGroupStart` asks for a new group (NEW_GROUP_REQUEST) when the open one is long, which is what the 30 s interval relies on; its limit is `FETCHED_GROUP_OBJECTS` in `trackSubscriptions.ts`. Not tried yet.
A 2 s keyframe interval on the Streamer makes all three tests pass, which confirms the cause but gives up the longer groups.
CI does not run this suite (`docs/todos/low/ci-live-translate-browser-e2e.md`), which is why the regression was merged.
