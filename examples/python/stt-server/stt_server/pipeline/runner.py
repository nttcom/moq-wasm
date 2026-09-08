import asyncio
import logging
import time

from .base import (
    EventSink,
    LanguageModel,
    PIPELINE_SAMPLE_RATE,
    ReplyAudio,
    SpeechToText,
    Stage,
    TextToSpeech,
    TurnEvent,
    VoiceActivityDetector,
)

log = logging.getLogger("pipeline")


class VoicePipeline:
    """VAD → STT → LLM → TTS for one audio track. `feed()` only runs the VAD;
    utterances are processed by a worker task started here, so the track
    reader never waits for a model or a remote service. Each utterance is one
    conversation turn, numbered from 1, and every stage reports its start and
    its duration through the sink. LLM and TTS are optional stages."""

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
        self.turns = 0
        self._marker: dict | None = None
        self._utterances: asyncio.Queue[tuple[bytes, dict | None] | None] = asyncio.Queue()
        self._worker = asyncio.ensure_future(self._run_worker())

    async def feed(self, pcm: bytes, marker: dict | None = None) -> None:
        """`marker` describes where this audio came from (for MoQT, the object's
        location); the utterance it completes carries the last one seen."""
        if marker is not None:
            self._marker = marker
        for utterance in self.vad.push(pcm):
            self._utterances.put_nowait((utterance, self._marker))

    async def close(self) -> None:
        remaining = self.vad.flush()
        if remaining is not None:
            self._utterances.put_nowait((remaining, self._marker))
        self._utterances.put_nowait(None)
        await self._worker

    async def _run_worker(self) -> None:
        while (item := await self._utterances.get()) is not None:
            utterance, marker = item
            self.turns += 1
            try:
                await self._run_turn(self.turns, utterance, marker)
            except Exception as error:
                log.exception("turn %d failed: %s", self.turns, error)

    async def _run_turn(self, turn: int, utterance: bytes, marker: dict | None) -> None:
        started = time.monotonic()
        utterance_sec = len(utterance) / (PIPELINE_SAMPLE_RATE * 2)
        await self.sink(
            TurnEvent(
                turn=turn,
                stage="vad",
                state="done",
                elapsed_ms=utterance_sec * 1000,
                detail={"utterance_sec": round(utterance_sec, 3), "audio": marker},
            )
        )
        text = await self._stage(turn, "stt", self.stt.transcribe(utterance))
        if not text:
            return await self._finish(turn, started)
        if self.llm is None:
            return await self._finish(turn, started)
        reply = await self._stage(turn, "llm", self.llm.reply(text))
        if not reply or self.tts is None:
            return await self._finish(turn, started)
        await self.sink(TurnEvent(turn=turn, stage="tts", state="start"))
        tts_started = time.monotonic()
        speech = await self.tts.synthesize(reply)
        await self.sink(ReplyAudio(turn=turn, speech=speech))
        await self.sink(
            TurnEvent(
                turn=turn,
                stage="tts",
                state="done",
                elapsed_ms=(time.monotonic() - tts_started) * 1000,
                detail={"speech_sec": round(len(speech.pcm) / (speech.sample_rate * 2), 3)},
            )
        )
        await self._finish(turn, started)

    async def _stage(self, turn: int, stage: Stage, work) -> str:
        await self.sink(TurnEvent(turn=turn, stage=stage, state="start"))
        started = time.monotonic()
        try:
            text = (await work).strip()
        except Exception as error:
            await self.sink(
                TurnEvent(
                    turn=turn,
                    stage=stage,
                    state="failed",
                    elapsed_ms=(time.monotonic() - started) * 1000,
                    text=str(error),
                )
            )
            raise
        await self.sink(
            TurnEvent(
                turn=turn,
                stage=stage,
                state="done",
                elapsed_ms=(time.monotonic() - started) * 1000,
                text=text,
            )
        )
        return text

    async def _finish(self, turn: int, started: float) -> None:
        await self.sink(
            TurnEvent(
                turn=turn,
                stage="turn",
                state="done",
                elapsed_ms=(time.monotonic() - started) * 1000,
            )
        )
