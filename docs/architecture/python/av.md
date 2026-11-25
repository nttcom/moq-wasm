# PyAV for the Python SDK `codecs` Extra (`python`)

**Status:** Accepted (2026-10-01)

## What

Add **PyAV** (`av`) to the `codecs` extra of the Python SDK (`moq-wasm[codecs]`, also included by `moq-wasm[pipecat]`):

- `moq_wasm.codecs`: decodes/encodes Opus, decodes H.264, H.265, VP8, VP9, and AV1, and encodes H.264 and VP8.
- `moq_system_one_bot.video`: encodes JPEG stills for System One bots.

## Context

Bots receive encoded MoQ objects—Opus packets and H.264 Annex-B groups (one group per keyframe)—from the native module, which only delivers raw payloads. Bots need decoding to inspect video frames and transcribe PCM, and encoding to speak or publish video.

## Alternatives

- **Send JPEG stills from the browser:** Eliminates decoding in the bot, but stops carrying video over MoQ.
- **Pillow or OpenCV:** Pillow cannot decode H.264; OpenCV bundles FFmpeg but is heavier and requires NumPy round trips to encode JPEGs.
- **Shell out to the `ffmpeg` CLI:** Requires system FFmpeg on every host and spawns a subprocess per stream.

## Decision

Depend on PyAV (`av`) behind the optional `codecs` extra. PyAV ships binary wheels bundling FFmpeg, so `uv sync` works out of the box on macOS and Linux.

## Consequences

- **Positive:** No system FFmpeg or extra image library required; decodes any group produced by browser WebCodecs (SPS/PPS included).
- **Negative:** Large wheel (tens of MB), kept opt-in via the `codecs` extra rather than a core dependency.
- **Negative:** Decoders must start at a group boundary, requiring publishers to begin every group with a keyframe.

## References

- [PyAV](https://pyav.basswood-io.com/)
