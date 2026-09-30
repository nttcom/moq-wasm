import json
import time
from typing import Any

import moq

from moq_chat_moderation.chat import Location

EventRecord = dict[str, Any]


def location_record(location: Location, data: dict[str, Any]) -> EventRecord:
    return {"l": [location[0], location[1]], "data": data}


class EventTimeline:
    """draft-ietf-moq-msf-01 §8 event timeline: each group opens with every record so far,
    and later objects in the group carry one new record each."""

    def __init__(self, track_name: str):
        self._track_name = track_name
        self._records: list[EventRecord] = []
        self._broadcast: moq.BroadcastProducer | None = None
        self._track: moq.TrackProducer | None = None
        self._group: moq.GroupProducer | None = None

    def publish_on(self, broadcast: moq.BroadcastProducer) -> None:
        if broadcast is self._broadcast:
            return
        self._broadcast = broadcast
        self._track = broadcast.publish_track(self._track_name)
        # The relay keeps a track's cache across publisher sessions and isolates a track
        # whose publisher repeats a location, so group ids are seeded from the wall clock.
        self._group = self._track.create_group(time.time_ns() // 1_000)
        self._group.write_frame(_encode(self._records))

    def append(self, record: EventRecord) -> None:
        self._records.append(record)
        if self._group is not None:
            self._group.write_frame(_encode([record]))


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":")).encode()
