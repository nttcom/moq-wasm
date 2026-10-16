import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame, InputTransportMessageFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.d3_flash import D3FlashClient
from moq_chat_moderation.event_timeline import EventTimeline, location_record

ABUSIVE_THRESHOLD = 0.5


class ChatModerator(FrameProcessor):
    def __init__(self, d3_flash: D3FlashClient, timeline: EventTimeline):
        super().__init__()
        self._d3_flash = d3_flash
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
            probability = await self._d3_flash.abusive_probability(chat.text)
        except (aiohttp.ClientError, KeyError, ValueError) as error:
            logger.warning(f"d3-flash could not judge the chat message at {chat.location}: {error}")
            return
        abusive = probability >= ABUSIVE_THRESHOLD
        logger.info(f"chat {chat.location} abusive={abusive} (p={probability:.2f})")
        self._timeline.append(
            location_record(chat.location, {"abusive": abusive, "probability": probability})
        )
