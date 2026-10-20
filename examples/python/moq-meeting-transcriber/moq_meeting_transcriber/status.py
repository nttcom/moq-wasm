import json

from moq_meeting_transcriber.transcript import Publication

TRACK_NAME = "status"
STARTING = "starting"
READY = "ready"


async def write_status(publication: Publication, asr: str, state: str) -> None:
    """The `status` track: one object per group naming the room's recognizer and whether it answers yet."""
    payload = json.dumps({"asr": asr, "state": state}, separators=(",", ":")).encode()
    await publication.write(payload, new_group=True)
