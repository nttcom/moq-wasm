import asyncio

import aiohttp
from loguru import logger
from moq_system_one_bot.event_timeline import location_record
from moq_wasm.pipecat import MoqWasmEventRecord, MoqWasmTrackMessage
from pipecat.frames.frames import Frame, InputTransportMessageFrame, OutputTransportMessageFrame, StartFrame
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from moq_chat_moderation.chat import ChatMessage
from moq_chat_moderation.system_one import SystemOneClient

# CHAT is both the chat page's participant id, the last element of its namespace, and the name of
# its track.
CHAT = "chat"
EVENT_TIMELINE_TRACK = "eventtimeline"


class ChatModerator(FrameProcessor):
    """Judges each message of the chat page's chat track against the labels it carries and records
    the probabilities at the message's Location on the event timeline."""

    def __init__(self, system_one: SystemOneClient):
        super().__init__()
        self._system_one = system_one
        self._chats: asyncio.Queue[ChatMessage] = asyncio.Queue()
        self._moderating: asyncio.Task | None = None

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        await super().process_frame(frame, direction)
        if isinstance(frame, InputTransportMessageFrame) and isinstance(frame.message, MoqWasmTrackMessage):
            self._read_chat(frame.message)
        else:
            await self.push_frame(frame, direction)
        if isinstance(frame, StartFrame) and self._moderating is None:
            self._moderating = self.create_task(self._moderate_each())

    async def cleanup(self):
        await super().cleanup()
        if self._moderating is not None:
            await self.cancel_task(self._moderating)

    def _read_chat(self, message: MoqWasmTrackMessage) -> None:
        if message.participant_id != CHAT or message.track != CHAT:
            return
        location = (message.location.group_id, message.location.object_id)
        try:
            chat = ChatMessage.parse(message.payload, location)
        except ValueError as error:
            logger.warning(f"ignoring an invalid chat object at {location}: {error}")
            return
        self._chats.put_nowait(chat)

    async def _moderate_each(self) -> None:
        """Judges the chats one at a time outside `process_frame`: an `InputTransportMessageFrame` is a
        system frame, so awaiting System One there would hold up the frames behind it, such as the
        System One status a late answer writes."""
        while True:
            chat = await self._chats.get()
            try:
                await self._moderate(chat)
            except Exception:
                logger.exception(f"moderating the chat message at {chat.location} failed")

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
        record = location_record(chat.location, {"labels": labels})
        await self.push_frame(OutputTransportMessageFrame(message=MoqWasmEventRecord(EVENT_TIMELINE_TRACK, record)))
