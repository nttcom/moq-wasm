import time
from dataclasses import dataclass

import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, SystemFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_camera_detection.event_timeline import EventTimeline, location_record
from moq_camera_detection.vision import DjevVisionClient

STALE_KEYFRAME_SECONDS = 2.0


@dataclass
class CameraKeyframe(SystemFrame):
    jpeg: bytes
    group_id: int
    decoded_at: float


class PersonDetector(FrameProcessor):
    def __init__(self, vision: DjevVisionClient, timeline: EventTimeline):
        super().__init__()
        self._vision = vision
        self._timeline = timeline

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if not isinstance(frame, CameraKeyframe):
            await self.push_frame(frame, direction)
            return
        if time.monotonic() - frame.decoded_at > STALE_KEYFRAME_SECONDS:
            return
        try:
            person = await self._vision.has_person(frame.jpeg)
        except (aiohttp.ClientError, KeyError, IndexError) as error:
            logger.warning(f"djev-vision could not judge group {frame.group_id}: {error}")
            return
        logger.info(f"camera group {frame.group_id} person={person}")
        self._timeline.append(location_record((frame.group_id, 0), {"person": person}))
