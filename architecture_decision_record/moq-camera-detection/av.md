# ADR: PyAV for the camera detection bot

## Status
Accepted

## Date
2026-10-01

## Context
The camera detection bot (`examples/python/moq-camera-detection`) receives the browser's camera as H.264 Annex B
objects over MoQ: one group per keyframe. djev-vision judges images, not video, so the bot has to decode the
frames of each group and send a picture every 0.1 s as a JPEG. pipecat's `MOQTransport` does not receive video, and the `moq` library
hands over encoded frames only, so neither decodes H.264.

## Decision
Use **PyAV** (`av`) to decode the frames with FFmpeg's H.264 decoder and to encode the picture with its MJPEG
encoder.

PyAV ships binary wheels that bundle FFmpeg, so `uv sync` is enough on macOS and Linux. One library covers both
the decode and the JPEG encode, and its libx264 encoder lets the tests build a real Annex B group.

## Consequences

### Positive
- No system FFmpeg or extra image library is needed
- Decoding works on any group the browser's WebCodecs encoder produces, SPS/PPS included

### Negative
- The wheel is large (tens of MB) for an example
- A decoder starts at a group boundary, so the browser has to start every group with a keyframe

## Alternatives Considered

### Send JPEG stills from the browser
No decoder in the bot at all, but the demo would no longer carry video over MoQ. The user chose H.264.

### Pillow or OpenCV
Pillow cannot decode H.264. OpenCV can, through its own FFmpeg build, but it is a heavier dependency and needs
NumPy round trips to produce a JPEG.

### Shelling out to the `ffmpeg` CLI
Requires FFmpeg on every machine and a process per camera stream.

## References
- [PyAV](https://pyav.basswood-io.com/)
