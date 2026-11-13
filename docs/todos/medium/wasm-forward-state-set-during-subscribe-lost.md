# [S] Apply a Forward State that changed while a media SUBSCRIBE was in flight

Next: in `crates/wasm/js/src/media/player/trackSubscriptions.ts`, keep the Forward State last sent on `TrackSubscription` as `priority` is kept, and reconcile it where `subscribe()` and `handOver()` call `updatePriority(kind)`.

## Goal

A `pauseForward()` or `resumeForward()` during the round trip of a SUBSCRIBE reaches that subscription.
Today `open()` reads `forwardPaused` before it awaits, and `setForward` walks only the registered subscriptions: a track subscribed with Forward 0 stays silent after the resume, or one subscribed with Forward 1 keeps delivering through a review. Found by reading the code, not reproduced.

## Plan

Open: whether the player can reach either case. `livePlayer.ts` ends the review before it subscribes the video, which leaves audio and the rendition hand-over.
