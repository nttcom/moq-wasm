import argparse
import asyncio
import time
from functools import partial

import aiohttp
import moq
from loguru import logger
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.d3_flash_status import D3FlashStatus
from moq_camera_detection.detector import CameraDetector, CameraPicture
from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.identity_token import IdentityToken, gcloud_identity_token, metadata_identity_token
from moq_camera_detection.prompt import Prompt, parse_prompt
from moq_camera_detection.video import GroupDecoder, picture_to_jpeg
from moq_camera_detection.vision import D3FlashVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8098
# d3-flash answers one batch at a time, and one camera already sends it a picture every 0.1 s.
DEFAULT_MAX_ROOMS = 1
NAMESPACE_ROOT = "anon/moq-camera-detection"
VIDEO_TRACK = "video"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
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
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    auth.add_argument(
        "--metadata-auth",
        action="store_true",
        help="send the metadata server's identity token, for a bot on Cloud Run or GCE",
    )
    return parser.parse_args()


class LatestPrompt:
    def __init__(self):
        self.prompt: Prompt | None = None

    async def follow(self, track: moq.TrackConsumer):
        try:
            async for group in track:
                frame = await group.read_frame()
                prompt = parse_prompt(frame.payload, (group.sequence, 0)) if frame else None
                if prompt is None:
                    logger.warning(f"ignoring an invalid prompt in group {group.sequence}")
                    continue
                logger.info(f"prompt {prompt.location}: {prompt.question} {prompt.choices}")
                self.prompt = prompt
        except moq.Error as error:
            logger.info(f"prompt track ended: {error}")


async def forward_pictures(video: moq.TrackConsumer, latest_prompt: LatestPrompt, worker: PipelineWorker):
    next_sample_at = 0.0
    async for group in video:
        decoder = GroupDecoder()
        object_id = 0
        while (frame := await group.read_frame()) is not None:
            picture = await asyncio.to_thread(decoder.decode, frame.payload)
            decoded_at = time.monotonic()
            prompt = latest_prompt.prompt
            if picture is not None and prompt is not None and decoded_at >= next_sample_at:
                next_sample_at = decoded_at + SAMPLE_INTERVAL_SECONDS
                jpeg = await asyncio.to_thread(picture_to_jpeg, picture)
                await worker.queue_frame(
                    CameraPicture(
                        jpeg=jpeg,
                        location=(group.sequence, object_id),
                        decoded_at=decoded_at,
                        prompt=prompt,
                    )
                )
            object_id += 1


async def forward_camera(client: moq.Client, camera_path: str, worker: PipelineWorker):
    while True:
        try:
            camera = await client.announced_broadcast(camera_path)
            latest_prompt = LatestPrompt()
            following = asyncio.create_task(latest_prompt.follow(await camera.subscribe_track(PROMPT_TRACK)))
            try:
                await forward_pictures(await camera.subscribe_track(VIDEO_TRACK), latest_prompt, worker)
            finally:
                following.cancel()
        except moq.Error as error:
            logger.info(f"camera track ended: {error}")
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def stay_connected(
    args: argparse.Namespace, camera_path: str, publish_origin: moq.OriginProducer, worker: PipelineWorker
):
    while True:
        try:
            async with moq.Client(
                args.relay_url,
                tls_verify=not args.insecure,
                publish=publish_origin,
                subscribe=moq.OriginProducer(),
            ) as client:
                logger.info(f"connected to {args.relay_url}")
                async with asyncio.TaskGroup() as tasks:
                    forwarding = tasks.create_task(forward_camera(client, camera_path, worker))
                    await client.session.closed()
                    forwarding.cancel()
            logger.warning("relay session closed")
        except* moq.Error as errors:
            logger.warning(f"relay session failed: {errors.exceptions[0]}")
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def detect_in_room(
    args: argparse.Namespace, session: aiohttp.ClientSession, identity_token: IdentityToken | None, room_id: str
) -> None:
    publish_origin = moq.OriginProducer()
    detector_broadcast = publish_origin.create_broadcast(f"{NAMESPACE_ROOT}/{room_id}/detector")
    timeline = EventTimeline(detector_broadcast, EVENT_TIMELINE_TRACK)
    status = D3FlashStatus(STATUS_TRACK)
    status.publish_on(detector_broadcast)
    vision = D3FlashVisionClient(session, args.d3_flash_url, status, identity_token)
    worker = PipelineWorker(
        Pipeline([CameraDetector(vision, timeline)]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    camera_path = f"{NAMESPACE_ROOT}/{room_id}/camera"
    async with asyncio.TaskGroup() as tasks:
        connecting = tasks.create_task(stay_connected(args, camera_path, publish_origin, worker))
        await runner.run()
        connecting.cancel()


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as session:
        identity_token = (
            gcloud_identity_token()
            if args.gcloud_auth
            else metadata_identity_token(session, args.d3_flash_url)
            if args.metadata_auth
            else None
        )
        await serve_rooms(args, partial(detect_in_room, args, session, identity_token))


if __name__ == "__main__":
    asyncio.run(main())
