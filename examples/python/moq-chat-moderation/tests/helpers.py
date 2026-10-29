import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

from aiohttp import web

from moq_chat_moderation.event_timeline import EventTimeline

READ_TIMEOUT_SECONDS = 2


class RecordingPublication:
    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self) -> tuple[bool, Any]:
        return await asyncio.wait_for(self._writes.get(), READ_TIMEOUT_SECONDS)


async def published_timeline() -> tuple[EventTimeline, RecordingPublication]:
    timeline = EventTimeline()
    publication = RecordingPublication()
    await timeline.publish_on(publication)
    await publication.next_write()
    return timeline, publication


@dataclass
class StubSystemOne:
    probability: float
    texts: list[str] = field(default_factory=list)

    async def abusive_probability(self, text: str) -> float:
        self.texts.append(text)
        return self.probability


@dataclass
class FakeSystemOneServer:
    noul: float
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"answers": {"abusive": {"noul": self.noul}}})
