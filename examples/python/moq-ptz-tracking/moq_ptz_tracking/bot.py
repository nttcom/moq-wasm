import argparse
import asyncio
from functools import partial

import aiohttp
import moq_wasm
from moq_bot_job.bot_job import BotJob, Bots, add_bot_job_arguments, serve_bot_job
from moq_bot_job.identity_token import IdentityToken, add_identity_token_arguments, identity_token_from_arguments
from moq_system_one_bot.system_one_client import REQUEST_TIMEOUT
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.pipeline_track import PipelineTrack
from moq_system_one_bot.relay import stay_connected
from moq_wasm.pipecat import MoqWasmParams, MoqWasmTransport
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineWorker
from pipecat.workers.runner import WorkerRunner

from moq_ptz_tracking.ptz import PtzCalibration, PtzCommands
from moq_ptz_tracking.tracker import CAMERA, COMMAND_TRACK, EVENT_TIMELINE_TRACK, PROMPT_TRACK, VIEWER, PtzTracker
from moq_ptz_tracking.vision import SystemOneVisionClient

STATUS_TRACK = "status"
# What the bot publishes is kept for FETCH as long as the bot service lets a room's bot run.
RETENTION_SECONDS = 60 * 60
# The page publishes the viewer as `<room>/page/viewer` (VIEWER); onvif-ingest publishes the camera
# as `<room>/onvif/client` (CAMERA) and follows the commands the tracker publishes at
# `<room>/bots/ptz-tracking/command`. The two rooms are siblings: one session cannot follow a
# namespace prefix inside another it follows.
PAGE_ROOM = "page"
CAMERA_ROOM = "onvif"
COMMAND_BROADCAST = "command"

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Pan and tilt the ONVIF camera so every box of the MoQ PTZ tracking page's targets stays in the picture"
    )
    parser.set_defaults(bot="moq-ptz-tracking")
    add_bot_job_arguments(parser)
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


async def track_in_session(
    args: argparse.Namespace,
    http: aiohttp.ClientSession,
    identity_token: IdentityToken | None,
    job: BotJob,
    session: moq_wasm.Session,
) -> None:
    """Returns once no one has published in the page's room for the grace period."""
    # The tracker takes part in two rooms: the page's, where it reads the viewer's prompt and writes
    # its event timeline and status, and onvif-ingest's, where it watches the camera and writes the
    # camera's commands.
    page = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=[*job.namespace_prefix, PAGE_ROOM],
            broadcast=job.broadcast,
            data_in_tracks=[PROMPT_TRACK],
            event_timelines_out={EVENT_TIMELINE_TRACK: RETENTION_SECONDS},
            data_out_tracks={STATUS_TRACK: RETENTION_SECONDS},
            empty_timeout=job.grace_seconds,
        ),
    )
    onvif = MoqWasmTransport(
        session,
        MoqWasmParams(
            room=[*job.namespace_prefix, CAMERA_ROOM],
            broadcast=[*job.broadcast, COMMAND_BROADCAST],
            video_in_enabled=True,
            data_out_tracks={COMMAND_TRACK: RETENTION_SECONDS},
        ),
    )
    status = SystemOneStatus()
    commands = PtzCommands()
    vision = SystemOneVisionClient(http, args.system_one_url, status, identity_token)
    calibration = PtzCalibration(args.pan_per_second, args.tilt_per_second, args.margin, args.dead_zone)
    tracker = PtzTracker(vision, commands, calibration, partial(onvif.release_participant_video, CAMERA))
    # The camera's input sits right before the tracker, so the tracker's image requests reach it first.
    worker = PipelineWorker(
        Pipeline([page.input(), onvif.input(), tracker, onvif.output(), page.output()]),
        enable_rtvi=False,
        idle_timeout_secs=None,
    )
    # serve_bot_job owns SIGINT and SIGTERM.
    runner = WorkerRunner(handle_sigint=False)

    @page.event_handler("on_client_disconnected")
    async def on_client_disconnected(_, participant_id: str):
        if participant_id == VIEWER:
            await tracker.set_target(None)

    @page.event_handler("on_room_empty")
    async def on_room_empty(_):
        await runner.cancel()

    await runner.add_workers(worker)
    commands.publish_on(tracker.command_writer())
    await status.publish_on(PipelineTrack(worker, STATUS_TRACK))
    await runner.run()


async def track_in_room(args: argparse.Namespace, job: BotJob, bots: Bots) -> None:
    async with aiohttp.ClientSession(timeout=REQUEST_TIMEOUT) as http:
        identity_token = identity_token_from_arguments(args, http, args.system_one_url)
        await stay_connected(args, bots, partial(track_in_session, args, http, identity_token, job))


async def main():
    args = parse_args()
    await serve_bot_job(args, partial(track_in_room, args))


if __name__ == "__main__":
    asyncio.run(main())
