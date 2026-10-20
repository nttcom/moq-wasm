import asyncio

import moqt
from loguru import logger
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.frames.frames import UserAudioRawFrame
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.processors.audio.vad_processor import VADProcessor
from pipecat.workers.runner import WorkerRunner

from moq_meeting_transcriber.audio import SAMPLE_RATE, OpusDecoder
from moq_meeting_transcriber.catalog import audio_track_name
from moq_meeting_transcriber.stt import SharedWhisper, SharedWhisperSTTService
from moq_meeting_transcriber.transcript import TranscriptTimeline, TranscriptWriter

CATALOG_TRACK = "catalog"
RESUBSCRIBE_DELAY_SECONDS = 1.0


class Participant:
    """Transcribes one participant of the room for as long as their catalog track lasts."""

    def __init__(
        self,
        session: moqt.Session,
        namespace: list[str],
        whisper: SharedWhisper,
        timeline: TranscriptTimeline,
    ):
        self._session = session
        self._namespace = namespace
        self._name = namespace[-1]
        self._whisper = whisper
        self._timeline = timeline

    async def run(self) -> None:
        # The joining FETCH brings the catalog the relay holds for the participant's other viewers.
        catalog = await self._session.subscribe(self._namespace, CATALOG_TRACK, current_group=True)
        audio_track: str | None = None
        transcribing: asyncio.Task | None = None
        try:
            async for catalog_object in catalog:
                track = audio_track_name(catalog_object.payload)
                if track == audio_track:
                    continue
                if transcribing is not None:
                    transcribing.cancel()
                audio_track = track
                transcribing = asyncio.create_task(self._transcribe(track)) if track else None
                logger.info(f"{self._name}: transcribing {track}" if track else f"{self._name}: no audio track")
        finally:
            if transcribing is not None:
                transcribing.cancel()

    async def _transcribe(self, track_name: str) -> None:
        worker = PipelineWorker(
            Pipeline(
                [
                    VADProcessor(vad_analyzer=SileroVADAnalyzer()),
                    SharedWhisperSTTService(self._whisper, self._name),
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
        try:
            while True:
                await self._forward_audio(track_name, worker)
                # A muted participant's audio track ends; it comes back on unmute.
                await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)
        finally:
            await runner.cancel()
            await running

    async def _forward_audio(self, track_name: str, worker: PipelineWorker) -> None:
        decoder = OpusDecoder()
        try:
            track = await self._session.subscribe(self._namespace, track_name)
        except RuntimeError as error:
            logger.info(f"{self._name}: cannot subscribe to {track_name}: {error}")
            return
        try:
            async for audio_object in track:
                pcm = decoder.decode(audio_object.payload)
                if pcm:
                    await worker.queue_frame(
                        UserAudioRawFrame(user_id=self._name, audio=pcm, sample_rate=SAMPLE_RATE, num_channels=1)
                    )
        except RuntimeError as error:
            logger.info(f"{self._name}: {track_name} ended: {error}")
        finally:
            # Ends the profile's encoder in the participant's browser when no one else takes it.
            try:
                await track.unsubscribe()
            except RuntimeError as error:
                logger.debug(f"{self._name}: UNSUBSCRIBE {track_name} failed: {error}")
