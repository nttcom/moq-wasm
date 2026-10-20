import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import av
from aiohttp import web

from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.prompt import Prompt

READ_TIMEOUT_SECONDS = 2


def h264_group(frame_count: int) -> list[bytes]:
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width = 64
    encoder.height = 48
    encoder.pix_fmt = "yuv420p"
    encoder.options = {"tune": "zerolatency", "profile": "baseline"}
    pictures = [av.VideoFrame(64, 48, "yuv420p") for _ in range(frame_count)]
    for pts, picture in enumerate(pictures):
        picture.pts = pts
    packets = [packet for picture in [*pictures, None] for packet in encoder.encode(picture)]
    return [bytes(packet) for packet in packets]


class RecordingPublication:
    def __init__(self):
        self._writes: asyncio.Queue[tuple[bool, Any]] = asyncio.Queue()

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self._writes.put_nowait((new_group, json.loads(payload)))

    async def next_write(self, timeout: float = READ_TIMEOUT_SECONDS) -> tuple[bool, Any]:
        return await asyncio.wait_for(self._writes.get(), timeout)


async def next_records(publication: RecordingPublication) -> list[dict[str, Any]]:
    _, records = await publication.next_write()
    return records


async def published_timeline() -> tuple[EventTimeline, RecordingPublication]:
    timeline = EventTimeline()
    publication = RecordingPublication()
    await timeline.publish_on(publication)
    await publication.next_write()
    return timeline, publication


YES_NO_PROMPT = Prompt("Is there a person?", ("yes", "no"), (7, 0))


@dataclass
class StubVision:
    answer: str | None
    images: list[bytes] = field(default_factory=list)

    async def choose(self, jpeg: bytes, prompt: Prompt) -> str | None:
        self.images.append(jpeg)
        return self.answer


@dataclass
class FakeVisionServer:
    choice: str
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"answers": {"answer": {"choice": self.choice}}})
