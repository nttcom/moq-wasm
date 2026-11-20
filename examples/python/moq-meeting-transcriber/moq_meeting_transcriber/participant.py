import asyncio

from loguru import logger
from moq_wasm.media import EventTimeline, RemoteBroadcast
from moq_wasm.pipecat import MoqWasmInputParams, MoqWasmParticipantInput, MoqWasmTrack
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineParams, PipelineWorker
from pipecat.processors.audio.vad_processor import VADProcessor
from pipecat.workers.runner import WorkerRunner

from moq_meeting_transcriber.audio import SAMPLE_RATE
from moq_meeting_transcriber.stt import Recognizer, UtteranceSTTService
from moq_meeting_transcriber.transcript import TranscriptWriter


class Participant:
    """Transcribes one participant of the room, in a pipeline of their own so that the voice
    activity detector hears one speaker. `MoqWasmParticipantInput` takes the Opus LOC
    audio track with the lowest bitrate, the profile the Meeting's viewers take; the publisher
    encodes a profile only while someone subscribes to it, so this starts no extra encoder in the
    participant's browser."""

    def __init__(self, broadcast: RemoteBroadcast, recognizer: Recognizer, timeline: EventTimeline):
        self._broadcast = broadcast
        self._name = broadcast.namespace[-1]
        self._recognizer = recognizer
        self._timeline = timeline

    async def run(self, stopping: asyncio.Event) -> None:
        """Runs until cancelled, as when the participant leaves; it then transcribes the utterances
        it already heard, unless `stopping` is set, which drops them."""
        audio = MoqWasmParticipantInput(self._broadcast, MoqWasmInputParams(audio_in_enabled=True))

        @audio.event_handler("on_track_subscribed")
        async def on_track_subscribed(_, track: MoqWasmTrack):
            logger.info(f"{self._name}: transcribing {track.name}")

        worker = PipelineWorker(
            Pipeline(
                [
                    audio,
                    VADProcessor(vad_analyzer=SileroVADAnalyzer()),
                    UtteranceSTTService(self._recognizer, self._name, stopping),
                    TranscriptWriter(self._timeline),
                ]
            ),
            # The input decodes at the pipeline's rate, which Silero starts at too.
            params=PipelineParams(audio_in_sample_rate=SAMPLE_RATE),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
        runner = WorkerRunner(handle_sigint=False)
        await runner.add_workers(worker)
        running = asyncio.create_task(runner.run())
        try:
            await asyncio.shield(running)
        except asyncio.CancelledError:
            await worker.stop_when_done()
            await running
            raise
