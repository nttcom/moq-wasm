# [S] Keep the live-viewer rewind E2E from failing right after an up-switch

Next: in `examples/browser/tests/live-viewer-e2e.spec.ts:52`, read what the expected width depends on: the live `<video>`'s `videoWidth`, taken once before the poll.

## Goal

`live viewer plays the ingested stream, switches renditions and rewinds` (with `--transcode`) passes every run. It failed once with `Expected: 640, Received: 854`: Auto subscribed `video_480p` 0.4 s before the Home press, the splice had handed over, so `abandonVideoSwitch` (`crates/wasm/js/src/media/player/livePlayer.ts:805`) kept 480p for the review, while the live element still showed its last 360p frame. The rerun passed.

## Plan

Open: whether the test should compare against the rendition the review plays, or the player should not start a review before the incoming rendition has shown a frame.
