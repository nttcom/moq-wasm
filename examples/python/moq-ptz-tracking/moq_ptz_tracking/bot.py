import argparse
import asyncio
from collections.abc import AsyncIterator
from dataclasses import dataclass
from functools import partial

import aiohttp
import moq_wasm
from loguru import logger
from moq_bot_rooms.identity_token import IdentityToken, add_identity_token_arguments, identity_token_from_arguments
from moq_bot_rooms.relay import add_relay_arguments
from moq_bot_rooms.rooms import GRACE_SECONDS, BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.relay import stay_connected
from moq_system_one_bot.video import picture_to_jpeg
from moq_wasm.codecs import decode_pictures
from moq_wasm.media import RemoteBroadcast, create_broadcast, watch_broadcasts
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands
from moq_ptz_tracking.target import LatestTarget, parse_prompt
from moq_ptz_tracking.tracker import CameraPicture, PtzTracker
from moq_ptz_tracking.vision import SystemOneVisionClient

DEFAULT_PORT = 8099
# There is one ONVIF camera, so there is one room.
ROOM_ID = "onvif"
COMMAND_TRACK = "command"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
STATUS_TRACK = "status"
# What the bot publishes is kept for FETCH as long as a room runs by default (--max-room-seconds).
RETENTION_SECONDS = 60 * 60
# onvif-ingest publishes the camera's H.264.
CAMERA_CODEC = "avc1"
SAMPLE_INTERVAL_SECONDS = 0.1
JPEG_WIDTH = 640


@dataclass(frozen=True)
class RoomNamespaces:
    """The room's namespaces under its root, the same ones examples/browser/examples/moq-ptz-tracking/main.ts uses."""

    camera: list[str]
    command: list[str]
    viewer: list[str]
    tracker: list[str]

    @classmethod
    def of(cls, room: BotRoom) -> "RoomNamespaces":
        root = room.namespace_root
        return cls(
            camera=[root, "onvif", "client"],
            command=[root, "onvif", "viewer"],
            viewer=[root, "moq-ptz-tracking", "viewer"],
            tracker=[root, "moq-ptz-tracking", "tracker"],
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Pan and tilt the ONVIF camera so every box of the MoQ PTZ tracking page's targets stays in the picture"
    )
    add_serve_arguments(
        parser, bot="moq-ptz-tracking", default_port=DEFAULT_PORT, default_max_rooms=1
    )
    add_relay_arguments(parser)
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
        "--margin",
        type=float,
        default=0.1,
        help="share of the picture width and height on each side the targets' boxes must stay out of before the camera turns",
    )
    parser.add_argument(
        "--dead-zone",
        type=float,
        default=0.05,
        help="largest offset of the boxes' center from the picture's, as a fraction of it, that leaves an axis still",
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


async def follow_prompts(latest_target: LatestTarget, viewer: RemoteBroadcast):
    try:
        async for prompt_object in viewer.follow(PROMPT_TRACK):
            location = (prompt_object.group_id, prompt_object.object_id)
            try:
                target = parse_prompt(prompt_object.payload, location)
            except ValueError as error:
                logger.warning(f"ignoring an invalid prompt at {location}: {error}")
                continue
            logger.info(f"prompt {location}: {target}")
            latest_target.set(target)
    finally:
        latest_target.set(None)


async def forward_video(
    video: AsyncIterator[moq_wasm.Object], latest_target: LatestTarget, worker: PipelineWorker
):
    async for decoded in decode_pictures(video, CAMERA_CODEC, interval=SAMPLE_INTERVAL_SECONDS):
        target = latest_target.target
        if target is None:
            continue
        jpeg = await asyncio.to_thread(picture_to_jpeg, decoded.picture, JPEG_WIDTH)
        await worker.queue_frame(
            CameraPicture(
                jpeg=jpeg,
                location=(decoded.location.group_id, decoded.location.object_id),
                decoded_at=decoded.decoded_at,
                target=target,
            )
        )


async def forward_camera(latest_target: LatestTarget, worker: PipelineWorker, camera: RemoteBroadcast):
    """Subscribes to the camera only while a target is set, because onvif-ingest streams the profile
    its first subscriber picks and rejects the others."""
    while True:
        target = latest_target.target
        if target is None:
            await latest_target.changed()
            continue
        logger.info(f"watching {'/'.join(camera.namespace)}/{target.video_track}")
        watching = asyncio.create_task(forward_video(camera.follow(target.video_track), latest_target, worker))
        try:
            await latest_target.leaves(target.video_track)
        finally:
            watching.cancel()


async def track_in_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    namespaces: RoomNamespaces,
    session: moq_wasm.Session,
) -> None:
    """Returns once no viewer has published `namespaces.viewer` for the grace period."""
    # Both broadcasts take their tracks before either is published.
    tracker_broadcast = create_broadcast(session, namespaces.tracker)
    command_broadcast = create_broadcast(session, namespaces.command)
    timeline = tracker_broadcast.event_timeline(EVENT_TIMELINE_TRACK, retention_seconds=RETENTION_SECONDS)
    status_track = tracker_broadcast.publish_track(STATUS_TRACK, retention_seconds=RETENTION_SECONDS)
    status = SystemOneStatus()
    commands = PtzCommands()
    commands.publish_on(command_broadcast.publish_track(COMMAND_TRACK, retention_seconds=RETENTION_SECONDS))
    latest_target = LatestTarget()
    vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
    calibration = PtzCalibration(args.pan_per_second, args.tilt_per_second, args.margin, args.dead_zone)
    tracker = PtzTracker(vision, timeline, commands, calibration, latest_target)
    worker = PipelineWorker(Pipeline([tracker]), enable_rtvi=False, idle_timeout_secs=None)
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    async with tracker_broadcast, command_broadcast, asyncio.TaskGroup() as tasks:
        tasks.create_task(runner.run())
        await tracker_broadcast.published()
        await status.publish_on(status_track)
        forwarding = tasks.create_task(
            watch_broadcasts(
                session,
                namespaces.camera[:-1],
                partial(forward_camera, latest_target, worker),
                filter=lambda published: published == namespaces.camera,
            )
        )
        await watch_broadcasts(
            session,
            namespaces.viewer[:-1],
            partial(follow_prompts, latest_target),
            filter=lambda published: published == namespaces.viewer,
            empty_timeout=GRACE_SECONDS,
        )
        forwarding.cancel()
        await runner.cancel()


async def track_in_room(
    args: argparse.Namespace, http: aiohttp.ClientSession, identity_token: IdentityToken | None, room: BotRoom
) -> None:
    await stay_connected(args, room, partial(track_in_session, args, http, identity_token, RoomNamespaces.of(room)))


async def main():
    args = parse_args()
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await serve_rooms(args, partial(track_in_room, args, http, identity_token), only_room=ROOM_ID)


if __name__ == "__main__":
    asyncio.run(main())
