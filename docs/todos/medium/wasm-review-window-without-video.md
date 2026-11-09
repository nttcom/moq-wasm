# [M] Play the sound of a review window that has no video

Next: read the `frames.length === 0` branch of `fetchWindow` at `crates/wasm/js/src/media/player/reviewSession.ts:185`.

## Goal

A review through groups whose camera was off plays their sound. Read from the code, not reproduced: the media timeline lists the groups the audio starts while the video is absent, so a window of only those FETCHes no video and is taken as evicted. Review then skips its sound, and `forgetThrough` drops every earlier group, so a VOD replay cannot seek back before the gap.

## Plan

Tell a window without video from an evicted range; nothing is evicted from a broadcast that is not live. `LocReview.play` (`locReview.ts:62`) assumes at least one frame.
