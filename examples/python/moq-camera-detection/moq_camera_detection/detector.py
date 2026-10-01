import asyncio
import time
from dataclasses import dataclass

import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, SystemFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_camera_detection.event_timeline import EventTimeline, location_record
from moq_camera_detection.vision import DjevVisionClient

STALE_PICTURE_SECONDS = 2.0
MAX_JUDGING_PICTURES = 8


@dataclass
class CameraPicture(SystemFrame):
    jpeg: bytes
    location: tuple[int, int]
    decoded_at: float


class PersonDetector(FrameProcessor):
    def __init__(self, vision: DjevVisionClient, timeline: EventTimeline):
        super().__init__()
        self._vision = vision
        self._timeline = timeline
        self._judging: set[asyncio.Task] = set()
        self._latest_recorded = (-1, -1)

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if not isinstance(frame, CameraPicture):
            await self.push_frame(frame, direction)
            return
        if time.monotonic() - frame.decoded_at > STALE_PICTURE_SECONDS:
            return
        if len(self._judging) >= MAX_JUDGING_PICTURES:
            logger.debug(f"djev-vision is busy, skipping camera picture {frame.location}")
            return
        task = self.create_task(self._judge(frame))
        self._judging.add(task)
        task.add_done_callback(self._judging.discard)

    async def cleanup(self):
        await super().cleanup()
        for task in list(self._judging):
            await self.cancel_task(task)

    async def _judge(self, picture: CameraPicture):
        requested_at = time.monotonic()
        try:
            person = await self._vision.has_person(picture.jpeg)
        except (aiohttp.ClientError, KeyError, IndexError) as error:
            logger.warning(f"djev-vision could not judge camera picture {picture.location}: {error}")
            return
        logger.info(
            f"camera picture {picture.location} person={person}"
            f" in {time.monotonic() - requested_at:.2f}s"
        )
        if picture.location <= self._latest_recorded:
            return
        self._latest_recorded = picture.location
        self._timeline.append(location_record(picture.location, {"person": person}))
