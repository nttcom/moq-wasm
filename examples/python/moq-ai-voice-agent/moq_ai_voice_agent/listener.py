import asyncio

from pipecat.frames.frames import Frame, UserAudioRawFrame


class Listener:
    """Picks the one participant the bot listens to: the earliest of those still in the room. The
    transport hands every participant's audio to the pipeline, and one speech-to-text stream cannot
    tell interleaved speakers apart.

    It learns the participants from the transport's `on_client_connected` and
    `on_client_disconnected` rather than watching the room itself: the relay refuses a second
    SUBSCRIBE_NAMESPACE of the room on the session the transport already watches it on."""

    def __init__(self):
        self._participants: list[str] = []
        self._changed = asyncio.Event()

    @property
    def current(self) -> str | None:
        return self._participants[0] if self._participants else None

    def joined(self, participant_id: str) -> None:
        self._participants.append(participant_id)
        self._changed.set()

    def left(self, participant_id: str) -> None:
        if participant_id in self._participants:
            self._participants.remove(participant_id)
        self._changed.set()

    async def until_empty_for(self, seconds: float) -> None:
        """Returns once nobody has been in the room for `seconds`, counted from the call or from
        the moment the last participant left."""
        while True:
            self._changed.clear()
            if self._participants:
                await self._changed.wait()
                continue
            try:
                await asyncio.wait_for(self._changed.wait(), seconds)
            except TimeoutError:
                return

    async def accepts(self, frame: Frame) -> bool:
        return not isinstance(frame, UserAudioRawFrame) or frame.user_id == self.current
