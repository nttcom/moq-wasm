import asyncio
import json
from dataclasses import dataclass

from moq_ptz_tracking.event_timeline import Location

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


class LatestTarget:
    def __init__(self):
        self.target: Target | None = None
        self._changed = asyncio.Event()

    def set(self, target: Target | None) -> None:
        self.target = target
        self._changed.set()
        self._changed = asyncio.Event()

    async def changed(self) -> None:
        await self._changed.wait()

    async def leaves(self, video_track: str) -> None:
        while self.target is not None and self.target.video_track == video_track:
            await self.changed()
