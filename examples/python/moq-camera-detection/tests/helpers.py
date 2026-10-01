import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import av
import moq
from aiohttp import web

from moq_camera_detection.event_timeline import EventTimeline

READ_TIMEOUT_SECONDS = 2
TRACK_NAME = "eventtimeline"


def h264_keyframe() -> bytes:
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width = 64
    encoder.height = 48
    encoder.pix_fmt = "yuv420p"
    picture = av.VideoFrame(64, 48, "yuv420p")
    return b"".join(bytes(packet) for packet in [*encoder.encode(picture), *encoder.encode(None)])


async def next_records(group: moq.GroupConsumer) -> list[dict[str, Any]]:
    frame = await asyncio.wait_for(group.read_frame(), READ_TIMEOUT_SECONDS)
    return json.loads(frame.payload)


async def published_timeline() -> tuple[EventTimeline, moq.GroupConsumer]:
    broadcast = moq.BroadcastProducer()
    timeline = EventTimeline(broadcast, TRACK_NAME)
    track = await broadcast.consume().subscribe_track(TRACK_NAME)
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    await next_records(group)
    return timeline, group


@dataclass
class StubVision:
    person: bool | None
    images: list[bytes] = field(default_factory=list)

    async def has_person(self, jpeg: bytes) -> bool | None:
        self.images.append(jpeg)
        return self.person


@dataclass
class FakeVisionServer:
    content: str
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"choices": [{"message": {"content": self.content}}]})
