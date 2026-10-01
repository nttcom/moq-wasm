import argparse
import asyncio
import time

import aiohttp
import moq
from loguru import logger
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.detector import CameraKeyframe, PersonDetector
from moq_camera_detection.event_timeline import EventTimeline
from moq_camera_detection.identity_token import GcloudIdentityToken
from moq_camera_detection.keyframe import keyframe_to_jpeg
from moq_camera_detection.vision import DjevVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CAMERA_BROADCAST_PATH = "anon/moq-camera-detection/camera"
DETECTOR_BROADCAST_PATH = "anon/moq-camera-detection/detector"
VIDEO_TRACK = "video"
EVENT_TIMELINE_TRACK = "eventtimeline"
RESUBSCRIBE_DELAY_SECONDS = 1.0
# djev-vision scales to zero; a cold start copies 19 GB of weights before it answers.
VISION_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Judge whether a person is on the MoQ camera")
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument(
        "--djev-url", required=True, help="djev-vision's /v1/chat/completions endpoint"
    )
    parser.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    return parser.parse_args()


async def forward_keyframes(client: moq.Client, worker: PipelineWorker):
    while True:
        try:
            camera = await client.announced_broadcast(CAMERA_BROADCAST_PATH)
            track = await camera.subscribe_track(VIDEO_TRACK)
            async for group in track:
                frame = await group.read_frame()
                group.cancel()
                if frame is None:
                    continue
                jpeg = await asyncio.to_thread(keyframe_to_jpeg, frame.payload)
                if jpeg:
                    await worker.queue_frame(
                        CameraKeyframe(jpeg=jpeg, group_id=group.sequence, decoded_at=time.monotonic())
                    )
        except moq.Error as error:
            logger.info(f"camera track ended: {error}")
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def main():
    args = parse_args()
    publish_origin = moq.OriginProducer()
    detector_broadcast = publish_origin.create_broadcast(DETECTOR_BROADCAST_PATH)
    timeline = EventTimeline(detector_broadcast, EVENT_TIMELINE_TRACK)

    async with (
        aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as session,
        moq.Client(
            args.relay_url,
            tls_verify=not args.insecure,
            publish=publish_origin,
            subscribe=moq.OriginProducer(),
        ) as client,
    ):
        vision = DjevVisionClient(
            session, args.djev_url, GcloudIdentityToken() if args.gcloud_auth else None
        )
        worker = PipelineWorker(
            Pipeline([PersonDetector(vision, timeline)]),
            enable_rtvi=False,
            idle_timeout_secs=None,
        )
        runner = WorkerRunner()
        await runner.add_workers(worker)
        forwarding = asyncio.create_task(forward_keyframes(client, worker))
        try:
            await runner.run()
        finally:
            forwarding.cancel()


if __name__ == "__main__":
    asyncio.run(main())
