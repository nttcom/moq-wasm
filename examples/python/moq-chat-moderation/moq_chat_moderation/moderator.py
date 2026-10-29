import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.system_one import SystemOneClient
from moq_chat_moderation.event_timeline import EventTimeline, location_record

ABUSIVE_THRESHOLD = 0.5


class ChatModerator(FrameProcessor):
    def __init__(self, system_one: SystemOneClient, timeline: EventTimeline):
        super().__init__()
        self._system_one = system_one
        self._timeline = timeline

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, ChatMessage):
            await self._moderate(frame)
        else:
            await self.push_frame(frame, direction)

    async def _moderate(self, chat: ChatMessage):
        try:
            probability = await self._system_one.abusive_probability(chat.text)
        except (aiohttp.ClientError, KeyError, ValueError) as error:
            logger.warning(f"System One could not judge the chat message at {chat.location}: {error}")
            return
        abusive = probability >= ABUSIVE_THRESHOLD
        logger.info(f"chat {chat.location} abusive={abusive} (p={probability:.2f})")
        await self._timeline.append(
            location_record(chat.location, {"abusive": abusive, "probability": probability})
        )
