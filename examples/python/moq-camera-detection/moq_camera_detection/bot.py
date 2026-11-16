import argparse
import asyncio
import time
from collections.abc import AsyncIterator
from contextlib import aclosing
from functools import partial

import aiohttp
import moq_wasm
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken
from moq_bot_rooms.rooms import GRACE_SECONDS, BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.identity_token import add_identity_token_arguments, identity_token_from_arguments
from moq_system_one_bot.relay import add_relay_arguments, stay_connected
from moq_system_one_bot.video import picture_to_jpeg
from moq_wasm.codecs import VideoDecoder
from moq_wasm.media import CatalogTrack, RemoteBroadcast, create_broadcast, watch_broadcasts
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.catalog import camera_video_track
from moq_camera_detection.detector import CameraDetector, CameraPicture
from moq_camera_detection.prompt import Prompt, parse_prompt
from moq_camera_detection.vision import SystemOneVisionClient

DEFAULT_PORT = 8098
# system-one-server answers one request at a time, and one camera already sends it a picture every
# SAMPLE_INTERVAL_SECONDS.
DEFAULT_MAX_ROOMS = 1
DEMO_NAME = "moq-camera-detection"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# The event timeline and the System One status are kept for FETCH as long as a room runs by default
# (--max-room-seconds).
RETENTION_SECONDS = 60 * 60
RESUBSCRIBE_DELAY_SECONDS = 1.0
SAMPLE_INTERVAL_SECONDS = 0.1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Answer the MoQ camera page's question about its camera, one bot per camera room"
    )
    add_serve_arguments(
        parser, bot="moq-camera-detection", default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS
    )
    add_relay_arguments(parser)
    parser.add_argument(
        "--system-one-url", required=True, help="a System One /v1/systemone endpoint that reads images, such as system-one-server"
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


def room_namespace(room: BotRoom) -> list[str]:
    return [room.namespace_root, DEMO_NAME, room.room_id]


class LatestPrompt:
    def __init__(self):
        self.prompt: Prompt | None = None

    async def follow(self, prompts: AsyncIterator[moq_wasm.Object]):
        async for prompt_object in prompts:
            location = (prompt_object.group_id, prompt_object.object_id)
            prompt = parse_prompt(prompt_object.payload, location)
            if prompt is None:
                logger.warning(f"ignoring an invalid prompt at {location}")
                continue
            logger.info(f"prompt {prompt.location}: {prompt.questions}")
            self.prompt = prompt


async def forward_pictures(
    video: AsyncIterator[moq_wasm.Object], codec: str, latest_prompt: LatestPrompt, worker: PipelineWorker
):
    next_sample_at = 0.0
    decoder: VideoDecoder | None = None
    group_id: int | None = None
    async for video_object in video:
        # Each group starts at a keyframe, so each gets a fresh decoder. A group joined in the middle
        # has no keyframe and decodes to nothing.
        if video_object.group_id != group_id:
            decoder = VideoDecoder(codec)
            group_id = video_object.group_id
        picture = await asyncio.to_thread(decoder.decode, video_object.payload)
        decoded_at = time.monotonic()
        prompt = latest_prompt.prompt
        if picture is not None and prompt is not None and decoded_at >= next_sample_at:
            next_sample_at = decoded_at + SAMPLE_INTERVAL_SECONDS
            jpeg = await asyncio.to_thread(picture_to_jpeg, picture)
            await worker.queue_frame(
                CameraPicture(
                    jpeg=jpeg,
                    location=(video_object.group_id, video_object.object_id),
                    decoded_at=decoded_at,
                    prompt=prompt,
                )
            )


async def video_track(camera: RemoteBroadcast) -> CatalogTrack:
    """The camera's video track, from the catalog the relay holds for the page's other viewers or a
    later one; retries while the catalog cannot be read."""
    while True:
        try:
            async with aclosing(camera.catalogs()) as catalogs:
                async for catalog in catalogs:
                    track = camera_video_track(catalog)
                    if track is not None:
                        return track
            logger.info("the catalog ended before it listed a video track")
        except RuntimeError as error:
            logger.info(f"cannot read the catalog: {error}")
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def forward_camera(worker: PipelineWorker, camera: RemoteBroadcast):
    latest_prompt = LatestPrompt()
    following = asyncio.create_task(latest_prompt.follow(camera.follow(PROMPT_TRACK)))
    try:
        track = await video_track(camera)
        await forward_pictures(camera.follow(track.name), track.codec or "", latest_prompt, worker)
    finally:
        following.cancel()


async def detect_in_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    room: BotRoom,
    session: moq_wasm.Session,
) -> None:
    """Returns once no page has published the room's camera for the grace period."""
    namespace = room_namespace(room)
    camera_namespace = [*namespace, "camera"]
    async with create_broadcast(session, [*namespace, "detector"]) as detector:
        timeline = detector.event_timeline(EVENT_TIMELINE_TRACK, retention_seconds=RETENTION_SECONDS)
        status_track = detector.publish_track(STATUS_TRACK, retention_seconds=RETENTION_SECONDS)
        status = SystemOneStatus()
        vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
        worker = PipelineWorker(
            Pipeline([CameraDetector(vision, timeline)]),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
        runner = WorkerRunner(handle_sigint=False)
        await runner.add_workers(worker)
        await detector.published()
        await status.publish_on(status_track)
        async with asyncio.TaskGroup() as tasks:
            tasks.create_task(runner.run())
            await watch_broadcasts(
                session,
                namespace,
                partial(forward_camera, worker),
                filter=lambda published: published == camera_namespace,
                empty_timeout=GRACE_SECONDS,
            )
            await runner.cancel()


async def detect_in_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, room: BotRoom
) -> None:
    await stay_connected(args, room, partial(detect_in_session, args, http, identity_token, room))


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await serve_rooms(args, partial(detect_in_room, args, http, identity_token))


if __name__ == "__main__":
    asyncio.run(main())
