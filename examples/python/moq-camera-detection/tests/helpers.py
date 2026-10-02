import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import av
import moq
from aiohttp import web

from moq_camera_detection.djev_status import DjevStatus
from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.prompt import Prompt

READ_TIMEOUT_SECONDS = 2
TRACK_NAME = "eventtimeline"
STATUS_TRACK_NAME = "status"


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
    content: str
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Authorization"), await request.json()))
        return web.json_response({"choices": [{"message": {"content": self.content}}]})


@dataclass
class FakeMetadataServer:
    url: str = ""
    requests: list[tuple[str | None, str | None]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Metadata-Flavor"), request.query.get("audience")))
        return web.Response(text="metadata-token")


async def published_status(status: DjevStatus) -> moq.TrackConsumer:
    broadcast = moq.BroadcastProducer()
    status.publish_on(broadcast)
    return await broadcast.consume().subscribe_track(STATUS_TRACK_NAME)


async def next_status(track: moq.TrackConsumer) -> str:
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    frame = await asyncio.wait_for(group.read_frame(), READ_TIMEOUT_SECONDS)
    return json.loads(frame.payload)["djev"]
