import json
from typing import Protocol

TRACK_NAME = "status"
STARTING = "starting"
READY = "ready"


class Publication(Protocol):
    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...


async def write_status(publication: Publication, asr: str, state: str) -> None:
    """The `status` track: one object per group naming the room's recognizer and whether it answers yet."""
    payload = json.dumps({"asr": asr, "state": state}, separators=(",", ":")).encode()
    await publication.write(payload, new_group=True)
