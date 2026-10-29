import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import av
from aiohttp import web

from moq_ptz_tracking.event_timeline import EventTimeline
from moq_ptz_tracking.ptz import PtzCommands
from moq_ptz_tracking.target import LatestTarget, Target
from moq_ptz_tracking.vision import COLUMNS, ROWS, Answer, Position, cell_key

READ_TIMEOUT_SECONDS = 2
# next_group ends a group after this long without a write, so a test's turns must be shorter.
MOVE_END_WAIT_SECONDS = 0.3

MUG_TARGET = Target("a red mug", "video/profile_1", (7, 0))


def h264_group(frame_count: int, width: int = 64, height: int = 48) -> list[bytes]:
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width = width
    encoder.height = height
    encoder.pix_fmt = "yuv420p"
    encoder.options = {"tune": "zerolatency", "profile": "baseline"}
    pictures = [av.VideoFrame(width, height, "yuv420p") for _ in range(frame_count)]
    for pts, picture in enumerate(pictures):
        picture.pts = pts
    packets = [packet for picture in [*pictures, None] for packet in encoder.encode(picture)]
    return [bytes(packet) for packet in packets]


class RecordingPublication:
    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()
        self._held: tuple[bool, Any] | None = None

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self, timeout: float = READ_TIMEOUT_SECONDS) -> tuple[bool, Any]:
        if self._held is not None:
            held, self._held = self._held, None
            return held
        return await asyncio.wait_for(self._writes.get(), timeout)

    async def next_group(self, timeout: float = READ_TIMEOUT_SECONDS) -> list[Any]:
        """The objects of the next group: from a write that opens one until the next such write, or
        until MOVE_END_WAIT_SECONDS pass without a write."""
        new_group, first = await self.next_write(timeout)
        assert new_group, f"{first} does not open a group"
        objects = [first]
        while True:
            try:
                new_group, written = await self.next_write(MOVE_END_WAIT_SECONDS)
            except TimeoutError:
                return objects
            if new_group:
                self._held = (new_group, written)
                return objects
            objects.append(written)


async def next_records(publication: RecordingPublication) -> list[dict[str, Any]]:
    _, records = await publication.next_write()
    return records


async def published_timeline() -> tuple[EventTimeline, RecordingPublication]:
    timeline = EventTimeline()
    publication = RecordingPublication()
    await timeline.publish_on(publication)
    await publication.next_write()
    return timeline, publication


async def published_commands() -> tuple[PtzCommands, RecordingPublication]:
    commands = PtzCommands()
    publication = RecordingPublication()
    await commands.publish_on(publication)
    return commands, publication


def tracking(target: Target) -> LatestTarget:
    latest_target = LatestTarget()
    latest_target.set(target)
    return latest_target


@dataclass
class StubVision:
    answer: Position | Answer
    images: list[bytes] = field(default_factory=list)

    async def locate(self, jpeg: bytes, description: str) -> Position | Answer:
        self.images.append(jpeg)
        return self.answer


def grid_probabilities(given: dict[str, float]) -> dict[str, float]:
    keys = [cell_key(row, column) for row in range(len(ROWS)) for column in range(len(COLUMNS))]
    return {key: given.get(key, 0.0) for key in keys}


@dataclass
class FakeVisionServer:
    answers: dict[str, Any]
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"model": "d1-2B", "answers": self.answers})
