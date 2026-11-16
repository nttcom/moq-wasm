from typing import Any

from moq_wasm.media import EventRecord

# A record's place in the track it is about: (group id, object id).
Location = tuple[int, int]


def location_record(location: Location, data: dict[str, Any]) -> EventRecord:
    """A draft-ietf-moq-msf-02 §8 event timeline record indexed by `location`."""
    return {"l": [location[0], location[1]], "data": data}
