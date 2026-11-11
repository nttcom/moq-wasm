# [M] Find why the tile size E2E fails when a viewer starts on the low rendition

Next: run `Auto follows the size of a member tile when the view changes` (`examples/browser/tests/meeting-e2e.spec.ts:499`) 500 times in CI, with alice's `pause`/`play` events of each `live-player-video` and bob's player log on the console, and read a run that fails at `:490` or `:491`.

## Goal

The test passes when a member's first catalog lists `camera_low` alone. Of 2,200 runs, when the test still encoded H.264, 12 failed this way: bob's video stayed 320 wide (now `:490`), or carol's speaking ring never showed (now `:491`).
Forcing it locally (main rendition held back 1.5 s, bob's mic muted 4 s later), the step up waited 13 s, then took 2.3 s: its first `camera` group was one `camera_low` had played, and the relay held back the new-group request for 2 s (`trackSubscriptions.ts:28`). Carol's audio levels kept their voice activity. Neither failure came back in 32 runs.

## Plan

Lead for `:490`: a `<video>` the page detaches across tasks stays paused, and a splice keeps the element, so its width freezes even when the switch completes (`wasm-live-video-paused-after-container-move.md`). Check whether bob's element was paused when it failed.
Open for `:491`: whether carol's audio objects stopped, or her view lost the `onAudioLevel` listener when her catalog first named the source after `camera_low` (`sourceNames.ts:58`).
