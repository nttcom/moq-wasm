import asyncio
import json
from typing import Any

from moq_system_one_bot.event_timeline import EventRecord, EventTimeline

READ_TIMEOUT_SECONDS = 2


class RecordingPublication:
    """A `Publication` for tests: keeps each JSON write and whether it opened a group."""

    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self, timeout: float = READ_TIMEOUT_SECONDS) -> tuple[bool, Any]:
        return await asyncio.wait_for(self._writes.get(), timeout)


async def next_records(publication: RecordingPublication) -> list[EventRecord]:
    _, records = await publication.next_write()
    return records


async def published_timeline(*, replay: bool = False) -> tuple[EventTimeline, RecordingPublication]:
    """A timeline publishing on a `RecordingPublication`, with its first write already read."""
    timeline = EventTimeline(replay=replay)
    publication = RecordingPublication()
    await timeline.publish_on(publication)
    await publication.next_write()
    return timeline, publication
