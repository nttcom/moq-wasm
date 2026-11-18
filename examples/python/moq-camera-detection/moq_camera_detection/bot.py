import argparse
import asyncio
from functools import partial

import aiohttp
import moq_wasm
from moq_bot_rooms.bot_job import BotJob, Bots, add_bot_job_arguments, serve_bot_job
from moq_bot_rooms.identity_token import IdentityToken, add_identity_token_arguments, identity_token_from_arguments
from moq_bot_rooms.relay import add_relay_arguments
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

STATUS_TRACK = "status"
# The event timeline and the System One status are kept for FETCH as long as the bot service lets a
# room's bot run.
RETENTION_SECONDS = 60 * 60


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Answer the MoQ camera page's question about its camera, in the room the bot service launched the bot for"
    )
    parser.set_defaults(bot="moq-camera-detection")
    add_bot_job_arguments(parser)
    add_relay_arguments(parser)
    parser.add_argument(
        "--system-one-url", required=True, help="a System One /v1/systemone endpoint that reads images, such as system-one-server"
    )
    add_identity_token_arguments(parser)
    return parser.parse_args()


async def detect_in_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    job: BotJob,
    session: moq_wasm.Session,
) -> None:
    """Returns once nothing has been published in the room for the grace period."""
    transport = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=job.namespace_prefix,
            broadcast=job.broadcast,
            video_in_enabled=True,
            video_in_sources={CAMERA: VideoInSourceParams(on_request_only=True)},
            video_in_track=camera_video_track,
            data_in_tracks=[PROMPT_TRACK],
            event_timelines_out={EVENT_TIMELINE_TRACK: RETENTION_SECONDS},
            data_out_tracks={STATUS_TRACK: RETENTION_SECONDS},
            empty_timeout=job.grace_seconds,
        ),
    )
    status = SystemOneStatus()
    vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
    worker = PipelineWorker(
        Pipeline([transport.input(), CameraDetector(vision), transport.output()]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # serve_bot_job owns SIGINT and SIGTERM.
    runner = WorkerRunner(handle_sigint=False)

    @transport.event_handler("on_room_empty")
    async def on_room_empty(_):
        await runner.cancel()

    await runner.add_workers(worker)
    await status.publish_on(PipelineTrack(worker, STATUS_TRACK))
    await runner.run()


async def detect_in_room(args: argparse.Namespace, job: BotJob, bots: Bots) -> None:
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await stay_connected(args, bots, partial(detect_in_session, args, http, identity_token, job))


async def main():
    args = parse_args()
    await serve_bot_job(args, partial(detect_in_room, args))


if __name__ == "__main__":
    asyncio.run(main())
