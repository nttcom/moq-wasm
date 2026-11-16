from dataclasses import dataclass

from loguru import logger
from moq_wasm.media import EventTimeline
from pipecat.frames.frames import Frame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_meeting_transcriber.stt import UtteranceTranscriptionFrame

TRACK_NAME = "transcript"


@dataclass
class TranscriptRecord:
    started_at_ms: int
    speaker: str
    text: str

    def to_json(self) -> dict:
        return {"t": self.started_at_ms, "data": {"speaker": self.speaker, "text": self.text}}


class TranscriptWriter(FrameProcessor):
    """Appends each utterance a participant's pipeline transcribes to the room's timeline."""

    def __init__(self, timeline: EventTimeline):
        super().__init__()
        self._timeline = timeline

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, UtteranceTranscriptionFrame):
            logger.debug(f"{frame.user_id}: {frame.text}")
            await self._timeline.append(TranscriptRecord(frame.started_at_ms, frame.user_id, frame.text).to_json())
        await self.push_frame(frame, direction)

