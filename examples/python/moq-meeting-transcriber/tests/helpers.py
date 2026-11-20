import asyncio
import json
from typing import cast

from moq_wasm import Session, TrackWriter
from moq_wasm.media import EventTimeline
from pipecat.frames.frames import (
    Frame,
    StartFrame,
    UserAudioRawFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.processors.filters.identity_filter import IdentityFilter
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_meeting_transcriber.audio import SAMPLE_RATE


class RecordingPublication:
    def __init__(self):
        self.writes: list[tuple[bool, object]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.writes.append((new_group, json.loads(payload)))


async def recorded_timeline() -> tuple[EventTimeline, RecordingPublication]:
    """A timeline on a registered track writing to a `RecordingPublication`."""
    # The writer reaches its session only to close the track or wait for a subscriber, which
    # these tests do not.
    writer = TrackWriter(cast(Session, None), ["test"], "transcript", retention_seconds=60)
    timeline = EventTimeline(writer)
    publication = RecordingPublication()
    await writer._bind(publication)
    return timeline, publication


class NamedRecognizer:
    def __init__(self, name: str):
        self.name = name

    async def ready(self) -> None:
        pass


class SessionRefusingNamespaces:
    async def watch_namespaces(self, prefix: list[str]):
        raise RuntimeError("SUBSCRIBE_NAMESPACE refused")


class HeldRecognizer(NamedRecognizer):
    """Answers each utterance with `text` once `release` is set; `called` is set once it has one."""

    def __init__(self, text: str):
        super().__init__("whisper")
        self.text = text
        self.called = asyncio.Event()
        self.release = asyncio.Event()

    async def recognize(self, pcm: bytes) -> str:
        self.called.set()
        await self.release.wait()
        return self.text


class OneUtteranceInput(FrameProcessor):
    """Stands in for `MoqWasmParticipantInput` and the VAD after it: once the pipeline starts, the
    participant says one second of audio, framed by the VAD's start and stop."""

    def __init__(self, broadcast, params):
        super().__init__()
        self._register_event_handler("on_track_subscribed")

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        await self.push_frame(frame, direction)
        if isinstance(frame, StartFrame):
            await self.push_frame(VADUserStartedSpeakingFrame())
            await self.push_frame(
                UserAudioRawFrame(
                    user_id="alice", audio=bytes(2 * SAMPLE_RATE), sample_rate=SAMPLE_RATE, num_channels=1
                )
            )
            await self.push_frame(VADUserStoppedSpeakingFrame())


def passing_vad(vad_analyzer) -> FrameProcessor:
    """Stands in for `VADProcessor`: `OneUtteranceInput` frames the speech itself."""
    return IdentityFilter()
