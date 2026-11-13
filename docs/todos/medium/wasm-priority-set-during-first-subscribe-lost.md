# [XS] Send a video priority that was set while the first SUBSCRIBE was in flight

Next: in `crates/wasm/js/src/media/player/trackSubscriptions.ts:79`, call `this.updatePriority(kind)` after `this.subscriptions.set(...)` in `subscribe()`, with a test like the hand-over one in `trackSubscriptions.test.ts`.

## Goal

`LivePlayer.setVideoPriority` called before the first video SUBSCRIBE is answered reaches the relay.
Today `open()` reads the priority before it awaits the SUBSCRIBE, and `sendPriority` finds no subscription to update, so the track keeps the old value until the next change. Found by reading the code, not reproduced.

## Plan

`handOver()` already re-sends with `updatePriority(kind)` for the same reason; `subscribe()` does not.
Remote Monitoring hits it when the main camera changes within the round trip of a preview camera's first SUBSCRIBE.
