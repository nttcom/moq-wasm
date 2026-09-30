from dataclasses import dataclass
from typing import Any

Location = tuple[int, int]


@dataclass(frozen=True)
class ChatMessage:
    text: str
    location: Location

    @classmethod
    def parse(cls, message: Any) -> "ChatMessage | None":
        if not isinstance(message, dict):
            return None
        text = message.get("text")
        location = message.get("location")
        if not isinstance(text, str) or not _is_location(location):
            return None
        return cls(text=text, location=(location[0], location[1]))


def _is_location(value: Any) -> bool:
    return (
        isinstance(value, list)
        and len(value) == 2
        and all(isinstance(element, int) and not isinstance(element, bool) for element in value)
    )
