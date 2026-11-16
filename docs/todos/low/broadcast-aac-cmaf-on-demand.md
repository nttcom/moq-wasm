# [S] Decide whether an on-demand source's AAC CMAF sibling is published on demand

Next: in `crates/broadcast/src/track_publisher.rs` (the `MediaEvent::AudioConfig` arm), `publish_on_demand` covers `tracks.audio` but not `audio_cmaf`; decide whether it should, as `prepare_encoded_video` does for `video_cmaf`.

## Goal

With `demand_sender` set, `audio_cmaf` is sent only while a subscriber takes it and its demand is reported, like the video's CMAF sibling; or the docs (`SourceTracks::demand_sender`, the wasm and Python `on_demand` docs) say it is not.
