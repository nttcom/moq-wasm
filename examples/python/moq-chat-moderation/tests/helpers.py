import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import moq
from aiohttp import web

from moq_chat_moderation.event_timeline import EventTimeline

READ_TIMEOUT_SECONDS = 2
TRACK_NAME = "eventtimeline"


class TimelineReader:
    def __init__(self, group: moq.GroupConsumer):
        self._group = group

    @property
    def group_id(self) -> int:
        return self._group.sequence

    async def next_records(self) -> list[dict[str, Any]]:
        frame = await asyncio.wait_for(self._group.read_frame(), READ_TIMEOUT_SECONDS)
        return json.loads(frame.payload)


async def open_timeline(broadcast: moq.BroadcastProducer) -> TimelineReader:
    track = await broadcast.consume().subscribe_track(TRACK_NAME)
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    return TimelineReader(group)


async def published_timeline() -> tuple[EventTimeline, TimelineReader]:
    timeline = EventTimeline(TRACK_NAME)
    broadcast = moq.BroadcastProducer()
    timeline.publish_on(broadcast)
    reader = await open_timeline(broadcast)
    await reader.next_records()
    return timeline, reader


@dataclass
class StubJev:
    probability: float
    texts: list[str] = field(default_factory=list)

    async def abusive_probability(self, text: str) -> float:
        self.texts.append(text)
        return self.probability


@dataclass
class FakeJevServer:
    noul: float
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"answers": {"abusive": {"noul": self.noul}}})
