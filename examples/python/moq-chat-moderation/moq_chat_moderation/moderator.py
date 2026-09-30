import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, InputTransportMessageFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.event_timeline import EventTimeline, location_record
from moq_chat_moderation.jev import JevClient

ABUSIVE_THRESHOLD = 0.5


class ChatModerator(FrameProcessor):
    def __init__(self, jev: JevClient, timeline: EventTimeline):
        super().__init__()
        self._jev = jev
        self._timeline = timeline

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        chat = (
            ChatMessage.parse(frame.message)
            if isinstance(frame, InputTransportMessageFrame)
            else None
        )
        if chat is None:
            await self.push_frame(frame, direction)
            return
        await self._moderate(chat)

    async def _moderate(self, chat: ChatMessage):
        try:
            probability = await self._jev.abusive_probability(chat.text)
        except (aiohttp.ClientError, KeyError, ValueError) as error:
            logger.warning(f"Jev could not judge the chat message at {chat.location}: {error}")
            return
        abusive = probability >= ABUSIVE_THRESHOLD
        logger.info(f"chat {chat.location} abusive={abusive} (p={probability:.2f})")
        self._timeline.append(
            location_record(chat.location, {"abusive": abusive, "probability": probability})
        )
