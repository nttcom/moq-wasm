# [M] Play the sound of a review window that has no video

Next: read the `frames.length === 0` branch of `fetchWindow` at `sdk/js/js/src/media/player/reviewSession.ts:245`.

## Goal

A review through groups whose camera was off plays their sound. Read from the code, not reproduced: the media timeline lists the groups the audio starts while the video is absent, so a window of only those FETCHes no video and is taken as evicted. Review then skips its sound, and `forgetThrough` drops every earlier group, so a VOD replay cannot seek back before the gap.

The same happens when the played rendition lacks a group the timeline lists, as a `ingest-gateway` rendition did when it missed its source's keyframe: the live-srt E2E saw `FETCH` of that group answered `NoObjects`, and the seek window fell from over 10 s to 2.8 s.

A broadcast with no video track at all has no seek axis: `SeekTimeline.groups` records video groups only, so `seek`, `skip` and a VOD replay of it do nothing, and live it plays without rewind.

## Plan

Tell a window without video, or a group the rendition lacks, from an evicted range: forget only that group, not every earlier one; nothing is evicted from a broadcast that is not live. `LocReview.play` (`locReview.ts:65`) assumes at least one frame.
