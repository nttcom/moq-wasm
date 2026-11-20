import asyncio
import json
from types import SimpleNamespace
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

from moq_meeting_transcriber import participant
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


class NamespaceEvents:
    """A session whose SUBSCRIBE_NAMESPACE yields the namespaces `publish` and `withdraw` announce."""

    def __init__(self):
        self._events: asyncio.Queue[SimpleNamespace] = asyncio.Queue()

    def publish(self, namespace: list[str]) -> None:
        self._events.put_nowait(SimpleNamespace(namespace=namespace, active=True))

    def withdraw(self, namespace: list[str]) -> None:
        self._events.put_nowait(SimpleNamespace(namespace=namespace, active=False))

    async def watch_namespaces(self, prefix: list[str]):
        return self

    def __aiter__(self):
        return self

    async def __anext__(self) -> SimpleNamespace:
        return await self._events.get()

    async def close(self) -> None:
        pass

    def _publishes(self, namespace: list[str]) -> bool:
        return False


class HeldRecognizer(NamedRecognizer):
    """Answers its calls with `texts` in order once `release` is set; `called` is set once it has one,
    and `calls` counts them."""

    def __init__(self, *texts: str):
        super().__init__("whisper")
        self.texts = list(texts)
        self.called = asyncio.Event()
        self.calls = 0
        self.release = asyncio.Event()

    async def recognize(self, pcm: bytes) -> str:
        self.calls += 1
        self.called.set()
        await self.release.wait()
        return self.texts.pop(0)


class UtterancesInput(FrameProcessor):
    """Stands in for `MoqWasmParticipantInput` and the VAD after it: once the pipeline starts, the
    participant says `count` utterances of one second, each framed by the VAD's start and stop."""

    count = 1

    def __init__(self, broadcast, params):
        super().__init__()
        self._register_event_handler("on_track_subscribed")

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        await self.push_frame(frame, direction)
        if isinstance(frame, StartFrame):
            for _ in range(self.count):
                await self.push_frame(VADUserStartedSpeakingFrame())
                await self.push_frame(
                    UserAudioRawFrame(
                        user_id="alice", audio=bytes(2 * SAMPLE_RATE), sample_rate=SAMPLE_RATE, num_channels=1
                    )
                )
                await self.push_frame(VADUserStoppedSpeakingFrame())


def passing_vad(vad_analyzer) -> FrameProcessor:
    """Stands in for `VADProcessor`: `UtterancesInput` frames the speech itself."""
    return IdentityFilter()


def hear_utterances(monkeypatch, count: int = 1) -> None:
    """Makes each participant's pipeline hear `count` utterances, framed by `UtterancesInput`."""
    monkeypatch.setattr(participant, "MoqWasmParticipantInput", UtterancesInput)
    monkeypatch.setattr(UtterancesInput, "count", count)
    monkeypatch.setattr(participant, "VADProcessor", passing_vad)


async def until_transcribed(transcript: RecordingPublication, count: int) -> None:
    """Returns once `transcript` holds `count` texts; raises TimeoutError after 5 s."""
    async with asyncio.timeout(5):
        while len(texts(transcript)) < count:
            await asyncio.sleep(0.01)


async def called_within(recognizer: HeldRecognizer, calls: int, seconds: float) -> bool:
    """Whether `recognizer` has had `calls` calls within `seconds`."""
    try:
        async with asyncio.timeout(seconds):
            while recognizer.calls < calls:
                await asyncio.sleep(0.01)
    except TimeoutError:
        return False
    return True


def texts(transcript: RecordingPublication) -> list[str]:
    return [record["data"]["text"] for _, records in transcript.writes for record in records]
