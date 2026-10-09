import asyncio
import json
from dataclasses import dataclass, field
from typing import Any

import av
import moq
from aiohttp import web

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.event_timeline import EventTimeline
from moq_ptz_tracking.ptz import PtzCommands
from moq_ptz_tracking.target import LatestTarget, Target

READ_TIMEOUT_SECONDS = 2
TRACK_NAME = "eventtimeline"
STATUS_TRACK_NAME = "status"
COMMAND_TRACK_NAME = "command"

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


async def published_commands() -> tuple[PtzCommands, moq.TrackConsumer]:
    broadcast = moq.BroadcastProducer()
    commands = PtzCommands(broadcast, COMMAND_TRACK_NAME)
    return commands, await broadcast.consume().subscribe_track(COMMAND_TRACK_NAME)


async def next_command(track: moq.TrackConsumer) -> dict[str, Any]:
    group = await asyncio.wait_for(track.next_group(), READ_TIMEOUT_SECONDS)
    frame = await asyncio.wait_for(group.read_frame(), READ_TIMEOUT_SECONDS)
    return json.loads(frame.payload)


def tracking(target: Target) -> LatestTarget:
    latest_target = LatestTarget()
    latest_target.set(target)
    return latest_target


@dataclass
class StubVision:
    position: str | None
    images: list[bytes] = field(default_factory=list)

    async def locate(self, jpeg: bytes, description: str) -> str | None:
        self.images.append(jpeg)
        return self.position


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
