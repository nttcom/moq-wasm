import argparse
import asyncio
from functools import partial

import aiohttp
import moq_wasm
from moq_bot_rooms.identity_token import IdentityToken, add_identity_token_arguments, identity_token_from_arguments
from moq_bot_rooms.relay import add_relay_arguments
from moq_bot_rooms.rooms import GRACE_SECONDS, BotRoom
from moq_bot_rooms.server import add_serve_arguments, serve_rooms
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.pipeline_track import PipelineTrack
from moq_system_one_bot.relay import stay_connected
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTransport
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.transports.base_transport import VideoInSourceParams
from pipecat.workers.runner import WorkerRunner

from moq_camera_detection.catalog import camera_video_track
from moq_camera_detection.detector import CAMERA, EVENT_TIMELINE_TRACK, PROMPT_TRACK, CameraDetector
from moq_camera_detection.vision import SystemOneVisionClient

DEFAULT_PORT = 8098
# system-one-server answers one request at a time, and one camera already asks it about a picture
# every SAMPLE_INTERVAL_SECONDS.
DEFAULT_MAX_ROOMS = 1
DEMO_NAME = "moq-camera-detection"
STATUS_TRACK = "status"
# The event timeline and the System One status are kept for FETCH as long as a room runs by default
# (--max-room-seconds).
RETENTION_SECONDS = 60 * 60


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


async def detect_in_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    room: BotRoom,
    session: moq_wasm.Session,
) -> None:
    """Returns once nothing has been published in the room for the grace period."""
    transport = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=room_namespace(room),
            participant_id="detector",
            video_in_enabled=True,
            video_in_sources={CAMERA: VideoInSourceParams(on_request_only=True)},
            video_in_track=camera_video_track,
            data_in_tracks=[PROMPT_TRACK],
            event_timelines_out={EVENT_TIMELINE_TRACK: RETENTION_SECONDS},
            data_out_tracks={STATUS_TRACK: RETENTION_SECONDS},
            empty_timeout=GRACE_SECONDS,
        ),
    )
    status = SystemOneStatus()
    vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
    worker = PipelineWorker(
        Pipeline([transport.input(), CameraDetector(vision), transport.output()]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # The room server owns SIGINT and SIGTERM; it stops every room before it exits.
    runner = WorkerRunner(handle_sigint=False)

    @transport.event_handler("on_room_empty")
    async def on_room_empty(_):
        await runner.cancel()

    await runner.add_workers(worker)
    await status.publish_on(PipelineTrack(worker, STATUS_TRACK))
    await runner.run()


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
