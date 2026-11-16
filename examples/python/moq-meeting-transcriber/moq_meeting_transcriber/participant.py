import asyncio

from loguru import logger
from moq_wasm.codecs import OpusDecoder
from moq_wasm.media import EventTimeline, RemoteBroadcast
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.frames.frames import UserAudioRawFrame
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.processors.audio.vad_processor import VADProcessor
from pipecat.workers.runner import WorkerRunner

from moq_meeting_transcriber.audio import SAMPLE_RATE
from moq_meeting_transcriber.stt import Recognizer, UtteranceSTTService
from moq_meeting_transcriber.transcript import TranscriptWriter


class Participant:
    """Transcribes one participant of the room for as long as their catalog track lasts, each in a
    pipeline of their own so that the voice activity detector hears one speaker."""

    def __init__(self, broadcast: RemoteBroadcast, recognizer: Recognizer, timeline: EventTimeline):
        self._broadcast = broadcast
        self._name = broadcast.namespace[-1]
        self._recognizer = recognizer
        self._timeline = timeline

    async def run(self) -> None:
        audio_track: str | None = None
        transcribing: asyncio.Task | None = None
        try:
            async for catalog in self._broadcast.catalogs():
                # The lowest bitrate is the profile the Meeting's viewers take. The publisher encodes a
                # profile only while someone subscribes to it, so this starts no extra encoder in the
                # participant's browser.
                track = catalog.select(role="audio", packaging="loc", codec="opus", prefer="lowest_bitrate")
                name = track.name if track else None
                if name == audio_track:
                    continue
                if transcribing is not None:
                    transcribing.cancel()
                audio_track = name
                transcribing = asyncio.create_task(self._transcribe(name)) if name else None
                logger.info(f"{self._name}: transcribing {name}" if name else f"{self._name}: no audio track")
        finally:
            if transcribing is not None:
                transcribing.cancel()

    async def _transcribe(self, track_name: str) -> None:
        worker = PipelineWorker(
            Pipeline(
                [
                    VADProcessor(vad_analyzer=SileroVADAnalyzer()),
                    UtteranceSTTService(self._recognizer, self._name),
                    TranscriptWriter(self._timeline),
                ]
            ),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
        runner = WorkerRunner(handle_sigint=False)
        await runner.add_workers(worker)
        running = asyncio.create_task(runner.run())
        decoder = OpusDecoder(SAMPLE_RATE)
        try:
            # A muted participant's audio track ends until they unmute.
            async for audio_object in self._broadcast.follow(track_name):
                pcm = decoder.decode(audio_object.payload)
                if pcm:
                    await worker.queue_frame(
                        UserAudioRawFrame(user_id=self._name, audio=pcm, sample_rate=SAMPLE_RATE, num_channels=1)
                    )
        finally:
            await runner.cancel()
            await running
