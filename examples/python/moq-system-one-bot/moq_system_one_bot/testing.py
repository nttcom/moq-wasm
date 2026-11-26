import asyncio
import json
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any

from aiohttp import web
from aiohttp.test_utils import TestServer
from moq_wasm.pipecat import MoqWasmEventRecord
from pipecat.frames.frames import OutputTransportMessageFrame

READ_TIMEOUT_SECONDS = 2


class RecordingPublication:
    """A `moq_wasm.TrackWriter` for tests: keeps each JSON write and whether it opened a group."""

    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self, timeout: float = READ_TIMEOUT_SECONDS) -> tuple[bool, Any]:
        return await asyncio.wait_for(self._writes.get(), timeout)


def records(frames) -> list[dict]:
    return [
        frame.message.record
        for frame in frames
        if isinstance(frame, OutputTransportMessageFrame) and isinstance(frame.message, MoqWasmEventRecord)
    ]


@dataclass
class FakeSystemOneServer:
    """`requests` keeps each request's Authorization header and JSON body."""

    answers: dict[str, Any]
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"answers": self.answers})


@asynccontextmanager
async def fake_system_one(answers: dict[str, Any]) -> AsyncIterator[FakeSystemOneServer]:
    server = FakeSystemOneServer(answers)
    app = web.Application()
    app.router.add_post("/v1/systemone", server.handle)
    async with TestServer(app) as test_server:
        server.url = str(test_server.make_url("/v1/systemone"))
        yield server
