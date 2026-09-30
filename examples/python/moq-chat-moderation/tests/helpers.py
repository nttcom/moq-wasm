import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import moq
from aiohttp import web

READ_TIMEOUT_SECONDS = 2


class TimelineReader:
    def __init__(self, group: moq.GroupConsumer):
        self._group = group

    @property
    def group_id(self) -> int:
        return self._group.sequence

    async def next_records(self) -> list[dict[str, Any]]:
        frame = await asyncio.wait_for(self._group.read_frame(), READ_TIMEOUT_SECONDS)
        return json.loads(frame.payload)


async def open_timeline(broadcast: moq.BroadcastProducer, track_name: str) -> TimelineReader:
    track = await broadcast.consume().subscribe_track(track_name)
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    return TimelineReader(group)


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
        return web.json_response(
            {
                "model": "jev-1.13.0",
                "answers": {"abusive": {"type": "noul", "noul": self.noul}},
                "usage": {"input_tokens": 10, "output_tokens": 1},
            }
        )
