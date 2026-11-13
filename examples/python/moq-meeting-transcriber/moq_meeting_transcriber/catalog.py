import moqt
from loguru import logger


def audio_track_name(catalog: bytes) -> str | None:
    """The Opus LOC audio track the Meeting's viewers subscribe to: the one with the lowest bitrate.

    The publisher encodes a profile only while someone subscribes to it, so picking the same one
    starts no extra encoder in the participant's browser.
    """
    try:
        tracks = moqt.parse_catalog(catalog).get("tracks", [])
    except ValueError as error:
        logger.warning(f"catalog ignored: {error}")
        return None
    audio = [
        track
        for track in tracks
        if track.get("role") == "audio" and track.get("packaging") == "loc" and track.get("codec") == "opus"
    ]
    if not audio:
        return None
    return min(audio, key=lambda track: track.get("bitrate", float("inf")))["name"]
