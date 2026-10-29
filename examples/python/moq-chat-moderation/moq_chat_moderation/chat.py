import json
from dataclasses import dataclass

import moqt
from pipecat.frames.frames import DataFrame

Location = tuple[int, int]

MAX_LABELS = 16
MAX_LABEL_NAME_CHARS = 20
MAX_LABEL_DESCRIPTION_CHARS = 100


@dataclass(frozen=True)
class Label:
    name: str
    description: str


@dataclass
class ChatMessage(DataFrame):
    """A chat object the page sent; a verdict names it by its `location`."""

    text: str
    labels: tuple[Label, ...]
    location: Location

    @classmethod
    def parse(cls, chat_object: moqt.Object) -> "ChatMessage":
        """Raises ValueError for a payload that is not a valid `{"text", "labels": [{"name", "description"}]}`
        within the length and count limits."""
        body = json.loads(chat_object.payload)
        if not isinstance(body, dict):
            raise ValueError("chat is not an object")
        text = body.get("text")
        if not isinstance(text, str) or not text:
            raise ValueError("text is not a non-empty string")
        labels = body.get("labels")
        if not isinstance(labels, list):
            raise ValueError("labels is not a list")
        if len(labels) > MAX_LABELS:
            raise ValueError(f"more than {MAX_LABELS} labels")
        parsed = tuple(_parse_label(label) for label in labels)
        if len({label.name for label in parsed}) < len(parsed):
            raise ValueError("two labels share a name")
        return cls(text=text, labels=parsed, location=(chat_object.group_id, chat_object.object_id))


def _parse_label(label: object) -> Label:
    if not isinstance(label, dict):
        raise ValueError("a label is not an object")
    name = label.get("name")
    description = label.get("description")
    if not isinstance(name, str) or not 0 < len(name) <= MAX_LABEL_NAME_CHARS:
        raise ValueError(f"a label name is not 1 to {MAX_LABEL_NAME_CHARS} characters")
    if not isinstance(description, str) or len(description) > MAX_LABEL_DESCRIPTION_CHARS:
        raise ValueError(f"a label description is not up to {MAX_LABEL_DESCRIPTION_CHARS} characters")
    return Label(name, description)
