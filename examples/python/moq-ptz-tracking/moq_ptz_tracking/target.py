import json
from dataclasses import dataclass

from moq_system_one_bot.event_timeline import Location

MAX_DESCRIPTIONS = 16
MAX_DESCRIPTION_CHARS = 100


@dataclass(frozen=True)
class Target:
    """What the page asked to keep in the picture: every instance of each description."""

    descriptions: tuple[str, ...]
    video_track: str
    location: Location


def parse_prompt(payload: bytes, location: Location) -> Target | None:
    """Returns None for a prompt that stops tracking and raises ValueError for an invalid one."""
    body = json.loads(payload)
    if not isinstance(body, dict):
        raise ValueError("prompt is not an object")
    descriptions = body.get("targets")
    if not isinstance(descriptions, list):
        raise ValueError("targets is not a list")
    if not descriptions:
        return None
    if not all(isinstance(text, str) and 0 < len(text) <= MAX_DESCRIPTION_CHARS for text in descriptions):
        raise ValueError("a target is not a short text")
    unique = tuple(dict.fromkeys(descriptions))
    if len(unique) > MAX_DESCRIPTIONS:
        raise ValueError(f"more than {MAX_DESCRIPTIONS} targets")
    video_track = body.get("video")
    if not isinstance(video_track, str) or not video_track:
        raise ValueError("video is not a track name")
    return Target(unique, video_track, location)

