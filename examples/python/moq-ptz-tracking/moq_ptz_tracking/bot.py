import argparse
import asyncio
import time

import aiohttp
import moq
from loguru import logger
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_ptz_tracking.djev_status import DjevStatus
from moq_ptz_tracking.event_timeline import EventTimeline
from moq_ptz_tracking.identity_token import gcloud_identity_token, metadata_identity_token
from moq_ptz_tracking.ptz import PtzCommands, PtzSteps
from moq_ptz_tracking.target import LatestTarget, parse_prompt
from moq_ptz_tracking.tracker import CameraPicture, PtzTracker
from moq_ptz_tracking.video import GroupDecoder, picture_to_jpeg
from moq_ptz_tracking.vision import DjevVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
CAMERA_BROADCAST_PATH = "anon/onvif/client"
COMMAND_BROADCAST_PATH = "anon/onvif/viewer"
COMMAND_TRACK = "command"
VIEWER_BROADCAST_PATH = "anon/moq-ptz-tracking/viewer"
TRACKER_BROADCAST_PATH = "anon/moq-ptz-tracking/tracker"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
RESUBSCRIBE_DELAY_SECONDS = 1.0
RECONNECT_DELAY_SECONDS = 2.0
SAMPLE_INTERVAL_SECONDS = 0.1
# djev-vision scales to zero; a cold start copies 19 GB of weights before it answers.
VISION_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Pan and tilt the ONVIF camera so the MoQ PTZ tracking page's target stays centered"
    )
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
        "--pan-step",
        type=float,
        default=0.3,
        help="RelativeMove pan for a target one grid column off center; negative for a flipped camera",
    )
    parser.add_argument(
        "--tilt-step",
        type=float,
        default=0.3,
        help="RelativeMove tilt for a target one grid row off center; negative for a flipped camera",
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


async def follow_prompts(client: moq.Client, latest_target: LatestTarget):
    while True:
        try:
            viewer = await client.announced_broadcast(VIEWER_BROADCAST_PATH)
            async with await viewer.subscribe_track(PROMPT_TRACK) as prompts:
                async for group in prompts:
                    frame = await group.read_frame()
                    try:
                        target = parse_prompt(frame.payload if frame else b"", (group.sequence, 0))
                    except ValueError as error:
                        logger.warning(f"ignoring an invalid prompt in group {group.sequence}: {error}")
                        continue
                    logger.info(f"prompt {group.sequence}: {target}")
                    latest_target.set(target)
        except moq.Error as error:
            logger.info(f"prompt track ended: {error}")
        finally:
            latest_target.set(None)
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


class GroupSampler:
    def __init__(self, latest_target: LatestTarget, worker: PipelineWorker):
        self._latest_target = latest_target
        self._worker = worker
        self._next_sample_at = 0.0

    async def forward(self, group: moq.GroupConsumer):
        decoder = GroupDecoder()
        object_id = 0
        while (frame := await group.read_frame()) is not None:
            picture = await asyncio.to_thread(decoder.decode, frame.payload)
            decoded_at = time.monotonic()
            target = self._latest_target.target
            if picture is not None and target is not None and decoded_at >= self._next_sample_at:
                self._next_sample_at = decoded_at + SAMPLE_INTERVAL_SECONDS
                jpeg = await asyncio.to_thread(picture_to_jpeg, picture)
                await self._worker.queue_frame(
                    CameraPicture(
                        jpeg=jpeg,
                        location=(group.sequence, object_id),
                        decoded_at=decoded_at,
                        target=target,
                    )
                )
            object_id += 1


async def forward_pictures(video: moq.TrackConsumer, latest_target: LatestTarget, worker: PipelineWorker):
    sampler = GroupSampler(latest_target, worker)
    async for group in video:
        try:
            await sampler.forward(group)
        except moq.Error as error:
            # The relay starts a subscriber that joins mid-group at the object after Largest
            # Object (draft-14 §9.7), and the moq library refuses a group that does not start at
            # object 0. That group holds no keyframe anyway; the next one starts at object 0.
            logger.debug(f"skipping camera group {group.sequence}: {error}")


async def forward_video(client: moq.Client, video_track: str, latest_target: LatestTarget, worker: PipelineWorker):
    try:
        camera = await client.announced_broadcast(CAMERA_BROADCAST_PATH)
        async with await camera.subscribe_track(video_track) as video:
            logger.info(f"watching {CAMERA_BROADCAST_PATH}/{video_track}")
            await forward_pictures(video, latest_target, worker)
    except moq.Error as error:
        logger.info(f"camera track ended: {error}")


async def forward_camera(client: moq.Client, latest_target: LatestTarget, worker: PipelineWorker):
    """Subscribes to the camera only while a target is set, because onvif-ingest streams the profile
    its first subscriber picks and rejects the others."""
    while True:
        target = latest_target.target
        if target is None:
            await latest_target.changed()
            continue
        watching = asyncio.create_task(forward_video(client, target.video_track, latest_target, worker))
        leaving = asyncio.create_task(latest_target.leaves(target.video_track))
        try:
            done, _ = await asyncio.wait({watching, leaving}, return_when=asyncio.FIRST_COMPLETED)
        finally:
            watching.cancel()
            leaving.cancel()
        if watching in done:
            await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def stay_connected(
    args: argparse.Namespace,
    publish_origin: moq.OriginProducer,
    latest_target: LatestTarget,
    worker: PipelineWorker,
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
                    following = tasks.create_task(follow_prompts(client, latest_target))
                    forwarding = tasks.create_task(forward_camera(client, latest_target, worker))
                    await client.session.closed()
                    following.cancel()
                    forwarding.cancel()
            logger.warning("relay session closed")
        except* moq.Error as errors:
            logger.warning(f"relay session failed: {errors.exceptions[0]}")
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def main():
    args = parse_args()
    publish_origin = moq.OriginProducer()
    tracker_broadcast = publish_origin.create_broadcast(TRACKER_BROADCAST_PATH)
    timeline = EventTimeline(tracker_broadcast, EVENT_TIMELINE_TRACK)
    status = DjevStatus(STATUS_TRACK)
    status.publish_on(tracker_broadcast)
    commands = PtzCommands(publish_origin.create_broadcast(COMMAND_BROADCAST_PATH), COMMAND_TRACK)
    latest_target = LatestTarget()

    async with aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as session:
        identity_token = (
            gcloud_identity_token()
            if args.gcloud_auth
            else metadata_identity_token(session, args.djev_url)
            if args.metadata_auth
            else None
        )
        vision = DjevVisionClient(session, args.djev_url, status, identity_token)
        tracker = PtzTracker(vision, timeline, commands, PtzSteps(args.pan_step, args.tilt_step), latest_target)
        worker = PipelineWorker(Pipeline([tracker]), enable_rtvi=False, idle_timeout_secs=None)
        runner = WorkerRunner()
        await runner.add_workers(worker)
        async with asyncio.TaskGroup() as tasks:
            connecting = tasks.create_task(stay_connected(args, publish_origin, latest_target, worker))
            await runner.run()
            connecting.cancel()


if __name__ == "__main__":
    asyncio.run(main())
