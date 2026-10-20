import json
from typing import Any, Protocol

Location = tuple[int, int]

EventRecord = dict[str, Any]


class Publication(Protocol):
    async def write(self, payload: bytes, *, new_group: bool = False) -> None: ...


def location_record(location: Location, data: dict[str, Any]) -> EventRecord:
    return {"l": [location[0], location[1]], "data": data}


class EventTimeline:
    """draft-ietf-moq-msf-01 §8 event timeline: each relay session's group opens with an empty
    record list, and later objects in the group carry one new record each."""

    def __init__(self):
        self._publication: Publication | None = None

    async def publish_on(self, publication: Publication) -> None:
        self._publication = publication
        await publication.write(_encode([]), new_group=True)

    async def append(self, record: EventRecord) -> None:
        if self._publication is not None:
            await self._publication.write(_encode([record]))


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":")).encode()
