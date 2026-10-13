# ADR: PyAV for the PTZ tracking bot

## Status
Accepted

## Date
2026-10-09

## Context
The PTZ tracking bot (`examples/python/moq-ptz-tracking`) receives an ONVIF camera's video from `onvif-ingest`
as H.264 Annex B LOC objects, one group per keyframe. djev-vision locates the target in images, not video, so the
bot has to decode the frames of each group and send a picture as a JPEG. The situation is the one the camera
detection bot was in, except that an ONVIF profile is often 1080p or larger.

## Decision
Use **PyAV** (`av`), as [the camera detection bot does](../moq-camera-detection/av.md), to decode the frames
and to encode the picture as a JPEG. The picture is scaled to 640 pixels wide on the way, so a large profile
does not inflate the djev-vision request.

## Consequences

### Positive
- Same dependency set as the camera detection bot; no system FFmpeg is needed

### Negative
- The wheel is large (tens of MB) for an example
- Every frame of the profile is decoded, also while the bot waits for djev-vision
- The `moq` library hands over payloads without LOC extension headers, so the avcC in `onvif-ingest`'s
  video config extension is out of reach; a decoder per group relies on SPS/PPS inside the keyframe, which
  `onvif-ingest` adds when it converts AVCC and which cameras sending Annex B usually repeat before each IDR

## Alternatives Considered

### Snapshots over ONVIF (`GetSnapshotUri`)
No decoder in the bot, but the bot would talk to the camera directly instead of over MoQ, and would need the
camera's credentials.

### Pillow or OpenCV, or the `ffmpeg` CLI
Rejected for the reasons in the camera detection ADR.

## References
- [PyAV](https://pyav.basswood-io.com/)
