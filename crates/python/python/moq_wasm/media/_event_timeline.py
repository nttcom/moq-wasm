import json
from typing import Any

from moq_wasm._session import TrackWriter

EventRecord = dict[str, Any]


class EventTimeline:
    """Writes a draft-ietf-moq-msf-02 §8 event timeline on a track as one group: it opens with an
    empty record list once the track is registered, and each record is one more object, so a late
    joiner's joining FETCH brings those the track still keeps (`retention_seconds`). A record is the §8 JSON object, such as `{"L": [group,
    object], "data": ...}` or `{"T": milliseconds, "data": ...}`."""

    def __init__(self, writer: TrackWriter):
        self._writer = writer
        writer._on_bound.append(self._open)

    async def append(self, record: EventRecord) -> None:
        await self._writer.write(_encode([record]))

    async def _open(self) -> None:
        await self._writer.write(_encode([]), new_group=True)


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":"), ensure_ascii=False).encode()
