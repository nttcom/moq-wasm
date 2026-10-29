import argparse
import asyncio
import time
from functools import partial

import aiohttp
import moqt
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken
from moq_bot_rooms.rooms import GRACE_SECONDS
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.identity_token import add_identity_token_arguments, identity_token_from_arguments
from moq_system_one_bot.namespace import until_room_empty, while_published
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_ptz_tracking.event_timeline import EventTimeline
from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands
from moq_ptz_tracking.target import LatestTarget, parse_prompt
from moq_ptz_tracking.tracker import CameraPicture, PtzTracker
from moq_ptz_tracking.video import GroupDecoder, picture_to_jpeg
from moq_ptz_tracking.vision import SystemOneVisionClient

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"
DEFAULT_PORT = 8099
# There is one ONVIF camera, so there is one room.
ROOM_ID = "onvif"
CAMERA_NAMESPACE = ["anon", "onvif", "client"]
COMMAND_NAMESPACE = ["anon", "onvif", "viewer"]
COMMAND_TRACK = "command"
VIEWER_NAMESPACE = ["anon", "moq-ptz-tracking", "viewer"]
TRACKER_NAMESPACE = ["anon", "moq-ptz-tracking", "tracker"]
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# What the bot publishes is kept for FETCH as long as a room runs by default (--max-room-seconds).
RETENTION_SECONDS = 60 * 60
RESUBSCRIBE_DELAY_SECONDS = 1.0
RECONNECT_DELAY_SECONDS = 2.0
SAMPLE_INTERVAL_SECONDS = 0.1
# The System One service scales to zero; a cold start copies its weights before it answers.
VISION_REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10 * 60)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Pan and tilt the ONVIF camera so the MoQ PTZ tracking page's target stays centered"
    )
    add_serve_arguments(parser, default_port=DEFAULT_PORT, default_max_rooms=1)
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
    parser.add_argument(
        "--system-one-url", required=True, help="a System One /v1/systemone endpoint that reads images, such as system-one-server"
    )
    parser.add_argument(
        "--pan-per-second",
        type=float,
        default=0.27,
        help="fraction of the picture width a one-second pan to the right shifts the view; negative for a camera that pans left",
    )
    parser.add_argument(
        "--tilt-per-second",
        type=float,
        default=0.38,
        help="fraction of the picture height a one-second tilt up shifts the view; negative for a camera that tilts down",
    )
    parser.add_argument(
        "--dead-zone",
        type=float,
        default=0.05,
        help="largest offset of the target from the center, as a fraction of the picture, that leaves the camera still",
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


async def follow_prompts(session: moqt.Session, latest_target: LatestTarget):
    while True:
        try:
            async for prompt_object in await session.subscribe(VIEWER_NAMESPACE, PROMPT_TRACK):
                location = (prompt_object.group_id, prompt_object.object_id)
                try:
                    target = parse_prompt(prompt_object.payload, location)
                except ValueError as error:
                    logger.warning(f"ignoring an invalid prompt at {location}: {error}")
                    continue
                logger.info(f"prompt {location}: {target}")
                latest_target.set(target)
            logger.info("prompt track ended")
        except RuntimeError as error:
            logger.info(f"prompt track ended: {error}")
        finally:
            latest_target.set(None)
        await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def forward_video(session: moqt.Session, video_track: str, latest_target: LatestTarget, worker: PipelineWorker):
    try:
        video = await session.subscribe(CAMERA_NAMESPACE, video_track)
        logger.info(f"watching {'/'.join(CAMERA_NAMESPACE)}/{video_track}")
        next_sample_at = 0.0
        decoder: GroupDecoder | None = None
        group_id: int | None = None
        async for video_object in video:
            # Each group starts at a keyframe, so each gets a fresh decoder. A group joined in the
            # middle has no keyframe and decodes to nothing.
            if video_object.group_id != group_id:
                decoder = GroupDecoder()
                group_id = video_object.group_id
            picture = await asyncio.to_thread(decoder.decode, video_object.payload)
            decoded_at = time.monotonic()
            target = latest_target.target
            if picture is not None and target is not None and decoded_at >= next_sample_at:
                next_sample_at = decoded_at + SAMPLE_INTERVAL_SECONDS
                jpeg = await asyncio.to_thread(picture_to_jpeg, picture)
                await worker.queue_frame(
                    CameraPicture(
                        jpeg=jpeg,
                        location=(video_object.group_id, video_object.object_id),
                        decoded_at=decoded_at,
                        target=target,
                    )
                )
        logger.info("camera track ended")
    except RuntimeError as error:
        logger.info(f"camera track ended: {error}")


async def forward_camera(session: moqt.Session, latest_target: LatestTarget, worker: PipelineWorker):
    """Subscribes to the camera only while a target is set, because onvif-ingest streams the profile
    its first subscriber picks and rejects the others."""
    while True:
        target = latest_target.target
        if target is None:
            await latest_target.changed()
            continue
        watching = asyncio.create_task(forward_video(session, target.video_track, latest_target, worker))
        leaving = asyncio.create_task(latest_target.leaves(target.video_track))
        try:
            done, _ = await asyncio.wait({watching, leaving}, return_when=asyncio.FIRST_COMPLETED)
        finally:
            watching.cancel()
            leaving.cancel()
        if watching in done:
            await asyncio.sleep(RESUBSCRIBE_DELAY_SECONDS)


async def track_in_session(
    session: moqt.Session,
    timeline: EventTimeline,
    status: SystemOneStatus,
    commands: PtzCommands,
    latest_target: LatestTarget,
    worker: PipelineWorker,
) -> bool:
    """True once no viewer has published `VIEWER_NAMESPACE` for the grace period; False when the relay
    session closed."""
    # Registered before the namespaces are published, since the relay may forward a SUBSCRIBE at once.
    for namespace, track_name, publisher in (
        (TRACKER_NAMESPACE, EVENT_TIMELINE_TRACK, timeline),
        (TRACKER_NAMESPACE, STATUS_TRACK, status),
        (COMMAND_NAMESPACE, COMMAND_TRACK, commands),
    ):
        await publisher.publish_on(await session.track(namespace, track_name, retention_seconds=RETENTION_SECONDS))
    await session.publish_namespace(TRACKER_NAMESPACE)
    await session.publish_namespace(COMMAND_NAMESPACE)
    following = asyncio.create_task(
        while_published(
            session,
            VIEWER_NAMESPACE,
            partial(follow_prompts, session, latest_target),
            unpublished_seconds=GRACE_SECONDS,
        )
    )
    forwarding = asyncio.create_task(
        while_published(session, CAMERA_NAMESPACE, partial(forward_camera, session, latest_target, worker))
    )
    try:
        return await until_room_empty(session, following)
    finally:
        following.cancel()
        forwarding.cancel()


async def stay_connected(
    args: argparse.Namespace,
    timeline: EventTimeline,
    status: SystemOneStatus,
    commands: PtzCommands,
    latest_target: LatestTarget,
    worker: PipelineWorker,
):
    while True:
        try:
            session = await moqt.connect(args.relay_url, verify_certificate=not args.insecure)
        except RuntimeError as error:
            logger.warning(f"cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"connected to {args.relay_url}")
        try:
            if await track_in_session(session, timeline, status, commands, latest_target, worker):
                return
            logger.warning("relay session closed")
        except RuntimeError as error:
            logger.warning(f"relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)


async def track_in_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, _room_id: str
) -> None:
    timeline = EventTimeline()
    status = SystemOneStatus()
    commands = PtzCommands()
    latest_target = LatestTarget()
    vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
    calibration = PtzCalibration(args.pan_per_second, args.tilt_per_second, args.dead_zone)
    tracker = PtzTracker(vision, timeline, commands, calibration, latest_target)
    worker = PipelineWorker(Pipeline([tracker]), enable_rtvi=False, idle_timeout_secs=None)
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    async with asyncio.TaskGroup() as tasks:
        tasks.create_task(runner.run())
        await stay_connected(args, timeline, status, commands, latest_target, worker)
        await runner.cancel()


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=VISION_REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await serve_rooms(args, partial(track_in_room, args, http, identity_token), only_room=ROOM_ID)


if __name__ == "__main__":
    asyncio.run(main())
