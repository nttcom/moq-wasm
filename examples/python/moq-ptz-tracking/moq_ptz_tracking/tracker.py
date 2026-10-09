import asyncio
import time
from dataclasses import dataclass

import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, SystemFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_ptz_tracking.event_timeline import EventTimeline, Location, location_record
from moq_ptz_tracking.ptz import PanTilt, PtzCommands, PtzSteps
from moq_ptz_tracking.target import LatestTarget, Target
from moq_ptz_tracking.vision import DjevVisionClient

STALE_PICTURE_SECONDS = 2.0
SETTLE_SECONDS = 1.5
NO_MOVE = PanTilt(0.0, 0.0)


@dataclass
class CameraPicture(SystemFrame):
    jpeg: bytes
    location: Location
    decoded_at: float
    target: Target


class PtzTracker(FrameProcessor):
    """Locates the target in one picture at a time and only in pictures decoded after the camera
    stopped moving, so a move never reacts to where the target was before the previous move."""

    def __init__(
        self,
        vision: DjevVisionClient,
        timeline: EventTimeline,
        commands: PtzCommands,
        steps: PtzSteps,
        latest_target: LatestTarget,
    ):
        super().__init__()
        self._vision = vision
        self._timeline = timeline
        self._commands = commands
        self._steps = steps
        self._latest_target = latest_target
        self._locating: asyncio.Task | None = None
        self._settled_at = 0.0

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if not isinstance(frame, CameraPicture):
            await self.push_frame(frame, direction)
            return
        if self._locating is not None or frame.decoded_at < self._settled_at:
            return
        if time.monotonic() - frame.decoded_at > STALE_PICTURE_SECONDS:
            return
        self._locating = self.create_task(self._locate(frame))

    async def cleanup(self):
        await super().cleanup()
        if self._locating is not None:
            await self.cancel_task(self._locating)

    async def _locate(self, picture: CameraPicture):
        requested_at = time.monotonic()
        try:
            position = await self._vision.locate(picture.jpeg, picture.target.description)
        except (aiohttp.ClientError, KeyError, IndexError) as error:
            logger.warning(f"djev-vision could not locate the target in {picture.location}: {error}")
            return
        finally:
            self._locating = None
        logger.info(
            f"camera picture {picture.location} position={position}"
            f" in {time.monotonic() - requested_at:.2f}s"
        )
        if self._latest_target.target != picture.target:
            return
        move = self._steps.toward(position) if position else NO_MOVE
        if move != NO_MOVE:
            self._commands.relative_move(move)
            self._settled_at = time.monotonic() + SETTLE_SECONDS
        self._timeline.append(
            location_record(
                picture.location,
                {"prompt": picture.target.location, "position": position, "pan": move.pan, "tilt": move.tilt},
            )
        )
