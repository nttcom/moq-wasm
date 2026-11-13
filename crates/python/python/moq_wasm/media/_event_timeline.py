import json
from typing import Any

from moq_wasm._session import TrackWriter

EventRecord = dict[str, Any]


class EventTimeline:
    """Writes a draft-ietf-moq-msf-01 §8 event timeline on a track: each connection's first group
    opens with the record list, and each later record is one object of that group. With `replay`
    the list holds every record so far, so a late joiner's joining FETCH brings them all; without
    it the list is empty. A record is the §8 JSON object, such as `{"l": [group, object], "data":
    ...}` or `{"t": milliseconds, "data": ...}`."""

    def __init__(self, writer: TrackWriter, *, replay: bool = False):
        self._writer = writer
        self._records: list[EventRecord] | None = [] if replay else None
        writer._on_bound.append(self._open)

    async def append(self, record: EventRecord) -> None:
        if self._records is not None:
            self._records.append(record)
        await self._writer.write(_encode([record]))

    async def _open(self) -> None:
        await self._writer.write(_encode(self._records or []), new_group=True)


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":"), ensure_ascii=False).encode()
