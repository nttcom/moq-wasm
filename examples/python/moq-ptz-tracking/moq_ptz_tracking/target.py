import asyncio
import json
from dataclasses import dataclass

from moq_ptz_tracking.event_timeline import Location

MAX_DESCRIPTION_CHARS = 100


@dataclass(frozen=True)
class Target:
    description: str
    video_track: str
    location: Location


def parse_prompt(payload: bytes, location: Location) -> Target | None:
    """Returns None for a prompt that stops tracking and raises ValueError for an invalid one."""
    body = json.loads(payload)
    if not isinstance(body, dict):
        raise ValueError("prompt is not an object")
    description = body.get("target")
    if description is None:
        return None
    video_track = body.get("video")
    if not isinstance(description, str) or not 0 < len(description) <= MAX_DESCRIPTION_CHARS:
        raise ValueError("target is not a short text")
    if not isinstance(video_track, str) or not video_track:
        raise ValueError("video is not a track name")
    return Target(description, video_track, location)


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
