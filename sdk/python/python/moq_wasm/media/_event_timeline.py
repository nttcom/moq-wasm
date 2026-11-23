import json
from typing import Any

from moq_wasm._once import Once
from moq_wasm._session import TrackWriter

EventRecord = dict[str, Any]


class EventTimeline:
    """Writes a draft-ietf-moq-msf-02 §8 event timeline on a track as one group: it opens with an
    empty record list once the track is registered, at once for a writer registered already, and
    each record is one more object, so a late joiner's joining FETCH brings those the track still
    keeps (`retention_seconds`). A record is the §8 JSON object, such as `{"L": [group, object],
    "data": ...}` or `{"T": milliseconds, "data": ...}`."""

    def __init__(self, writer: TrackWriter):
        self._writer = writer
        self._opening: Once[None] = Once()
        writer._when_bound(self._open)

    async def append(self, record: EventRecord) -> None:
        """Waits until the track is registered and the timeline opened, then writes `record`.
        Raises ValueError at once for a record JSON cannot carry, such as one with a NaN, what
        failed the track's registration or the opening write, which closes the track, and
        RuntimeError once the track was closed, as by leaving its broadcast, even while the opening
        was sent."""
        payload = _encode([record])
        await self._writer.registered()
        await self._open()
        await self._writer.write(payload)

    async def _open(self) -> None:
        """Writes the opening empty record list once, ahead of every record."""
        await self._opening.run(self._write_opening)

    async def _write_opening(self) -> None:
        await self._writer.write(_encode([]), new_group=True)


def _encode(records: list[EventRecord]) -> bytes:
    """Raises ValueError for a NaN or an infinity, which JSON does not have (RFC 8259 §6)."""
    return json.dumps(records, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()
