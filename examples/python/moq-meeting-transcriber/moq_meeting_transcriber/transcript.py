import json
from dataclasses import dataclass
from typing import Protocol

from loguru import logger
from pipecat.frames.frames import Frame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_meeting_transcriber.stt import UtteranceTranscriptionFrame

TRACK_NAME = "transcript"


class Publication(Protocol):
    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...


@dataclass
class TranscriptRecord:
    started_at_ms: int
    speaker: str
    text: str

    def to_json(self) -> dict:
        return {"t": self.started_at_ms, "data": {"speaker": self.speaker, "text": self.text}}


class TranscriptTimeline:
    """draft-ietf-moq-msf-01 §8 event timeline indexed by wallclock time, published as one group."""

    def __init__(self, publication: Publication):
        self._publication = publication

    async def open(self) -> None:
        await self._publication.write(_encode([]), new_group=True)

    async def append(self, record: TranscriptRecord) -> None:
        await self._publication.write(_encode([record.to_json()]))


class TranscriptWriter(FrameProcessor):
    """Appends each utterance a participant's pipeline transcribes to the room's timeline."""

    def __init__(self, timeline: TranscriptTimeline):
        super().__init__()
        self._timeline = timeline

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, UtteranceTranscriptionFrame):
            logger.debug(f"{frame.user_id}: {frame.text}")
            await self._timeline.append(TranscriptRecord(frame.started_at_ms, frame.user_id, frame.text))
        await self.push_frame(frame, direction)


def _encode(records: list[dict]) -> bytes:
    return json.dumps(records, separators=(",", ":"), ensure_ascii=False).encode()
