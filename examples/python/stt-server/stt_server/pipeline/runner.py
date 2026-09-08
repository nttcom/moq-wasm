import asyncio
import logging

from .base import (
    EventSink,
    LanguageModel,
    PIPELINE_PCM,
    ReplyAudioEvent,
    SpeechToText,
    TextEvent,
    TextToSpeech,
    VoiceActivityDetector,
)

log = logging.getLogger("pipeline")


class VoicePipeline:
    """VAD → STT → LLM → TTS for one audio track. `feed()` only runs the VAD;
    utterances are processed by a worker task so the track reader never waits
    for a model or a remote service. LLM and TTS are optional stages."""

    def __init__(
        self,
        vad: VoiceActivityDetector,
        stt: SpeechToText,
        llm: LanguageModel | None,
        tts: TextToSpeech | None,
        sink: EventSink,
    ) -> None:
        self.vad = vad
        self.stt = stt
        self.llm = llm
        self.tts = tts
        self.sink = sink
        self._utterances: asyncio.Queue[bytes | None] = asyncio.Queue()
        self._worker: asyncio.Task | None = None

    async def start(self) -> None:
        self._worker = asyncio.ensure_future(self._run_worker())

    async def feed(self, pcm: bytes) -> None:
        for utterance in self.vad.push(pcm):
            self._utterances.put_nowait(utterance)

    async def close(self) -> None:
        remaining = self.vad.flush()
        if remaining is not None:
            self._utterances.put_nowait(remaining)
        self._utterances.put_nowait(None)
        if self._worker is not None:
            await self._worker
            self._worker = None

    async def _run_worker(self) -> None:
        while (utterance := await self._utterances.get()) is not None:
            try:
                await self._process(utterance)
            except Exception as error:
                log.exception("pipeline stage failed: %s", error)

    async def _process(self, utterance: bytes) -> None:
        text = (await self.stt.transcribe(utterance, PIPELINE_PCM)).strip()
        if not text:
            return
        await self.sink(TextEvent("transcript", text))
        if self.llm is None:
            return
        reply = (await self.llm.reply(text)).strip()
        if not reply:
            return
        await self.sink(TextEvent("reply", reply))
        if self.tts is None:
            return
        await self.sink(ReplyAudioEvent(speech=await self.tts.synthesize(reply)))
