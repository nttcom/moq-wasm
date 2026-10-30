import aiohttp
from loguru import logger
from pipecat.frames.frames import Frame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.system_one import SystemOneClient
from moq_system_one_bot.event_timeline import EventTimeline, location_record


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
        if not chat.labels:
            return
        try:
            probabilities = await self._system_one.label_probabilities(chat.text, chat.labels)
        except (aiohttp.ClientError, KeyError, ValueError) as error:
            logger.warning(f"System One could not judge the chat message at {chat.location}: {error}")
            return
        labels = [
            {"name": label.name, "probability": probability}
            for label, probability in zip(chat.labels, probabilities)
        ]
        logger.info(f"chat {chat.location} labels={labels}")
        await self._timeline.append(location_record(chat.location, {"labels": labels}))
