import asyncio
import time
from dataclasses import dataclass

import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, SystemFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_ptz_tracking.event_timeline import EventTimeline, Location, location_record
from moq_ptz_tracking.pan_planner import Home, PanPlanner, Sweeping
from moq_ptz_tracking.ptz import CENTER_SECONDS, Heading, PtzCalibration, PtzCommands, PtzMove, Sweep
from moq_ptz_tracking.target import LatestTarget, Target
from moq_ptz_tracking.vision import Answer, DjevVisionClient, Position

# The camera stops within 0.3 s of a Stop command, and a picture reaches the bot about 0.4 s after
# the camera captured it.
SETTLE_SECONDS = 1.0
NO_MOVE = PtzMove(0.0, 0.0)


@dataclass
class CameraPicture(SystemFrame):
    jpeg: bytes
    location: Location
    decoded_at: float
    target: Target


class PtzTracker(FrameProcessor):
    """Locates the target in one picture at a time. After a step it only uses pictures decoded once the
    camera has settled, so a step never reacts to a mid-turn position; while sweeping it locates every
    picture and stops on sight."""

    def __init__(
        self,
        vision: DjevVisionClient,
        timeline: EventTimeline,
        commands: PtzCommands,
        calibration: PtzCalibration,
        latest_target: LatestTarget,
    ):
        super().__init__()
        self._vision = vision
        self._timeline = timeline
        self._commands = commands
        self._latest_target = latest_target
        self._locating: asyncio.Task | None = None
        self._stopped_at = float("-inf")
        self._planner = PanPlanner(calibration)
        self._sweep: Sweep | None = None

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if not isinstance(frame, CameraPicture):
            await self.push_frame(frame, direction)
            return
        settling = self._sweep is None and frame.decoded_at - self._stopped_at < SETTLE_SECONDS
        if self._locating is not None or settling:
            return
        self._locating = self.create_task(self._locate(frame))

    async def cleanup(self):
        await super().cleanup()
        if self._locating is not None:
            await self.cancel_task(self._locating)
        await self._stop_sweep()

    async def _locate(self, picture: CameraPicture):
        try:
            requested_at = time.monotonic()
            try:
                answer = await self._vision.locate(picture.jpeg, picture.target.description)
            except (aiohttp.ClientError, KeyError, IndexError) as error:
                logger.warning(f"djev-vision could not locate the target in {picture.location}: {error}")
                return
            logger.info(f"camera picture {picture.location} {answer} in {time.monotonic() - requested_at:.2f}s")
            if self._latest_target.target != picture.target:
                await self._stop_sweep()
                return
            decision = self._planner.decide(answer, time.monotonic())
            move = decision if isinstance(decision, PtzMove) else NO_MOVE
            self._timeline.append(
                location_record(
                    picture.location,
                    {
                        "prompt": picture.target.location,
                        "position": position_record(answer),
                        "pan_seconds": move.pan_seconds,
                        "tilt_seconds": move.tilt_seconds,
                        "sweep": [decision.heading.pan, decision.heading.tilt]
                        if isinstance(decision, Sweeping)
                        else None,
                    },
                )
            )
            if isinstance(decision, Sweeping):
                await self._sweep_toward(decision.heading)
                return
            await self._stop_sweep()
            if isinstance(decision, Home):
                self._commands.center()
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
        self._sweep = self._commands.sweep(heading)

    async def _stop_sweep(self) -> None:
        if self._sweep is None:
            return
        await self._sweep.stop()
        self._sweep = None
        self._stopped_at = time.monotonic()


def position_record(answer: Position | Answer) -> list[float] | str:
    if isinstance(answer, Position):
        return [round(answer.x, 3), round(answer.y, 3)]
    return answer.value
