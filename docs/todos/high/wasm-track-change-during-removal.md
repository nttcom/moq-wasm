# [S] Run replaceTrack and setVideoEncoding after a track removal in progress

Next: in `crates/wasm/js/src/media/internal/encoders/sourceEncoding.ts`, `replaceTrack` (`:243`) and `setVideoEncoding` (`:256`) run at once while the queued `removeTrack` (`:267`) awaits `stop(kind)`; put them on the same queue.

## Goal

A track change made while a removal is in progress applies after it.
Reproduced with fakes in Node: `setVideoEncoding` during the removal starts an encoder for the removed track, which runs after `setVideo(false)` resolved; a track that replaces the video during the removal is cleared when it completes (`:274`), so an unmute starts no encoder.
`setVideo(track)` called without awaiting `setVideo(false)` leaves `source.stream` without the track (`crates/wasm/js/src/media/localBroadcast.ts:380`).
