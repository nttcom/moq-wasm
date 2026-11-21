# [XS] Say which audio track the transcriber takes

Next: in `docs/architecture/ai-bots/meeting-transcriber.md:100`, replace "the one with the highest bitrate (`pickHighestBitrateTrack`)" with the lowest-bitrate Opus LOC track that `MoqWasmParticipantInput` selects (`sdk/python/python/moq_wasm/pipecat/_participant.py:336`).

## Goal

The document says the bot takes the highest-bitrate audio track through `pickHighestBitrateTrack`, which no longer exists; the bot takes the lowest bitrate, as `Participant`'s docstring says.
Check which audio profile the Meeting's viewers take today (`LivePlayer`, since #1332) before writing why.
