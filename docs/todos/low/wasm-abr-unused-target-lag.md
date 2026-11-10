# [XS] Drop or wire the unused target lag of AdaptiveBitrate

Next: remove `targetLagMs` from `crates/wasm/js/src/media/player/adaptiveBitrate.ts:147` and its tests, or pass it from `LivePlayer`.

## Goal

The code and *Automatic quality* in `docs/architecture/browser-examples/live-player.md:533` agree: the document says the meeting example gives a 250 ms target, but `LivePlayer` constructs `new AdaptiveBitrate()` without one.
