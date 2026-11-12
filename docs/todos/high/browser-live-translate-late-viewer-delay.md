# [S] Play a late live-translate viewer at the 5 s delay again

Next: give the Streamer a 2 s keyframe interval (`keyframeIntervalSeconds` in the encoding `examples/browser/examples/live-translate/streamer/main.ts` passes to `addSource`), or have the Viewer ask for a new group as `LivePlayer` does.

## Goal

`a viewer who joins after the translation started hears the original audio` (`examples/browser/tests/live-translate-e2e.spec.ts:102`) passes: the late viewer's `viewerDelayMs` is under 6 000.
On 2026-11-12 it was about 10 000 on master: the viewer subscribes from the start of the current video group, and since keyframes come every 30 s it plays behind by the group's age.

## Plan

With a 2 s keyframe interval on the Streamer all three tests of the file pass. Which object anchors the clock late is not traced.
CI does not run this suite (`docs/todos/low/ci-live-translate-browser-e2e.md`), which is why the regression was merged.
