import json
from typing import Any

from moq_system_one_bot.publication import Publication

Location = tuple[int, int]

EventRecord = dict[str, Any]


def location_record(location: Location, data: dict[str, Any]) -> EventRecord:
    return {"l": [location[0], location[1]], "data": data}


class EventTimeline:
    """draft-ietf-moq-msf-01 §8 event timeline: each relay session's group opens with a record list,
    and later objects in the group carry one new record each. With `replay` the list holds every
    record so far; without it the list is empty."""

    def __init__(self, replay: bool = False):
        self._publication: Publication | None = None
        self._records: list[EventRecord] | None = [] if replay else None

    async def publish_on(self, publication: Publication) -> None:
        self._publication = publication
        await publication.write(_encode(self._records or []), new_group=True)

    async def append(self, record: EventRecord) -> None:
        if self._records is not None:
            self._records.append(record)
        if self._publication is not None:
            await self._publication.write(_encode([record]))


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":")).encode()
