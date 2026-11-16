import json

from moq_wasm import TrackWriter
from moq_wasm.media import EventTimeline


class RecordingPublication:
    def __init__(self):
        self.writes: list[tuple[bool, object]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.writes.append((new_group, json.loads(payload)))


async def recorded_timeline() -> tuple[EventTimeline, RecordingPublication]:
    """A timeline on a registered track writing to a `RecordingPublication`."""
    writer = TrackWriter(["test"], "transcript", retention_seconds=60)
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
