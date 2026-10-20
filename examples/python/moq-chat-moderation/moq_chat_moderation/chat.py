from dataclasses import dataclass

import moqt
from pipecat.frames.frames import DataFrame

Location = tuple[int, int]


@dataclass
class ChatMessage(DataFrame):
    """A chat object the page sent: its UTF-8 text, and the Location a verdict names it by."""

    text: str
    location: Location

    @classmethod
    def parse(cls, chat_object: moqt.Object) -> "ChatMessage | None":
        try:
            text = chat_object.payload.decode()
        except UnicodeDecodeError:
            return None
        if not text:
            return None
        return cls(text=text, location=(chat_object.group_id, chat_object.object_id))
