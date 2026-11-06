# [S] Stop the live viewer E2E from failing when the player retires the outgoing rendition first

Next: read the assertion at `examples/browser/tests/live-viewer-e2e.spec.ts:173` next to `retireOutgoingVideo` in `crates/wasm/js/src/media/player/livePlayer.ts:792`.

## Goal

"live viewer plays the ingested stream, switches renditions and rewinds" passes whichever side ends the outgoing rendition. Today it asserts `${playing} ended by the publisher (status 3)`, logged on PUBLISH_DONE by `crates/wasm/js/src/media/player/trackSubscriptions.ts:150`.
At the live edge an outgoing object of the start group can arrive first; `handleLocObject` (`livePlayer.ts:832`, `acceptOutgoing(groupId) === 'retire'`) then retires it and only `unsubscribed <name>` is logged (`trackSubscriptions.ts:136`). It failed once in CI on a change to the meeting example alone, with `unsubscribed video_360p` two seconds after `subscribed .../video_480p`, and passed on rerun.

## Plan

Open: accept either log line in the test, or have the player wait for PUBLISH_DONE after asking `endBeforeGroup` (`livePlayer.ts:787`) so the test keeps checking that the relay honours it.
