import json
import time
from dataclasses import dataclass
from typing import Protocol

from loguru import logger
from pipecat.frames.frames import Frame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_meeting_transcriber.stt import UtteranceTranscriptionFrame

TRACK_NAME = "transcript"
# A subscription that starts mid-group receives objects from the next group on, which opens with
# every record so far. The first record this long after a group opened starts the next group.
GROUP_SECONDS = 10


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
    """draft-ietf-moq-msf-01 §8 event timeline indexed by wallclock time: each group opens with
    every record so far, and later objects in the group carry one new record each."""

    def __init__(self, publication: Publication, now=time.time):
        self._publication = publication
        self._now = now
        self._records: list[dict] = []
        self._group_opened_at: float | None = None

    async def open(self) -> None:
        self._group_opened_at = self._now()
        await self._publication.write(_encode(self._records), new_group=True)

    async def append(self, record: TranscriptRecord) -> None:
        self._records.append(record.to_json())
        if self._group_opened_at is None or self._now() - self._group_opened_at >= GROUP_SECONDS:
            await self.open()
        else:
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
