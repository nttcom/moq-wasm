import asyncio
import time

import aiohttp
from loguru import logger
from moq_system_one_bot.event_timeline import Location, location_record
from moq_system_one_bot.video import rgb_to_jpeg
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage
from pipecat.frames.frames import (
    Frame,
    InputTransportMessageFrame,
    StartFrame,
    OutputTransportMessageFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_camera_detection.prompt import Prompt, parse_prompt
from moq_camera_detection.vision import SystemOneVisionClient

CAMERA = "camera"
PROMPT_TRACK = "prompt"
EVENT_TIMELINE_TRACK = "eventtimeline"
# A picture is asked for every SAMPLE_INTERVAL_SECONDS while a prompt is set, no request is pending
# and fewer than MAX_JUDGING_PICTURES are being judged.
SAMPLE_INTERVAL_SECONDS = 0.1
MAX_JUDGING_PICTURES = 8


class CameraDetector(FrameProcessor):
    """Answers the camera page's latest prompt about the camera's pictures. Reads the prompt from
    the camera's prompt track, asks the input for a picture with `UserImageRequestFrame`, and
    records each answer at the picture's Location on the event timeline."""

    def __init__(self, vision: SystemOneVisionClient):
        super().__init__()
        self._vision = vision
        self._prompt: Prompt | None = None
        self._requesting = False
        self._judging: set[asyncio.Task] = set()
        self._latest_recorded = (-1, -1)
        self._sampling: asyncio.Task | None = None

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, InputTransportMessageFrame) and isinstance(frame.message, MoqWasmTrackMessage):
            self._read_prompt(frame.message)
        elif isinstance(frame, UserImageRawFrame) and frame.user_id == CAMERA and frame.request is not None:
            self._requesting = False
            self._judge_later(frame)
        else:
            await self.push_frame(frame, direction)
        if isinstance(frame, StartFrame) and self._sampling is None:
            self._sampling = self.create_task(self._sample())

    async def cleanup(self):
        await super().cleanup()
        if self._sampling is not None:
            await self.cancel_task(self._sampling)
        for task in list(self._judging):
            await self.cancel_task(task)

    def _read_prompt(self, message: MoqWasmTrackMessage) -> None:
        if message.participant_id != CAMERA or message.track != PROMPT_TRACK:
            return
        location = (message.location.group_id, message.location.object_id)
        prompt = parse_prompt(message.payload, location)
        if prompt is None:
            logger.warning(f"ignoring an invalid prompt at {location}")
            return
        logger.info(f"prompt {prompt.location}: {prompt.questions}")
        self._prompt = prompt

    async def _sample(self) -> None:
        while True:
            await asyncio.sleep(SAMPLE_INTERVAL_SECONDS)
            if self._prompt is None or self._requesting or len(self._judging) >= MAX_JUDGING_PICTURES:
                continue
            self._requesting = True
            await self.push_frame(
                UserImageRequestFrame(user_id=CAMERA, result_callback=self._request_failed), FrameDirection.UPSTREAM
            )

    def camera_left(self) -> None:
        """Stops asking for pictures; the camera's next prompt starts afresh."""
        self._prompt = None

    async def _request_failed(self, result: dict) -> None:
        """No picture answers the request, as while the camera is paused; the next one is asked for
        at the next sample."""
        logger.info(f"no camera picture: {result.get('error')}")
        self._requesting = False

    def _judge_later(self, frame: UserImageRawFrame) -> None:
        if self._prompt is None:
            return
        task = self.create_task(self._judge(frame, self._prompt))
        self._judging.add(task)
        task.add_done_callback(self._judging.discard)

    async def _judge(self, frame: UserImageRawFrame, prompt: Prompt) -> None:
        location: Location = (frame.metadata["location"].group_id, frame.metadata["location"].object_id)
        jpeg = await asyncio.to_thread(rgb_to_jpeg, frame.image, frame.size)
        requested_at = time.monotonic()
        try:
            answers = await self._vision.answer(jpeg, prompt)
        except (aiohttp.ClientError, KeyError, IndexError, TypeError) as error:
            logger.warning(f"System One could not judge camera picture {location}: {error}")
            return
        logger.info(f"camera picture {location} answers={answers} in {time.monotonic() - requested_at:.2f}s")
        if location <= self._latest_recorded:
            return
        self._latest_recorded = location
        record = location_record(location, {"prompt": prompt.location, "answers": answers})
        await self.push_frame(OutputTransportMessageFrame(message=MoqWasmEventRecord(EVENT_TIMELINE_TRACK, record)))

