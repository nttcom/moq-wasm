# [XS] Take empty PCM in OpusEncoder.encode

Next: return `[]` for empty `pcm` before the rate check in `sdk/python/python/moq_wasm/codecs.py:66` (`OpusEncoder.encode`).

## Goal

`OpusEncoder().encode(b"", 16000)` returns no packets; today the resampler's filter push raises `av.error.MemoryError` (also on a fresh encoder).
An empty `OutputAudioRawFrame` then raises out of `MoqWasmOutputTransport.write_audio_frame` (`sdk/python/python/moq_wasm/pipecat/_transport.py:252`).

## Plan

Test: an empty frame at 24 kHz between two 16 kHz frames through `MoqWasmTransport.output()`: no error, all of the speech written.
