# ADR: PyAV for the `codecs` extra

## Status
Accepted

## Date
2026-10-01

## Context
A bot receives a participant's media as encoded objects over MoQ: Opus packets, and H.264 Annex B with one group
per keyframe. The bots judge images and transcribe PCM, so the frames have to be decoded, and a bot that speaks
or shows a picture has to encode. The native module hands over encoded payloads only.

## Decision
The `codecs` extra of the Python SDK (`moq-wasm[codecs]`, which `moq-wasm[pipecat]` includes) depends on
**PyAV** (`av`). `moq_wasm.codecs` decodes and encodes Opus, decodes H.264, H.265, VP8, VP9 and AV1, and encodes
H.264 and VP8 with it. The System One bots encode their JPEG stills with it too (`moq_system_one_bot.video`).

PyAV ships binary wheels that bundle FFmpeg, so `uv sync` is enough on macOS and Linux.

## Consequences

### Positive
- No system FFmpeg or extra image library is needed
- Decoding works on any group the browser's WebCodecs encoder produces, SPS/PPS included

### Negative
- The wheel is large (tens of MB), which is why it is an extra and not a dependency of the core
- A decoder starts at a group boundary, so a publisher has to start every group with a keyframe

## Alternatives Considered

### Send JPEG stills from the browser
No decoder in the bot at all, but the demo would no longer carry video over MoQ.

### Pillow or OpenCV
Pillow cannot decode H.264. OpenCV can, through its own FFmpeg build, but it is a heavier dependency and needs
NumPy round trips to produce a JPEG.

### Shelling out to the `ffmpeg` CLI
Requires FFmpeg on every machine and a process per stream.

## References
- [PyAV](https://pyav.basswood-io.com/)
