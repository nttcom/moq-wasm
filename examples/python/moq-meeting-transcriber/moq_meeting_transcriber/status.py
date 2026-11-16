import json
from collections.abc import Awaitable, Callable
from typing import Protocol

TRACK_NAME = "status"
STARTING = "starting"
READY = "ready"


class Publication(Protocol):
    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...

    def on_connected(self, callback: Callable[[], Awaitable[None]]) -> None: ...


class RecognizerStatus:
    """The `status` track: one object per group naming the room's recognizer and whether it answers
    yet, written again on each connection so a page that joins later reads it first."""

    def __init__(self, publication: Publication, asr: str):
        self._publication = publication
        self._asr = asr
        self._state = STARTING
        publication.on_connected(self._write)

    async def set(self, state: str) -> None:
        self._state = state
        await self._write()

    async def _write(self) -> None:
        payload = json.dumps({"asr": self._asr, "state": self._state}, separators=(",", ":")).encode()
        await self._publication.write(payload, new_group=True)
