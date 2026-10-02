import argparse
import asyncio
import time

import aiohttp
import moq
from loguru import logger
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.detector import CameraPicture, CameraDetector
from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.identity_token import gcloud_identity_token, metadata_identity_token
from moq_camera_detection.prompt import Prompt, parse_prompt
from moq_camera_detection.video import GroupDecoder, picture_to_jpeg
from moq_camera_detection.vision import DjevVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CAMERA_BROADCAST_PATH = "anon/moq-camera-detection/camera"
DETECTOR_BROADCAST_PATH = "anon/moq-camera-detection/detector"
VIDEO_TRACK = "video"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
RESUBSCRIBE_DELAY_SECONDS = 1.0
RECONNECT_DELAY_SECONDS = 2.0
SAMPLE_INTERVAL_SECONDS = 0.1
# djev-vision scales to zero; a cold start copies 19 GB of weights before it answers.
VISION_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Answer the MoQ camera page's question about its camera")
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument(
        "--djev-url", required=True, help="djev-vision's /v1/chat/completions endpoint"
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
        help="send the GCE metadata server's identity token, for the bot VM",
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


async def forward_camera(client: moq.Client, worker: PipelineWorker):
    while True:
        try:
            camera = await client.announced_broadcast(CAMERA_BROADCAST_PATH)
            latest_prompt = LatestPrompt()
            following = asyncio.create_task(latest_prompt.follow(await camera.subscribe_track(PROMPT_TRACK)))
            try:
                await forward_pictures(await camera.subscribe_track(VIDEO_TRACK), latest_prompt, worker)
            finally:
                following.cancel()
        except moq.Error as error:
            logger.info(f"camera track ended: {error}")
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def stay_connected(args: argparse.Namespace, publish_origin: moq.OriginProducer, worker: PipelineWorker):
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
                    forwarding = tasks.create_task(forward_camera(client, worker))
                    await client.session.closed()
                    forwarding.cancel()
            logger.warning("relay session closed")
        except* moq.Error as errors:
            logger.warning(f"relay session failed: {errors.exceptions[0]}")
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def main():
    args = parse_args()
    publish_origin = moq.OriginProducer()
    detector_broadcast = publish_origin.create_broadcast(DETECTOR_BROADCAST_PATH)
    timeline = EventTimeline(detector_broadcast, EVENT_TIMELINE_TRACK)

    async with aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as session:
        identity_token = (
            gcloud_identity_token()
            if args.gcloud_auth
            else metadata_identity_token(session, args.djev_url)
            if args.metadata_auth
            else None
        )
        vision = DjevVisionClient(session, args.djev_url, identity_token)
        worker = PipelineWorker(
            Pipeline([CameraDetector(vision, timeline)]),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        runner = WorkerRunner()
        await runner.add_workers(worker)
        async with asyncio.TaskGroup() as tasks:
            connecting = tasks.create_task(stay_connected(args, publish_origin, worker))
            await runner.run()
            connecting.cancel()


if __name__ == "__main__":
    asyncio.run(main())
