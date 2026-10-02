import json
import time
from typing import Any

import moq

Location = tuple[int, int]

EventRecord = dict[str, Any]


def location_record(location: Location, data: dict[str, Any]) -> EventRecord:
    return {"l": [location[0], location[1]], "data": data}


class EventTimeline:
    """draft-ietf-moq-msf-01 §8 event timeline: the group opens with every record so far (none),
    and later objects in the group carry one new record each."""

    def __init__(self, broadcast: moq.BroadcastProducer, track_name: str):
        # The relay keeps a track's cache across publisher sessions and isolates a track
        # whose publisher repeats a location, so group ids are seeded from the wall clock.
        self._track = broadcast.publish_track(track_name)
        self._group = self._track.create_group(time.time_ns() // 1_000)
        self._group.write_frame(_encode([]))

    def append(self, record: EventRecord) -> None:
        self._group.write_frame(_encode([record]))


def _encode(records: list[EventRecord]) -> bytes:
    return json.dumps(records, separators=(",", ":")).encode()
