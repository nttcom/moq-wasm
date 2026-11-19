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

    @property
    def current(self) -> str | None:
        return self._participants[0] if self._participants else None

    def joined(self, participant_id: str) -> bool:
        """Returns whether the room had nobody else, which makes this participant the one the bot listens to."""
        self._participants.append(participant_id)
        return len(self._participants) == 1

    def left(self, participant_id: str) -> None:
        if participant_id in self._participants:
            self._participants.remove(participant_id)

    async def accepts(self, frame: Frame) -> bool:
        return not isinstance(frame, UserAudioRawFrame) or frame.user_id == self.current
