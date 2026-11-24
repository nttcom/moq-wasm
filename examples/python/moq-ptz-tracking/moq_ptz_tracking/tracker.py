import asyncio
import time
from collections.abc import Awaitable, Callable
from functools import partial

import aiohttp
from loguru import logger
from moq_system_one_bot.event_timeline import Location, location_record
from moq_system_one_bot.video import rgb_to_jpeg
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage, MoqWasmTrackObject
from pipecat.frames.frames import (
    CancelFrame,
    EndFrame,
    Frame,
    InputTransportMessageFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    StartFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_ptz_tracking.pan_planner import Home, PanPlanner, Sweeping
from moq_ptz_tracking.ptz import CENTER_SECONDS, Heading, PtzCalibration, PtzCommands, PtzMove, Sweep
from moq_ptz_tracking.target import Target, parse_prompt
from moq_ptz_tracking.vision import Answer, Box, Sighting, SystemOneVisionClient

# The participant ids of ingest-gateway onvif's camera and the PTZ tracking page, the last elements of their
# namespaces.
CAMERA = "client"
VIEWER = "viewer"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
COMMAND_TRACK = "command"
# The camera stops within 0.3 s of a Stop command, and a picture reaches the bot about 0.4 s after
# the camera captured it.
SETTLE_SECONDS = 1.0
SAMPLE_INTERVAL_SECONDS = 0.1
# How long the tracker waits to ask again after a request no picture answered, as while the camera
# is not published.
RETRY_SECONDS = 1.0
JPEG_WIDTH = 640
NO_MOVE = PtzMove(0.0, 0.0)


class PtzTracker(FrameProcessor):
    """Locates the viewer's targets in one camera picture at a time and turns the camera toward them.
    Reads the target from the viewer's prompt track and asks for a picture of the profile it names
    with `UserImageRequestFrame` every SAMPLE_INTERVAL_SECONDS while none is being located. After a
    step it asks for none until the camera has settled, so a step never reacts to a mid-turn
    position; while sweeping it locates every picture it gets and stops on sight.

    `release_video` stops receiving a profile of the camera, as when the target changes, because
    ingest-gateway onvif streams the profile its first subscriber picks and rejects the others. The camera's
    commands go out through `command_writer()`, so the Stop of a turn reaches the camera even as the
    pipeline ends."""

    def __init__(
        self,
        vision: SystemOneVisionClient,
        commands: PtzCommands,
        calibration: PtzCalibration,
        release_video: Callable[[str], Awaitable[None]],
    ):
        super().__init__()
        self._vision = vision
        self._commands = commands
        self._release_video = release_video
        self._target: Target | None = None
        self._requesting = False
        self._locating: asyncio.Task | None = None
        self._sampling: asyncio.Task | None = None
        self._stopped_at = float("-inf")
        self._retry_at = float("-inf")
        self._planner = PanPlanner(calibration)
        self._sweep: Sweep | None = None

    def command_writer(self) -> "_CommandWriter":
        """Writes ingest-gateway onvif's command track through this tracker, for `PtzCommands.publish_on`."""
        return _CommandWriter(self)

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, (EndFrame, CancelFrame)):
            # Stops the camera before the frame closes the transports: ContinuousMove has no timeout.
            if self._locating is not None:
                await self.cancel_task(self._locating)
            await self._stop_sweep()
        if isinstance(frame, InputTransportMessageFrame) and isinstance(frame.message, MoqWasmTrackMessage):
            await self._read_prompt(frame.message)
        elif isinstance(frame, UserImageRawFrame) and frame.user_id == CAMERA and frame.request is not None:
            self._requesting = False
            self._locate_later(frame)
        else:
            await self.push_frame(frame, direction)
        if isinstance(frame, StartFrame) and self._sampling is None:
            self._sampling = self.create_task(self._sample())

    async def cleanup(self):
        await super().cleanup()
        if self._sampling is not None:
            await self.cancel_task(self._sampling)
        if self._locating is not None:
            await self.cancel_task(self._locating)
        await self._stop_sweep()

    async def set_target(self, target: Target | None) -> None:
        """Tracks `target`, or nothing when it is None, as when the viewer leaves."""
        previous, self._target = self._target, target
        if previous is not None and (target is None or target.video_track != previous.video_track):
            await self._release_video(previous.video_track)

    async def _read_prompt(self, message: MoqWasmTrackMessage) -> None:
        if message.participant_id != VIEWER or message.track != PROMPT_TRACK:
            return
        location = (message.location.group_id, message.location.object_id)
        try:
            target = parse_prompt(message.payload, location)
        except ValueError as error:
            logger.warning(f"ignoring an invalid prompt at {location}: {error}")
            return
        logger.info(f"prompt {location}: {target}")
        await self.set_target(target)

    async def _sample(self) -> None:
        while True:
            await asyncio.sleep(SAMPLE_INTERVAL_SECONDS)
            now = time.monotonic()
            settling = self._sweep is None and now - self._stopped_at < SETTLE_SECONDS
            waiting = self._requesting or self._locating is not None or now < self._retry_at
            if self._target is None or waiting or settling:
                continue
            self._requesting = True
            track = self._target.video_track
            request = UserImageRequestFrame(
                user_id=CAMERA, video_source=track, result_callback=partial(self._request_failed, track)
            )
            await self.push_frame(request, FrameDirection.UPSTREAM)

    async def _request_failed(self, track: str, result: dict) -> None:
        logger.info(f"no camera picture: {result.get('error')}")
        self._requesting = False
        # A request for a profile the target left was released; the new profile is asked for at once.
        if self._target is not None and self._target.video_track == track:
            self._retry_at = time.monotonic() + RETRY_SECONDS

    def _locate_later(self, frame: UserImageRawFrame) -> None:
        target = self._target
        if self._locating is not None or target is None or frame.request.video_source != target.video_track:
            return
        self._locating = self.create_task(self._locate(frame, target))

    async def _locate(self, frame: UserImageRawFrame, target: Target):
        location: Location = (frame.metadata["location"].group_id, frame.metadata["location"].object_id)
        try:
            jpeg = await asyncio.to_thread(rgb_to_jpeg, frame.image, frame.size, JPEG_WIDTH)
            requested_at = time.monotonic()
            try:
                answer = await self._vision.locate(jpeg, target.descriptions)
            except (aiohttp.ClientError, KeyError, IndexError, TypeError) as error:
                logger.warning(f"System One could not locate the targets in {location}: {error}")
                return
            seconds = time.monotonic() - requested_at
            logger.info(f"camera picture {location} {boxes_record(answer)} in {seconds:.2f}s")
            if self._target != target:
                await self._stop_sweep()
                return
            decision = self._planner.decide(answer, time.monotonic())
            move = decision if isinstance(decision, PtzMove) else NO_MOVE
            record = location_record(
                location,
                {
                    "prompt": target.location,
                    "boxes": boxes_record(answer),
                    "pan_seconds": move.pan_seconds,
                    "tilt_seconds": move.tilt_seconds,
                    "sweep": [decision.heading.pan, decision.heading.tilt] if isinstance(decision, Sweeping) else None,
                },
            )
            await self.push_frame(OutputTransportMessageFrame(message=MoqWasmEventRecord(EVENT_TIMELINE_TRACK, record)))
            if isinstance(decision, Sweeping):
                await self._sweep_toward(decision.heading)
                return
            await self._stop_sweep()
            if isinstance(decision, Home):
                await self._commands.center()
                self._stopped_at = time.monotonic() + CENTER_SECONDS
            elif move != NO_MOVE:
                await self._commands.move(move)
                self._stopped_at = time.monotonic()
        finally:
            self._locating = None

    async def _sweep_toward(self, heading: Heading) -> None:
        if self._sweep is not None and self._sweep.turning and self._sweep.heading == heading:
            self._sweep.renew()
            return
        await self._stop_sweep()
        self._sweep = await self._commands.sweep(heading)

    async def _stop_sweep(self) -> None:
        if self._sweep is None:
            return
        await self._sweep.stop()
        self._sweep = None
        self._stopped_at = time.monotonic()


class _CommandWriter:
    """Pushes each command as an urgent transport message, which the output writes as it arrives,
    ahead of queued frames and before an `EndFrame` or `CancelFrame` closes it."""

    def __init__(self, tracker: PtzTracker):
        self._tracker = tracker

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        message = MoqWasmTrackObject(COMMAND_TRACK, payload, new_group)
        await self._tracker.push_frame(OutputTransportMessageUrgentFrame(message=message))


def boxes_record(answer: Sighting | Answer) -> list | str:
    """`[[target index, [left, top, right, bottom]], ...]`, or "not visible"."""
    if isinstance(answer, Sighting):
        return [[found.target, edges_record(found.box)] for found in answer.boxes]
    return answer.value


def edges_record(box: Box) -> list[float]:
    return [round(edge, 3) for edge in (box.left, box.top, box.right, box.bottom)]
