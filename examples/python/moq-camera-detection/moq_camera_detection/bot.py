import argparse
import asyncio
import time
from functools import partial

import aiohttp
import moqt
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_d3_flash_bot.d3_flash_status import D3FlashStatus
from moq_d3_flash_bot.identity_token import add_identity_token_arguments, identity_token_from_arguments
from moq_d3_flash_bot.namespace import while_published
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.detector import CameraDetector, CameraPicture
from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.prompt import Prompt, parse_prompt
from moq_camera_detection.video import GroupDecoder, picture_to_jpeg
from moq_camera_detection.vision import D3FlashVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8098
# d3-flash answers one batch at a time, and one camera already sends it a picture every SAMPLE_INTERVAL_SECONDS.
DEFAULT_MAX_ROOMS = 1
NAMESPACE_ROOT = ["anon", "moq-camera-detection"]
VIDEO_TRACK = "video"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# The event timeline and the d3-flash status are kept for FETCH as long as a room runs by default
# (--max-room-seconds).
RETENTION_SECONDS = 60 * 60
RESUBSCRIBE_DELAY_SECONDS = 1.0
RECONNECT_DELAY_SECONDS = 2.0
SAMPLE_INTERVAL_SECONDS = 0.1
# d3-flash scales to zero; a cold start copies 17 GB of weights before it answers.
VISION_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Answer the MoQ camera page's question about its camera, one bot per camera room"
    )
    add_serve_arguments(parser, default_port=DEFAULT_PORT, default_max_rooms=DEFAULT_MAX_ROOMS)
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument(
        "--d3-flash-url", required=True, help="a System One /v1/systemone endpoint that reads images, such as d3-flash"
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


def room_namespace(room_id: str) -> list[str]:
    return [*NAMESPACE_ROOT, room_id]


class LatestPrompt:
    def __init__(self):
        self.prompt: Prompt | None = None

    async def follow(self, track: moqt.Track):
        try:
            async for prompt_object in track:
                location = (prompt_object.group_id, prompt_object.object_id)
                prompt = parse_prompt(prompt_object.payload, location)
                if prompt is None:
                    logger.warning(f"ignoring an invalid prompt at {location}")
                    continue
                logger.info(f"prompt {prompt.location}: {prompt.question} {prompt.choices}")
                self.prompt = prompt
        except RuntimeError as error:
            logger.info(f"prompt track ended: {error}")


async def forward_pictures(video: moqt.Track, latest_prompt: LatestPrompt, worker: PipelineWorker):
    next_sample_at = 0.0
    decoder: GroupDecoder | None = None
    group_id: int | None = None
    async for video_object in video:
        # Each group starts at a keyframe, so each gets a fresh decoder. A group joined in the middle
        # has no keyframe and decodes to nothing.
        if video_object.group_id != group_id:
            decoder = GroupDecoder()
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


async def forward_camera(session: moqt.Session, camera_namespace: list[str], worker: PipelineWorker):
    while True:
        try:
            latest_prompt = LatestPrompt()
            prompts = await session.subscribe(camera_namespace, PROMPT_TRACK)
            following = asyncio.create_task(latest_prompt.follow(prompts))
            try:
                video = await session.subscribe(camera_namespace, VIDEO_TRACK)
                await forward_pictures(video, latest_prompt, worker)
                logger.info("camera track ended")
            finally:
                following.cancel()
        except RuntimeError as error:
            logger.info(f"camera track ended: {error}")
        # The relay ends the tracks when the page it subscribed to leaves; another page in the room
        # may still publish the camera namespace, so subscribe again.
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def detect_in_session(
    session: moqt.Session, room_id: str, timeline: EventTimeline, status: D3FlashStatus, worker: PipelineWorker
) -> None:
    detector_namespace = [*room_namespace(room_id), "detector"]
    # Registered before the namespace is published, since the relay may forward a SUBSCRIBE at once.
    for track_name, publisher in ((EVENT_TIMELINE_TRACK, timeline), (STATUS_TRACK, status)):
        await publisher.publish_on(
            await session.track(detector_namespace, track_name, retention_seconds=RETENTION_SECONDS)
        )
    await session.publish_namespace(detector_namespace)
    camera_namespace = [*room_namespace(room_id), "camera"]
    following = asyncio.create_task(
        while_published(session, camera_namespace, partial(forward_camera, session, camera_namespace, worker))
    )
    try:
        await session.closed()
    finally:
        following.cancel()


async def stay_connected(
    args: argparse.Namespace, room_id: str, timeline: EventTimeline, status: D3FlashStatus, worker: PipelineWorker
) -> None:
    while True:
        try:
            session = await moqt.connect(args.relay_url, verify_certificate=not args.insecure)
        except RuntimeError as error:
            logger.warning(f"room {room_id}: cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"room {room_id}: connected to {args.relay_url}")
        try:
            await detect_in_session(session, room_id, timeline, status, worker)
            logger.warning(f"room {room_id}: relay session closed")
        except RuntimeError as error:
            logger.warning(f"room {room_id}: relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def detect_in_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, room_id: str
) -> None:
    timeline = EventTimeline()
    status = D3FlashStatus()
    vision = D3FlashVisionClient(http, args.d3_flash_url, status, identity_token)
    worker = PipelineWorker(
        Pipeline([CameraDetector(vision, timeline)]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    async with asyncio.TaskGroup() as tasks:
        connecting = tasks.create_task(stay_connected(args, room_id, timeline, status, worker))
        await runner.run()
        connecting.cancel()


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.d3_flash_url)
        await serve_rooms(args, partial(detect_in_room, args, http, identity_token))


if __name__ == "__main__":
    asyncio.run(main())
