# [XS] Take empty PCM in OpusEncoder.encode

Next: return no samples for empty `pcm` before the rate check in `sdk/python/python/moq_wasm/codecs.py:141` (`_PcmResampler.resample`, which `OpusEncoder.encode` calls).

## Goal

`OpusEncoder().encode(b"", 16000)` returns no packets; today the resampler's filter push raises `av.error.MemoryError` (also on a fresh encoder).
An empty `OutputAudioRawFrame` then fails `MoqWasmOutputTransport.write_audio_frame` (`sdk/python/python/moq_wasm/pipecat/_transport.py:765`), which pushes a `cannot write audio` `ErrorFrame` upstream and returns False.

## Plan

Test: an empty frame at 24 kHz between two 16 kHz frames through `MoqWasmTransport.output()`: no error, all of the speech written.
